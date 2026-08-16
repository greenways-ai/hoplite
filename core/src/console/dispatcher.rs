use super::protocol::{
    failure, map_get, map_value, read_hta_frame, string_value, success, write_hta_frame,
    CommandSet, ConsoleGrant, ConsoleLimits,
};
use hara_wasm::core::Value;
use hara_wasm::hta;
use std::io::{Read, Write};

/// The one prepared HAL command function owned by an application worker.
///
/// Implementations adapt Hoplite's existing prepared-handler/work boundary and
/// drive the returned work to its immutable `Result`. The function is selected
/// once from trusted application configuration. No caller-controlled source,
/// symbol, handler identifier, or function name enters this interface.
pub trait PreparedHalBoundary {
    fn call(&mut self, input: Value) -> Result<Value, String>;
}

pub struct ApplicationConsoleDispatcher<H> {
    commands: CommandSet,
    limits: ConsoleLimits,
    handler: H,
}

impl<H: PreparedHalBoundary> ApplicationConsoleDispatcher<H> {
    pub fn new(commands: CommandSet, limits: ConsoleLimits, handler: H) -> Result<Self, String> {
        Ok(Self {
            commands,
            limits: limits.validate()?,
            handler,
        })
    }

    pub fn commands(&self, grant: &Value) -> Result<Value, String> {
        let grant = ConsoleGrant::parse(grant)?;
        self.commands.validate_grant(&grant)?;
        Ok(self.commands.granted_value(&grant))
    }

    pub fn call(&mut self, grant: &Value, command: &str, input: Value) -> Result<Value, String> {
        let parsed_grant = ConsoleGrant::parse(grant)?;
        self.commands
            .validate_call(&parsed_grant, command, &input, self.limits.result_bytes)?;
        let envelope = map_value(vec![
            ("grant", parsed_grant.to_value()),
            ("command", Value::String(command.into())),
            ("input", input),
        ]);
        let value = self.handler.call(envelope)?;
        let encoded = hta::encode(&value)
            .map_err(|error| format!("hoplite.console/result-not-immutable: {error}"))?;
        if encoded.len() > self.limits.result_bytes {
            return Err("hoplite.console/result-too-large".into());
        }
        Ok(value)
    }

    pub fn handle_broker_request(&mut self, request: Value) -> Value {
        let operation = map_get(&request, "op")
            .and_then(|value| string_value(&value).ok())
            .unwrap_or_default();
        let result = match operation.as_str() {
            "commands" => map_get(&request, "grant")
                .ok_or_else(|| "hoplite.console/grant-invalid".to_string())
                .and_then(|grant| self.commands(&grant)),
            "call" => {
                let grant = map_get(&request, "grant")
                    .ok_or_else(|| "hoplite.console/grant-invalid".to_string());
                let command = map_get(&request, "command")
                    .ok_or_else(|| "hoplite.console/command-unlisted".to_string())
                    .and_then(|value| string_value(&value));
                let input = map_get(&request, "input")
                    .ok_or_else(|| "hoplite.console/input-invalid".to_string());
                match (grant, command, input) {
                    (Ok(grant), Ok(command), Ok(input)) => self.call(&grant, &command, input),
                    (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
                }
            }
            _ => Err("hoplite.console/operation-unlisted".into()),
        };
        match result {
            Ok(value) => success(value),
            Err(error) => {
                let code = error_code(&error).to_owned();
                failure(code, error)
            }
        }
    }

    pub fn serve<RW: Read + Write>(&mut self, stream: &mut RW) -> Result<(), String> {
        while let Some(request) = read_hta_frame(stream, self.limits.result_bytes)? {
            let response = self.handle_broker_request(request);
            write_hta_frame(stream, &response, self.limits.result_bytes)?;
        }
        Ok(())
    }
}

fn error_code(error: &str) -> &str {
    error
        .split_once(':')
        .map(|(prefix, _)| prefix)
        .unwrap_or(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::protocol::{value_sequence, CommandSet, GRANT_PROTOCOL};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone)]
    struct RecordingBoundary {
        calls: Rc<RefCell<Vec<Value>>>,
    }

    impl PreparedHalBoundary for RecordingBoundary {
        fn call(&mut self, input: Value) -> Result<Value, String> {
            self.calls.borrow_mut().push(input);
            Ok(map_value(vec![
                ("ok", Value::Bool(true)),
                ("state", map_value(vec![])),
                ("value", Value::String("ready".into())),
                ("effects", Value::Vector(Default::default())),
            ]))
        }
    }

    fn descriptor(command: &str, effect: &str) -> Value {
        map_value(vec![
            ("command", Value::String(command.into())),
            ("effect", Value::Keyword(effect.into())),
            (
                "input",
                map_value(vec![
                    ("type", Value::Keyword("map".into())),
                    ("required", Value::Set(Default::default())),
                    ("optional", Value::Set(Default::default())),
                ]),
            ),
        ])
    }

    fn commands() -> CommandSet {
        CommandSet::parse(Value::Vector(
            vec![descriptor("status", "read"), descriptor("quit", "write")].into(),
        ))
        .unwrap()
    }

    fn grant(write: bool) -> Value {
        map_value(vec![
            ("protocol", Value::String(GRANT_PROTOCOL.into())),
            ("console", Value::String("console.test".into())),
            (
                "commands",
                Value::Set(
                    vec![Value::String("status".into()), Value::String("quit".into())].into(),
                ),
            ),
            ("write", Value::Bool(write)),
        ])
    }

    #[test]
    fn dispatcher_calls_only_the_prepared_boundary_with_a_closed_envelope() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let boundary = RecordingBoundary {
            calls: calls.clone(),
        };
        let mut dispatcher =
            ApplicationConsoleDispatcher::new(commands(), ConsoleLimits::default(), boundary)
                .unwrap();
        let result = dispatcher
            .call(&grant(false), "status", map_value(vec![]))
            .unwrap();
        assert_eq!(map_get(&result, "ok"), Some(Value::Bool(true)));
        let calls = calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            map_get(&calls[0], "command"),
            Some(Value::String("status".into()))
        );
        assert!(map_get(&calls[0], "grant").is_some());
        assert!(map_get(&calls[0], "input").is_some());
        assert!(map_get(&calls[0], "handler").is_none());
        assert!(map_get(&calls[0], "source").is_none());
    }

    #[test]
    fn dispatcher_validates_before_entering_the_handler_boundary() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let boundary = RecordingBoundary {
            calls: calls.clone(),
        };
        let mut dispatcher =
            ApplicationConsoleDispatcher::new(commands(), ConsoleLimits::default(), boundary)
                .unwrap();
        assert_eq!(
            dispatcher
                .call(&grant(false), "runtime.eval", map_value(vec![]))
                .unwrap_err(),
            "hoplite.console/command-unlisted"
        );
        assert_eq!(
            dispatcher
                .call(&grant(false), "quit", map_value(vec![]))
                .unwrap_err(),
            "hoplite.console/write-not-granted"
        );
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn command_listing_is_grant_filtered() {
        let boundary = RecordingBoundary {
            calls: Rc::new(RefCell::new(Vec::new())),
        };
        let dispatcher =
            ApplicationConsoleDispatcher::new(commands(), ConsoleLimits::default(), boundary)
                .unwrap();
        assert_eq!(
            value_sequence(&dispatcher.commands(&grant(false)).unwrap())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            value_sequence(&dispatcher.commands(&grant(true)).unwrap())
                .unwrap()
                .len(),
            2
        );
    }
}
