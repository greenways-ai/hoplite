use super::os;
use super::protocol::{
    failure, map_get, map_value, read_hta_frame, string_value, success, write_frame,
    write_hta_frame, ClientBundle, ConsoleLimits, BROKER_SERVICE, MAX_CLIENT_BUNDLE_BYTES,
    REQUEST_PROTOCOL,
};
use crate::hara_source;
use hara_native::core::{self, Value};
use hara_native::hta;
use hara_native::kernel::{parse_forms, Form};
use hara_native::Runtime;
use std::cell::RefCell;
use std::fs::File;
use std::io::Read;
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::rc::Rc;

const MAX_BUNDLE_FRAME_BYTES: usize = MAX_CLIENT_BUNDLE_BYTES + 4096;
const READY_PROTOCOL: &str = "hoplite.console-evaluator-ready/0-alpha";

#[derive(Clone, Debug)]
pub struct EvaluatorConfig {
    pub evaluation_fd: RawFd,
    pub broker_fd: RawFd,
    pub bundle_fd: RawFd,
    pub namespace: String,
    pub limits: ConsoleLimits,
}

pub fn run_evaluator(config: EvaluatorConfig) -> Result<(), String> {
    let limits = config.limits.validate()?;
    let mut evaluation = unsafe { UnixStream::from_raw_fd(config.evaluation_fd) };
    let broker = unsafe { UnixStream::from_raw_fd(config.broker_fd) };
    let bundle_file = unsafe { File::from_raw_fd(config.bundle_fd) };
    let bundle = read_bundle(bundle_file)?;
    if bundle.namespace != config.namespace {
        return Err(format!(
            "console client bundle declares {:?}, expected {:?}",
            bundle.namespace, config.namespace
        ));
    }
    validate_declared_namespace(&bundle.source, &bundle.namespace)?;

    // Foundation is mounted from the reviewed source checkout before the
    // evaluator's OS sandbox is enabled. The production server never starts
    // this source-side compiler/evaluator process.
    let mut runtime = hara_source::compiler_runtime()?;
    runtime.register_resource(&bundle.namespace, &bundle.source);
    runtime
        .eval_native_value(&bundle.source)
        .map_err(|error| format!("cannot load console client namespace: {error}"))?;
    install_broker(&mut runtime, broker, limits.result_bytes);

    // Load all trusted code before denying filesystem, network, process and
    // cross-process operations at the OS boundary. Source received from the
    // console is evaluated only after this policy is active.
    os::install_evaluator_sandbox()?;
    write_hta_frame(
        &mut evaluation,
        &success(map_value(vec![
            ("protocol", Value::String(READY_PROTOCOL.into())),
            ("namespace", Value::String(bundle.namespace)),
            ("authority", Value::Keyword("zero".into())),
        ])),
        4096,
    )?;

    while let Some(request) = read_hta_frame(&mut evaluation, limits.source_bytes + 4096)? {
        let response = evaluate_request(&mut runtime, request, limits);
        let encoded = encode_bounded_response(response, limits.result_bytes);
        write_frame(&mut evaluation, &encoded, limits.result_bytes)?;
    }
    Ok(())
}

fn read_bundle(mut file: File) -> Result<ClientBundle, String> {
    let mut bytes = Vec::new();
    file.by_ref()
        .take((MAX_BUNDLE_FRAME_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read console client bundle: {error}"))?;
    if bytes.len() > MAX_BUNDLE_FRAME_BYTES {
        return Err("console client bundle exceeds its maximum size".into());
    }
    ClientBundle::decode(&bytes)
}

fn validate_declared_namespace(source: &str, expected: &str) -> Result<(), String> {
    let forms = parse_forms(source)?;
    let declared = forms.into_iter().find_map(|form| match form {
        Form::List(values)
            if matches!(values.first(), Some(Form::Symbol(head)) if head == "ns") =>
        {
            match values.get(1) {
                Some(Form::Symbol(namespace)) => Some(namespace.clone()),
                _ => None,
            }
        }
        _ => None,
    });
    match declared.as_deref() {
        Some(namespace) if namespace == expected => Ok(()),
        Some(namespace) => Err(format!(
            "console client source declares namespace {namespace:?}, expected {expected:?}"
        )),
        None => Err("console client source must declare an ns namespace".into()),
    }
}

fn install_broker(runtime: &mut Runtime, broker: UnixStream, maximum: usize) {
    let broker = Rc::new(RefCell::new(broker));
    runtime.install_native_host_handler(Rc::new(move |service, method, arguments| {
        if service != BROKER_SERVICE {
            return Err(format!(
                "hoplite.console/service-denied: evaluator cannot call service {service:?}"
            ));
        }
        let request = match method.as_str() {
            "commands" if arguments.is_empty() => {
                map_value(vec![("op", Value::String("commands".into()))])
            }
            "call" if arguments.len() == 1 => {
                let request = arguments[0].clone();
                validate_client_call(&request)?;
                map_value(vec![
                    ("op", Value::String("call".into())),
                    ("request", request),
                ])
            }
            "commands" => {
                return Err("hoplite.console/commands-arity: commands expects no arguments".into())
            }
            "call" => return Err("hoplite.console/call-arity: call expects one request map".into()),
            _ => {
                return Err(format!(
                    "hoplite.console/operation-unlisted: evaluator cannot call method {method:?}"
                ))
            }
        };
        let mut broker = broker.borrow_mut();
        write_hta_frame(&mut *broker, &request, maximum)?;
        let response = read_hta_frame(&mut *broker, maximum)?
            .ok_or_else(|| "hoplite.console/broker-closed".to_string())?;
        broker_result(response)
    }));
}

fn validate_client_call(request: &Value) -> Result<(), String> {
    let entries = core::map_entries(request)
        .ok_or_else(|| "hoplite.console/request-invalid: request must be a map".to_string())?;
    if entries.len() != 3 {
        return Err("hoplite.console/request-invalid: request must contain exactly protocol, command and input".into());
    }
    let protocol = map_get(request, "protocol")
        .ok_or_else(|| "hoplite.console/request-invalid: missing protocol".to_string())
        .and_then(|value| string_value(&value))?;
    if protocol != REQUEST_PROTOCOL {
        return Err("hoplite.console/request-invalid: unsupported request protocol".into());
    }
    let command = map_get(request, "command")
        .ok_or_else(|| "hoplite.console/request-invalid: missing command".to_string())
        .and_then(|value| string_value(&value))?;
    if command.is_empty() || command.len() > 128 {
        return Err("hoplite.console/request-invalid: invalid command name".into());
    }
    let input = map_get(request, "input")
        .ok_or_else(|| "hoplite.console/request-invalid: missing input".to_string())?;
    hta::encode(&input).map_err(|error| format!("hoplite.console/input-not-immutable: {error}"))?;
    Ok(())
}

fn broker_result(response: Value) -> Result<Value, String> {
    match map_get(&response, "ok") {
        Some(Value::Bool(true)) => map_get(&response, "value")
            .ok_or_else(|| "hoplite.console/broker-response-invalid: missing value".into()),
        Some(Value::Bool(false)) => {
            let error = map_get(&response, "error").ok_or_else(|| {
                "hoplite.console/broker-response-invalid: missing error".to_string()
            })?;
            let code = map_get(&error, "code")
                .and_then(|value| string_value(&value).ok())
                .unwrap_or_else(|| "hoplite.console/call-failed".into());
            let message = map_get(&error, "message")
                .and_then(|value| string_value(&value).ok())
                .unwrap_or_else(|| code.clone());
            Err(format!("{code}: {message}"))
        }
        _ => Err("hoplite.console/broker-response-invalid: missing boolean ok".into()),
    }
}

fn evaluate_request(runtime: &mut Runtime, request: Value, limits: ConsoleLimits) -> Value {
    let operation = map_get(&request, "op")
        .and_then(|value| string_value(&value).ok())
        .unwrap_or_default();
    if operation != "eval" {
        return failure(
            "hoplite.console/evaluation-operation-unlisted",
            "the evaluator accepts only eval requests",
        );
    }
    let source = match map_get(&request, "source").and_then(|value| match value {
        Value::String(source) => Some(source),
        _ => None,
    }) {
        Some(source) => source,
        None => {
            return failure(
                "hoplite.console/source-invalid",
                "evaluation source must be a string",
            )
        }
    };
    if source.len() > limits.source_bytes {
        return failure(
            "hoplite.console/source-too-large",
            "evaluation source exceeds the configured limit",
        );
    }
    match runtime.eval_native_value(&source) {
        Ok(value) => success(value),
        Err(error) => failure("hoplite.console/evaluation-failed", error),
    }
}

fn encode_bounded_response(response: Value, maximum: usize) -> Vec<u8> {
    match hta::encode(&response) {
        Ok(bytes) if bytes.len() <= maximum => bytes,
        Ok(_) => hta::encode(&failure(
            "hoplite.console/result-too-large",
            "evaluation result exceeds the configured limit",
        ))
        .expect("bounded console failure is HTA-compatible"),
        Err(error) => hta::encode(&failure(
            "hoplite.console/live-result-rejected",
            format!("evaluation returned a non-transferable live value: {error}"),
        ))
        .expect("bounded console failure is HTA-compatible"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_bundle_must_declare_the_expected_namespace() {
        assert!(
            validate_declared_namespace("(ns tahto.console (:config {}))", "tahto.console").is_ok()
        );
        assert!(validate_declared_namespace("(+ 1 2)", "tahto.console").is_err());
        assert!(validate_declared_namespace(
            "(ns tahto.application (:config {}))",
            "tahto.console"
        )
        .is_err());
    }

    #[test]
    fn client_calls_have_an_exact_immutable_envelope() {
        let valid = map_value(vec![
            ("protocol", Value::String(REQUEST_PROTOCOL.into())),
            ("command", Value::String("status".into())),
            ("input", map_value(vec![])),
        ]);
        assert!(validate_client_call(&valid).is_ok());
        assert!(validate_client_call(&map_value(vec![
            ("protocol", Value::String(REQUEST_PROTOCOL.into())),
            ("command", Value::String("status".into())),
            ("input", map_value(vec![])),
            ("handler", Value::String("tahto.node.app/handler".into())),
        ]))
        .is_err());
    }

    #[test]
    fn live_values_are_replaced_with_a_bounded_transfer_error() {
        let live = success(Value::Promise(hara_native::core::Promise::new()));
        let encoded = encode_bounded_response(live, 1024);
        let decoded = hta::decode(&encoded).unwrap();
        assert_eq!(map_get(&decoded, "ok"), Some(Value::Bool(false)));
        assert_eq!(
            map_get(&map_get(&decoded, "error").unwrap(), "code"),
            Some(Value::String("hoplite.console/live-result-rejected".into()))
        );
    }
}
