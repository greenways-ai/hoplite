use super::dispatcher::{ApplicationConsoleDispatcher, PreparedHalBoundary};
use super::os;
use super::protocol::{
    failure, map_get, read_hta_frame, string_value, write_hta_frame, ConsoleLimits,
};
use hara_native::core::{self, Value};
use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ApplicationBrokerConfig {
    pub socket_path: PathBuf,
    pub limits: ConsoleLimits,
}

impl ApplicationBrokerConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.limits.validate()?;
        if !self.socket_path.is_absolute() {
            return Err("application console broker socket must be absolute".into());
        }
        Ok(())
    }
}

/// Run the private application-worker side of the named-command console.
///
/// The dispatcher owns one prepared HAL boundary selected from the immutable
/// application manifest. Each accepted connection carries exactly one bounded
/// HTA call and is authenticated as the worker's operating-system user.
pub fn run_application_broker<H: PreparedHalBoundary>(
    config: ApplicationBrokerConfig,
    dispatcher: &mut ApplicationConsoleDispatcher<H>,
) -> Result<(), String> {
    config.validate()?;
    let listener = bind_private_socket(&config.socket_path)?;
    loop {
        let (mut stream, _) = listener.accept().map_err(|error| {
            format!("cannot accept application console broker connection: {error}")
        })?;
        if let Err(error) = os::authenticate_peer(&stream) {
            let _ = write_hta_frame(
                &mut stream,
                &failure("hoplite.console/peer-denied", error),
                config.limits.result_bytes,
            );
            continue;
        }
        if let Err(error) = serve_authenticated(&mut stream, dispatcher, config.limits.result_bytes)
        {
            let code = error_code(&error).to_owned();
            let _ = write_hta_frame(
                &mut stream,
                &failure(&code, error),
                config.limits.result_bytes,
            );
        }
    }
}

/// Serve one already connected private broker stream.
///
/// This entry point is useful for a worker event-loop adapter: it performs the
/// same peer authentication and exact-envelope validation as the blocking
/// reference server without exposing the prepared handler or application
/// runtime to the caller.
pub fn serve_application_broker_connection<H: PreparedHalBoundary>(
    mut stream: UnixStream,
    dispatcher: &mut ApplicationConsoleDispatcher<H>,
    limits: ConsoleLimits,
) -> Result<(), String> {
    let limits = limits.validate()?;
    os::authenticate_peer(&stream)?;
    serve_authenticated(&mut stream, dispatcher, limits.result_bytes)
}

fn serve_authenticated<H: PreparedHalBoundary>(
    stream: &mut UnixStream,
    dispatcher: &mut ApplicationConsoleDispatcher<H>,
    maximum_bytes: usize,
) -> Result<(), String> {
    let request = read_hta_frame(stream, maximum_bytes)?.ok_or_else(|| {
        "hoplite.console/request-invalid: broker closed without a call".to_string()
    })?;
    let response = match validate_call_request(&request) {
        Ok(()) => dispatcher.handle_broker_request(request),
        Err(error) => {
            let code = error_code(&error).to_owned();
            failure(&code, error)
        }
    };
    write_hta_frame(stream, &response, maximum_bytes)
}

fn validate_call_request(request: &Value) -> Result<(), String> {
    let entries = core::map_entries(request).ok_or_else(|| {
        "hoplite.console/request-invalid: application broker request must be a map".to_string()
    })?;
    if entries.len() != 4 {
        return Err(
            "hoplite.console/request-invalid: application broker request must contain exactly op, grant, command and input"
                .into(),
        );
    }
    for (key, _) in entries {
        let name = string_value(&key).map_err(|_| {
            "hoplite.console/request-invalid: application broker keys must be text".to_string()
        })?;
        if !matches!(name.as_str(), "op" | "grant" | "command" | "input") {
            return Err(format!(
                "hoplite.console/request-invalid: unsupported application broker field {name:?}"
            ));
        }
    }
    if map_get(request, "op")
        .and_then(|value| string_value(&value).ok())
        .as_deref()
        != Some("call")
    {
        return Err("hoplite.console/operation-unlisted".into());
    }
    if map_get(request, "grant").is_none()
        || map_get(request, "command").is_none()
        || map_get(request, "input").is_none()
    {
        return Err(
            "hoplite.console/request-invalid: application broker call is incomplete".into(),
        );
    }
    Ok(())
}

fn bind_private_socket(path: &Path) -> Result<UnixListener, String> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() {
            return Err(format!(
                "refusing to replace non-socket application console broker path {}",
                path.display()
            ));
        }
        fs::remove_file(path).map_err(|error| {
            format!("cannot remove stale application console broker socket: {error}")
        })?;
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "cannot create application console broker directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let listener = UnixListener::bind(path).map_err(|error| {
        format!(
            "cannot bind application console broker socket {}: {error}",
            path.display()
        )
    })?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        format!("cannot set application console broker socket mode 0600: {error}")
    })?;
    let mode = fs::metadata(path)
        .map_err(|error| format!("cannot inspect application console broker socket: {error}"))?
        .permissions()
        .mode()
        & 0o777;
    if mode != 0o600 {
        return Err(format!(
            "application console broker socket mode is {mode:o}, expected 600"
        ));
    }
    Ok(listener)
}

fn error_code(error: &str) -> &str {
    error.split_once(':').map(|(code, _)| code).unwrap_or(error)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::console::protocol::{map_value, success, CommandSet, GRANT_PROTOCOL};
    use std::cell::RefCell;
    use std::os::unix::fs::PermissionsExt;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_SOCKET: AtomicU64 = AtomicU64::new(1);

    #[derive(Clone)]
    struct RecordingBoundary {
        calls: Rc<RefCell<Vec<Value>>>,
    }

    impl PreparedHalBoundary for RecordingBoundary {
        fn call(&mut self, input: Value) -> Result<Value, String> {
            self.calls.borrow_mut().push(input.clone());
            Ok(input)
        }
    }

    fn descriptor(command: &str) -> Value {
        map_value(vec![
            ("command", Value::String(command.into())),
            ("effect", Value::Keyword("read".into())),
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

    fn grant() -> Value {
        map_value(vec![
            ("protocol", Value::String(GRANT_PROTOCOL.into())),
            ("console", Value::String("console.test".into())),
            (
                "commands",
                Value::Set(vec![Value::String("status".into())].into()),
            ),
            ("write", Value::Bool(false)),
        ])
    }

    fn dispatcher(
        calls: Rc<RefCell<Vec<Value>>>,
    ) -> ApplicationConsoleDispatcher<RecordingBoundary> {
        ApplicationConsoleDispatcher::new(
            CommandSet::parse(Value::Vector(vec![descriptor("status")].into())).unwrap(),
            ConsoleLimits::default(),
            RecordingBoundary { calls },
        )
        .unwrap()
    }

    fn call_request(extra: Option<(&str, Value)>) -> Value {
        let mut entries = vec![
            ("op", Value::String("call".into())),
            ("grant", grant()),
            ("command", Value::String("status".into())),
            ("input", map_value(vec![])),
        ];
        if let Some(extra) = extra {
            entries.push(extra);
        }
        map_value(entries)
    }

    #[test]
    fn private_broker_forwards_only_the_closed_call_envelope() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut dispatcher = dispatcher(calls.clone());
        let (mut client, server) = UnixStream::pair().unwrap();
        write_hta_frame(&mut client, &call_request(None), 1024 * 1024).unwrap();
        serve_application_broker_connection(server, &mut dispatcher, ConsoleLimits::default())
            .unwrap();
        let response = read_hta_frame(&mut client, 1024 * 1024).unwrap().unwrap();
        assert_eq!(map_get(&response, "ok"), Some(Value::Bool(true)));
        let calls = calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            map_get(&calls[0], "command"),
            Some(Value::String("status".into()))
        );
        assert!(map_get(&calls[0], "grant").is_some());
        assert!(map_get(&calls[0], "input").is_some());
        assert!(map_get(&calls[0], "op").is_none());
        assert!(map_get(&calls[0], "handler").is_none());
        assert!(map_get(&calls[0], "source").is_none());
    }

    #[test]
    fn private_broker_rejects_routing_fields_before_hal() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut dispatcher = dispatcher(calls.clone());
        let (mut client, server) = UnixStream::pair().unwrap();
        write_hta_frame(
            &mut client,
            &call_request(Some((
                "handler",
                Value::String("tahto.node.console/dispatch".into()),
            ))),
            1024 * 1024,
        )
        .unwrap();
        serve_application_broker_connection(server, &mut dispatcher, ConsoleLimits::default())
            .unwrap();
        let response = read_hta_frame(&mut client, 1024 * 1024).unwrap().unwrap();
        assert_eq!(map_get(&response, "ok"), Some(Value::Bool(false)));
        assert_eq!(
            map_get(&map_get(&response, "error").unwrap(), "code"),
            Some(Value::String("hoplite.console/request-invalid".into()))
        );
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn application_broker_socket_is_mode_0600() {
        let path = std::env::temp_dir().join(format!(
            "hoplite-application-broker-{}-{}.sock",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = bind_private_socket(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(listener);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn bounded_failure_envelope_remains_transferable() {
        let value = success(map_value(vec![]));
        assert!(hara_native::hta::encode(&value).is_ok());
    }
}
