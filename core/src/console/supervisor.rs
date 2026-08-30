use super::os::{self, WaitEvent};
use super::protocol::{
    failure, map_get, map_value, read_hta_frame, string_value, success, write_hta_frame,
    CommandSet, ConsoleGrant, ConsoleLimits, REQUEST_PROTOCOL,
};
use hara_native::core::{self, Value};
use hara_native::hta;
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const EVALUATOR_READY_PROTOCOL: &str = "hoplite.console-evaluator-ready/0-alpha";
const CONNECTION_READY_PROTOCOL: &str = "hoplite.console-ready/0-alpha";
const CONNECTION_FRAME_OVERHEAD: usize = 4096;
static NEXT_CONSOLE: AtomicU64 = AtomicU64::new(1);

/// A generic adapter from the supervisor's validated named command to the
/// application's prepared HAL dispatcher boundary.
pub trait CommandBroker: Send + Sync + 'static {
    fn call(&self, grant: &Value, command: &str, input: Value) -> Result<Value, String>;
}

#[derive(Clone, Debug)]
pub struct UnixCommandBroker {
    pub socket_path: PathBuf,
    pub maximum_bytes: usize,
    pub timeout: Duration,
}

impl CommandBroker for UnixCommandBroker {
    fn call(&self, grant: &Value, command: &str, input: Value) -> Result<Value, String> {
        let mut stream = UnixStream::connect(&self.socket_path).map_err(|error| {
            format!(
                "cannot connect to application console broker {}: {error}",
                self.socket_path.display()
            )
        })?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|_| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|error| format!("cannot configure application console broker: {error}"))?;
        write_hta_frame(
            &mut stream,
            &map_value(vec![
                ("op", Value::String("call".into())),
                ("grant", grant.clone()),
                ("command", Value::String(command.into())),
                ("input", input),
            ]),
            self.maximum_bytes,
        )?;
        let response = read_hta_frame(&mut stream, self.maximum_bytes)?
            .ok_or_else(|| "application console broker closed without a result".to_string())?;
        response_result(response)
    }
}

#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    pub socket_path: PathBuf,
    pub evaluator_path: PathBuf,
    pub bundle_path: PathBuf,
    pub client_namespace: String,
    pub descriptors_hta: Vec<u8>,
    pub grant_hta: Vec<u8>,
    pub limits: ConsoleLimits,
}

impl SupervisorConfig {
    pub fn from_files(
        socket_path: PathBuf,
        evaluator_path: PathBuf,
        bundle_path: PathBuf,
        client_namespace: String,
        descriptors_path: &Path,
        grant_path: &Path,
        limits: ConsoleLimits,
    ) -> Result<Self, String> {
        let descriptors_hta = fs::read(descriptors_path).map_err(|error| {
            format!(
                "cannot read console descriptors {}: {error}",
                descriptors_path.display()
            )
        })?;
        let grant_hta = fs::read(grant_path).map_err(|error| {
            format!(
                "cannot read console grant {}: {error}",
                grant_path.display()
            )
        })?;
        let config = Self {
            socket_path,
            evaluator_path,
            bundle_path,
            client_namespace,
            descriptors_hta,
            grant_hta,
            limits,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.limits.validate()?;
        if !self.evaluator_path.is_absolute() {
            return Err("console evaluator path must be absolute".into());
        }
        if !self.evaluator_path.is_file() {
            return Err(format!(
                "console evaluator does not exist: {}",
                self.evaluator_path.display()
            ));
        }
        let descriptors = hta::decode(&self.descriptors_hta)
            .map_err(|error| format!("console descriptors are not valid HTA: {error}"))?;
        let commands = CommandSet::parse(descriptors)?;
        let grant = hta::decode(&self.grant_hta)
            .map_err(|error| format!("console grant is not valid HTA: {error}"))?;
        let grant = ConsoleGrant::parse(&grant)?;
        commands.validate_grant(&grant)?;
        let bundle = open_immutable_bundle(&self.bundle_path)?;
        drop(bundle);
        Ok(())
    }
}

pub fn run_supervisor(
    config: SupervisorConfig,
    broker: Arc<dyn CommandBroker>,
) -> Result<(), String> {
    config.validate()?;
    let listener = bind_private_socket(&config.socket_path)?;
    loop {
        let (stream, _) = listener
            .accept()
            .map_err(|error| format!("cannot accept console connection: {error}"))?;
        if let Err(error) = os::authenticate_peer(&stream) {
            let mut stream = stream;
            let _ = write_hta_frame(
                &mut stream,
                &failure("hoplite.console/peer-denied", error),
                config.limits.result_bytes,
            );
            continue;
        }
        let connection = config.clone();
        let broker = broker.clone();
        thread::Builder::new()
            .name("hoplite-console-connection".into())
            .spawn(move || {
                let _ = serve_one_authenticated(stream, connection, broker);
            })
            .map_err(|error| format!("cannot start console connection supervisor: {error}"))?;
    }
}

pub fn serve_one(
    stream: UnixStream,
    config: SupervisorConfig,
    broker: Arc<dyn CommandBroker>,
) -> Result<(), String> {
    config.validate()?;
    os::authenticate_peer(&stream)?;
    serve_one_authenticated(stream, config, broker)
}

fn serve_one_authenticated(
    mut client: UnixStream,
    config: SupervisorConfig,
    broker: Arc<dyn CommandBroker>,
) -> Result<(), String> {
    let commands_value = hta::decode(&config.descriptors_hta)
        .map_err(|error| format!("console descriptors are not valid HTA: {error}"))?;
    let commands = CommandSet::parse(commands_value)?;
    let grant_value = hta::decode(&config.grant_hta)
        .map_err(|error| format!("console grant is not valid HTA: {error}"))?;
    let grant_template = ConsoleGrant::parse(&grant_value)?;
    commands.validate_grant(&grant_template)?;
    let grant = grant_template.with_console(connection_id())?;

    let (parent_evaluation, child_evaluation) =
        UnixStream::pair().map_err(|error| format!("cannot create evaluator channel: {error}"))?;
    let (parent_broker, child_broker) = UnixStream::pair()
        .map_err(|error| format!("cannot create command broker channel: {error}"))?;
    let bundle = open_immutable_bundle(&config.bundle_path)?;
    let mut child = spawn_evaluator(&config, child_evaluation, child_broker, bundle)?;

    let descriptors_hta = config.descriptors_hta.clone();
    let grant_hta = hta::encode(&grant.to_value())
        .map_err(|error| format!("cannot encode per-console grant: {error}"))?;
    let limits = config.limits;
    let broker_worker = thread::Builder::new()
        .name("hoplite-console-command-broker".into())
        .spawn(move || {
            let _ = broker_loop(parent_broker, descriptors_hta, grant_hta, limits, broker);
        })
        .map_err(|error| {
            terminate(&mut child);
            format!("cannot start console command broker: {error}")
        })?;

    let result = supervise_connection(&mut client, parent_evaluation, &mut child, limits);
    terminate(&mut child);
    // The production Unix broker has bounded socket timeouts. Dropping a live
    // JoinHandle detaches rather than blocking evaluator teardown if another
    // implementation violates that contract.
    if broker_worker.is_finished() {
        let _ = broker_worker.join();
    }
    result
}

fn supervise_connection(
    client: &mut UnixStream,
    mut evaluator: UnixStream,
    child: &mut Child,
    limits: ConsoleLimits,
) -> Result<(), String> {
    await_ready(client, &mut evaluator, child, limits)?;
    write_hta_frame(
        client,
        &success(map_value(vec![
            ("protocol", Value::String(CONNECTION_READY_PROTOCOL.into())),
            ("source-bytes", Value::Number(limits.source_bytes as i64)),
            ("result-bytes", Value::Number(limits.result_bytes as i64)),
            (
                "evaluation-millis",
                Value::Number(limits.evaluation_millis as i64),
            ),
            ("memory-bytes", Value::Number(limits.memory_bytes as i64)),
        ])),
        4096,
    )?;
    loop {
        let Some(request) =
            read_hta_frame(client, limits.source_bytes + CONNECTION_FRAME_OVERHEAD)?
        else {
            return Ok(());
        };
        validate_eval_request(&request, limits.source_bytes)?;
        write_hta_frame(
            &mut evaluator,
            &request,
            limits.source_bytes + CONNECTION_FRAME_OVERHEAD,
        )?;
        match os::wait_event(
            evaluator.as_raw_fd(),
            client.as_raw_fd(),
            Duration::from_millis(limits.evaluation_millis),
        )? {
            WaitEvent::EvaluatorReadable => {
                let response = read_hta_frame(&mut evaluator, limits.result_bytes)?
                    .ok_or_else(|| "console evaluator closed without a result".to_string())?;
                write_hta_frame(client, &response, limits.result_bytes)?;
            }
            WaitEvent::ClientClosed => return Ok(()),
            WaitEvent::ClientReadable => {
                client
                    .set_read_timeout(Some(Duration::from_millis(250)))
                    .map_err(|error| {
                        format!("cannot configure console cancellation read: {error}")
                    })?;
                let cancellation = read_hta_frame(client, CONNECTION_FRAME_OVERHEAD);
                let _ = client.set_read_timeout(None);
                let cancellation = cancellation?;
                if cancellation.as_ref().is_some_and(cancel_request) {
                    let _ = write_hta_frame(
                        client,
                        &failure(
                            "hoplite.console/evaluation-cancelled",
                            "active evaluation was cancelled",
                        ),
                        limits.result_bytes,
                    );
                    return Ok(());
                }
                let _ = write_hta_frame(
                    client,
                    &failure(
                        "hoplite.console/evaluation-active",
                        "only one evaluation may be active on a console connection",
                    ),
                    limits.result_bytes,
                );
                return Ok(());
            }
            WaitEvent::Timeout => {
                let _ = write_hta_frame(
                    client,
                    &failure(
                        "hoplite.console/evaluation-timeout",
                        "evaluation exceeded its configured wall-clock limit",
                    ),
                    limits.result_bytes,
                );
                return Ok(());
            }
            WaitEvent::EvaluatorClosed => {
                let _ = write_hta_frame(
                    client,
                    &failure(
                        "hoplite.console/evaluator-terminated",
                        "evaluator exited or violated a resource policy",
                    ),
                    limits.result_bytes,
                );
                return Ok(());
            }
        }
    }
}

fn await_ready(
    client: &mut UnixStream,
    evaluator: &mut UnixStream,
    child: &mut Child,
    limits: ConsoleLimits,
) -> Result<(), String> {
    match os::wait_event(
        evaluator.as_raw_fd(),
        client.as_raw_fd(),
        Duration::from_millis(limits.evaluation_millis),
    )? {
        WaitEvent::EvaluatorReadable => {
            let response = read_hta_frame(evaluator, 4096)?
                .ok_or_else(|| "console evaluator closed during startup".to_string())?;
            let value = response_result(response)?;
            if map_get(&value, "protocol")
                .and_then(|value| string_value(&value).ok())
                .as_deref()
                != Some(EVALUATOR_READY_PROTOCOL)
            {
                return Err("console evaluator returned an invalid startup handshake".into());
            }
            Ok(())
        }
        WaitEvent::ClientClosed => {
            terminate(child);
            Err("console client disconnected during evaluator startup".into())
        }
        WaitEvent::ClientReadable => {
            terminate(child);
            Err("console client sent source before the ready handshake".into())
        }
        WaitEvent::Timeout => {
            terminate(child);
            Err("console evaluator startup timed out".into())
        }
        WaitEvent::EvaluatorClosed => {
            terminate(child);
            Err("console evaluator terminated during startup".into())
        }
    }
}

fn validate_eval_request(request: &Value, maximum_source: usize) -> Result<(), String> {
    let entries = core::map_entries(request).ok_or_else(|| {
        "hoplite.console/request-invalid: evaluation request must be a map".to_string()
    })?;
    if entries.len() != 2 {
        return Err(
            "hoplite.console/request-invalid: evaluation request must contain exactly op and source"
                .into(),
        );
    }
    if map_get(request, "op")
        .and_then(|value| string_value(&value).ok())
        .as_deref()
        != Some("eval")
    {
        return Err("hoplite.console/request-invalid: unsupported evaluation operation".into());
    }
    let source = match map_get(request, "source") {
        Some(Value::String(source)) => source,
        _ => return Err("hoplite.console/request-invalid: source must be a string".into()),
    };
    if source.len() > maximum_source {
        return Err("hoplite.console/source-too-large".into());
    }
    Ok(())
}

fn cancel_request(value: &Value) -> bool {
    core::map_entries(value).is_some_and(|entries| entries.len() == 1)
        && map_get(value, "op")
            .and_then(|value| string_value(&value).ok())
            .as_deref()
            == Some("cancel")
}

fn broker_loop(
    mut stream: UnixStream,
    descriptors_hta: Vec<u8>,
    grant_hta: Vec<u8>,
    limits: ConsoleLimits,
    broker: Arc<dyn CommandBroker>,
) -> Result<(), String> {
    let descriptors = hta::decode(&descriptors_hta)
        .map_err(|error| format!("console descriptors are not valid HTA: {error}"))?;
    let commands = CommandSet::parse(descriptors)?;
    let grant_value = hta::decode(&grant_hta)
        .map_err(|error| format!("console grant is not valid HTA: {error}"))?;
    let grant = ConsoleGrant::parse(&grant_value)?;
    commands.validate_grant(&grant)?;
    while let Some(request) = read_hta_frame(&mut stream, limits.result_bytes)? {
        let response = match handle_evaluator_broker_request(
            &commands,
            &grant,
            request,
            limits,
            broker.as_ref(),
        ) {
            Ok(value) => success(value),
            Err(error) => {
                let code = error_code(&error).to_owned();
                failure(&code, error)
            }
        };
        write_hta_frame(&mut stream, &response, limits.result_bytes)?;
    }
    Ok(())
}

fn handle_evaluator_broker_request(
    commands: &CommandSet,
    grant: &ConsoleGrant,
    request: Value,
    limits: ConsoleLimits,
    broker: &dyn CommandBroker,
) -> Result<Value, String> {
    let operation = map_get(&request, "op")
        .ok_or_else(|| "hoplite.console/operation-unlisted".to_string())
        .and_then(|value| string_value(&value))?;
    match operation.as_str() {
        "commands" => {
            let entries = core::map_entries(&request)
                .ok_or_else(|| "hoplite.console/commands-invalid".to_string())?;
            if entries.len() != 1 {
                return Err("hoplite.console/commands-invalid".into());
            }
            Ok(commands.granted_value(grant))
        }
        "call" => {
            let request_entries = core::map_entries(&request)
                .ok_or_else(|| "hoplite.console/request-invalid".to_string())?;
            if request_entries.len() != 2 {
                return Err("hoplite.console/request-invalid".into());
            }
            let client_request = map_get(&request, "request")
                .ok_or_else(|| "hoplite.console/request-invalid".to_string())?;
            let entries = core::map_entries(&client_request)
                .ok_or_else(|| "hoplite.console/request-invalid".to_string())?;
            if entries.len() != 3 {
                return Err("hoplite.console/request-invalid".into());
            }
            let protocol = map_get(&client_request, "protocol")
                .ok_or_else(|| "hoplite.console/request-invalid".to_string())
                .and_then(|value| string_value(&value))?;
            if protocol != REQUEST_PROTOCOL {
                return Err("hoplite.console/request-invalid".into());
            }
            let command = map_get(&client_request, "command")
                .ok_or_else(|| "hoplite.console/request-invalid".to_string())
                .and_then(|value| string_value(&value))?;
            let input = map_get(&client_request, "input")
                .ok_or_else(|| "hoplite.console/request-invalid".to_string())?;
            commands.validate_call(grant, &command, &input, limits.result_bytes)?;
            let value = broker.call(&grant.to_value(), &command, input)?;
            let encoded = hta::encode(&value)
                .map_err(|error| format!("hoplite.console/result-not-immutable: {error}"))?;
            if encoded.len() > limits.result_bytes {
                return Err("hoplite.console/result-too-large".into());
            }
            Ok(value)
        }
        _ => Err("hoplite.console/operation-unlisted".into()),
    }
}

fn spawn_evaluator(
    config: &SupervisorConfig,
    child_evaluation: UnixStream,
    child_broker: UnixStream,
    bundle: File,
) -> Result<Child, String> {
    let evaluation_fd = child_evaluation.as_raw_fd();
    let broker_fd = child_broker.as_raw_fd();
    let bundle_fd = bundle.as_raw_fd();
    let inherited = [evaluation_fd, broker_fd, bundle_fd];
    let limits = config.limits;
    let mut command = Command::new(&config.evaluator_path);
    command
        .arg("--evaluation-fd")
        .arg(evaluation_fd.to_string())
        .arg("--broker-fd")
        .arg(broker_fd.to_string())
        .arg("--bundle-fd")
        .arg(bundle_fd.to_string())
        .arg("--namespace")
        .arg(&config.client_namespace)
        .arg("--source-bytes")
        .arg(limits.source_bytes.to_string())
        .arg("--result-bytes")
        .arg(limits.result_bytes.to_string())
        .arg("--evaluation-millis")
        .arg(limits.evaluation_millis.to_string())
        .arg("--memory-bytes")
        .arg(limits.memory_bytes.to_string())
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || os::configure_child(&inherited, limits));
    }
    let child = command
        .spawn()
        .map_err(|error| format!("cannot spawn console evaluator: {error}"))?;
    drop(child_evaluation);
    drop(child_broker);
    drop(bundle);
    Ok(child)
}

fn bind_private_socket(path: &Path) -> Result<UnixListener, String> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() {
            return Err(format!(
                "refusing to replace non-socket console path {}",
                path.display()
            ));
        }
        fs::remove_file(path)
            .map_err(|error| format!("cannot remove stale console socket: {error}"))?;
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "cannot create console socket directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let listener = UnixListener::bind(path)
        .map_err(|error| format!("cannot bind console socket {}: {error}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("cannot set console socket mode 0600: {error}"))?;
    let mode = fs::metadata(path)
        .map_err(|error| format!("cannot inspect console socket: {error}"))?
        .permissions()
        .mode()
        & 0o777;
    if mode != 0o600 {
        return Err(format!("console socket mode is {mode:o}, expected 600"));
    }
    Ok(listener)
}

fn open_immutable_bundle(path: &Path) -> Result<File, String> {
    let link = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect console bundle {}: {error}", path.display()))?;
    if link.file_type().is_symlink() || !link.file_type().is_file() {
        return Err("console client bundle must be a regular non-symlink file".into());
    }
    if link.mode() & 0o222 != 0 {
        return Err("console client bundle must have no write permission bits".into());
    }
    let owner = os::effective_uid();
    if link.uid() != owner && link.uid() != 0 {
        return Err(format!(
            "console client bundle uid {} is neither supervisor uid {owner} nor root",
            link.uid()
        ));
    }
    let file = File::open(path).map_err(|error| {
        format!(
            "cannot open console client bundle {}: {error}",
            path.display()
        )
    })?;
    let opened = file
        .metadata()
        .map_err(|error| format!("cannot inspect open console client bundle: {error}"))?;
    if opened.dev() != link.dev() || opened.ino() != link.ino() {
        return Err("console client bundle changed while it was opened".into());
    }
    Ok(file)
}

fn response_result(response: Value) -> Result<Value, String> {
    match map_get(&response, "ok") {
        Some(Value::Bool(true)) => map_get(&response, "value")
            .ok_or_else(|| "console response is missing its value".into()),
        Some(Value::Bool(false)) => {
            let error = map_get(&response, "error")
                .ok_or_else(|| "console response is missing its error".to_string())?;
            let code = map_get(&error, "code")
                .and_then(|value| string_value(&value).ok())
                .unwrap_or_else(|| "hoplite.console/call-failed".into());
            let message = map_get(&error, "message")
                .and_then(|value| string_value(&value).ok())
                .unwrap_or_else(|| code.clone());
            Err(format!("{code}: {message}"))
        }
        _ => Err("console response is missing boolean ok".into()),
    }
}

fn error_code(error: &str) -> &str {
    error.split_once(':').map(|(code, _)| code).unwrap_or(error)
}

fn connection_id() -> String {
    format!(
        "console.{}.{}",
        std::process::id(),
        NEXT_CONSOLE.fetch_add(1, Ordering::Relaxed)
    )
}

fn terminate(child: &mut Child) {
    match child.try_wait() {
        Ok(Some(_)) => return,
        Ok(None) | Err(_) => {}
    }
    os::kill_process(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::protocol::GRANT_PROTOCOL;
    use std::collections::BTreeSet;

    struct NeverBroker;

    impl CommandBroker for NeverBroker {
        fn call(&self, _grant: &Value, _command: &str, _input: Value) -> Result<Value, String> {
            panic!("invalid requests must not reach the application broker")
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

    fn grant(write: bool) -> ConsoleGrant {
        ConsoleGrant {
            console: "console.test".into(),
            commands: BTreeSet::from(["status".into(), "quit".into()]),
            write,
        }
    }

    #[test]
    fn supervisor_revalidates_unlisted_and_write_calls() {
        let broker = NeverBroker;
        let unlisted = map_value(vec![
            ("op", Value::String("call".into())),
            (
                "request",
                map_value(vec![
                    ("protocol", Value::String(REQUEST_PROTOCOL.into())),
                    ("command", Value::String("runtime.eval".into())),
                    ("input", map_value(vec![])),
                ]),
            ),
        ]);
        assert_eq!(
            handle_evaluator_broker_request(
                &commands(),
                &grant(false),
                unlisted,
                ConsoleLimits::default(),
                &broker,
            )
            .unwrap_err(),
            "hoplite.console/command-unlisted"
        );
        let write = map_value(vec![
            ("op", Value::String("call".into())),
            (
                "request",
                map_value(vec![
                    ("protocol", Value::String(REQUEST_PROTOCOL.into())),
                    ("command", Value::String("quit".into())),
                    ("input", map_value(vec![])),
                ]),
            ),
        ]);
        assert_eq!(
            handle_evaluator_broker_request(
                &commands(),
                &grant(false),
                write,
                ConsoleLimits::default(),
                &broker,
            )
            .unwrap_err(),
            "hoplite.console/write-not-granted"
        );
    }

    #[test]
    fn grant_protocol_is_not_caller_controlled() {
        let value = grant(false).to_value();
        assert_eq!(
            map_get(&value, "protocol"),
            Some(Value::String(GRANT_PROTOCOL.into()))
        );
        assert!(map_get(&value, "console").is_some());
    }

    #[test]
    fn evaluation_requests_are_exact_and_single_operation() {
        assert!(validate_eval_request(
            &map_value(vec![
                ("op", Value::String("eval".into())),
                ("source", Value::String("(+ 1 2)".into())),
            ]),
            64 * 1024,
        )
        .is_ok());
        assert!(validate_eval_request(
            &map_value(vec![
                ("op", Value::String("eval".into())),
                ("source", Value::String("(+ 1 2)".into())),
                ("handler", Value::String("tahto.node.app/handler".into())),
            ]),
            64 * 1024,
        )
        .is_err());
    }
}
