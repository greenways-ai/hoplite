use hara_native::core::Value;
use hara_native::kernel::{parse_forms, Form};
use hara_native::vm::{self, BytecodeBundleModule};
use hara_native::{hta, Runtime as CompilerRuntime};
use hoplite_runtime::{
    hoplite_bootstrap_bytecode, hoplite_buffer_free, hoplite_call_resolve, hoplite_handler_prepare,
    hoplite_runtime_free, hoplite_runtime_new, hoplite_work_call, hoplite_work_close,
    hoplite_work_next_event, hoplite_work_poll, HopliteBuffer, HopliteRuntime,
};
use std::ptr;
use std::slice;

const SOCKET_SOURCE: &str = include_str!("../../lib/src/hoplite/socket.hal");
const APPLICATION_SOURCE: &str = r#"
(ns example.socket-application
  (:require [hoplite.socket :as socket]))

(defn ^:async show
  [_request]
  (let [client
        (std.foundation.coroutine/await
         (socket/tcp))
        configured
        (std.foundation.coroutine/await
         (socket/settimeouts client 1000 1000 1000))
        connected
        (std.foundation.coroutine/await
         (socket/connect client "127.0.0.1" 19091))
        optioned
        (std.foundation.coroutine/await
         (socket/setoption client "tcp-nodelay" true))
        received
        (std.foundation.coroutine/await
         (socket/receiveany client 4))
        reader
        (std.foundation.coroutine/await
         (socket/receiveuntil client "--end--" {:inclusive true}))
        delimited
        (std.foundation.coroutine/await
         (reader 4))
        shutdown
        (std.foundation.coroutine/await
         (socket/shutdown client "send"))
        unix-client
        (std.foundation.coroutine/await
         (socket/tcp))
        unix-connected
        (std.foundation.coroutine/await
         (socket/connect unix-client "unix:/tmp/hoplite.sock"))
        pooled-client
        (std.foundation.coroutine/await
         (socket/tcp))
        pooled-connected
        (std.foundation.coroutine/await
         (socket/connect
          pooled-client
          "127.0.0.1"
          19091
          {:pool "source-free" :pool-size 2 :backlog 2}))
        reused
        (std.foundation.coroutine/await
         (socket/getreusedtimes pooled-client))
        returned
        (std.foundation.coroutine/await
         (socket/setkeepalive pooled-client 1000 2))]
    {:client client
     :configured configured
     :connected connected
     :optioned optioned
     :received received
     :delimited delimited
     :shutdown shutdown
     :unix-client unix-client
     :unix-connected unix-connected
     :pooled-client pooled-client
     :pooled-connected pooled-connected
     :reused reused
     :returned returned}))
"#;

struct RuntimeGuard(*mut HopliteRuntime);

impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        unsafe { hoplite_runtime_free(self.0) };
    }
}

fn render_sequence(values: &[Form], prefix: &str, suffix: &str) -> String {
    format!(
        "{prefix}{}{suffix}",
        values.iter().map(render_form).collect::<Vec<_>>().join(" ")
    )
}

fn render_form(form: &Form) -> String {
    match form {
        Form::Metadata(metadata, value) => {
            format!("^{} {}", render_form(metadata), render_form(value))
        }
        Form::Tagged(tag, value) => format!("#{tag}{}", render_form(value)),
        Form::List(values) => render_sequence(values, "(", ")"),
        Form::Vector(values) => render_sequence(values, "[", "]"),
        Form::Set(values) => render_sequence(values, "#{", "}"),
        Form::Map(entries) => {
            let values = entries
                .iter()
                .flat_map(|(key, value)| [render_form(key), render_form(value)])
                .collect::<Vec<_>>();
            format!("{{{}}}", values.join(" "))
        }
        _ => form.to_string(),
    }
}

fn compile_module(
    runtime: &mut CompilerRuntime,
    namespace: &str,
    source: &str,
    dependencies: Vec<String>,
) -> BytecodeBundleModule {
    let forms = parse_forms(source).expect("module parses");
    let declaration = forms
        .iter()
        .find(|form| {
            matches!(form, Form::List(items)
                if matches!(items.first(), Some(Form::Symbol(operator)) if operator == "ns"))
        })
        .expect("module declares an ns form");
    let declaration = render_form(declaration);
    runtime
        .eval_native(&declaration)
        .unwrap_or_else(|error| panic!("{namespace} declaration evaluates: {error}"));
    let body = forms
        .into_iter()
        .filter(|form| {
            !matches!(form, Form::List(items)
                if matches!(items.first(), Some(Form::Symbol(operator)) if operator == "ns"))
        })
        .map(|form| render_form(&form))
        .collect::<Vec<_>>()
        .join("\n");
    let artifact = runtime
        .compile_bytecode_artifact(&body)
        .unwrap_or_else(|error| panic!("{namespace} compiles: {error}"));
    runtime
        .eval_bytecode_artifact(&artifact)
        .unwrap_or_else(|error| panic!("{namespace} bytecode evaluates: {error}"));
    BytecodeBundleModule {
        resource: namespace.into(),
        namespace_form: declaration,
        source_digest: [0; 32],
        dependencies,
        eager: true,
        artifact,
    }
}

fn source_free_bundle() -> Vec<u8> {
    let mut compiler = hoplite::hara_source::compiler_runtime().unwrap();
    compiler.register_resource("hoplite.socket", SOCKET_SOURCE);
    compiler.register_resource("example.socket-application", APPLICATION_SOURCE);
    let socket = compile_module(&mut compiler, "hoplite.socket", SOCKET_SOURCE, Vec::new());
    let application = compile_module(
        &mut compiler,
        "example.socket-application",
        APPLICATION_SOURCE,
        vec!["hoplite.socket".into()],
    );
    vm::encode_bytecode_bundle(&[socket, application]).expect("HBX0 bundle encodes")
}

fn next_event(runtime: *mut HopliteRuntime) -> Value {
    unsafe {
        assert!(
            hoplite_work_poll(runtime) > 0,
            "runtime has a pending event"
        );
        let mut output = HopliteBuffer {
            data: ptr::null_mut(),
            len: 0,
        };
        assert_eq!(hoplite_work_next_event(runtime, &mut output), 0);
        let bytes = slice::from_raw_parts(output.data, output.len).to_vec();
        hoplite_buffer_free(output.data, output.len);
        hta::decode(&bytes).expect("runtime event decodes")
    }
}

fn host_call(event: &Value, service: &str, operation: &str) -> u64 {
    let Value::Vector(values) = event else {
        panic!("expected host-call vector, received {}", event.display())
    };
    assert!(
        matches!(values.get(0), Some(Value::Number(2))),
        "expected host call, received {}",
        event.display()
    );
    assert!(
        matches!(values.get(5), Some(Value::String(value)) if value == service),
        "expected service {service}, received {}",
        event.display()
    );
    assert!(
        matches!(values.get(6), Some(Value::String(value)) if value == operation),
        "expected operation {operation}, received {}",
        event.display()
    );
    match values.get(1) {
        Some(Value::Number(value)) if *value > 0 => *value as u64,
        _ => panic!("host call omitted its id: {}", event.display()),
    }
}

fn resolve(runtime: *mut HopliteRuntime, call: u64, value: Value) {
    let payload = hta::encode(&value).expect("completion encodes");
    assert_eq!(
        unsafe { hoplite_call_resolve(runtime, call, payload.as_ptr(), payload.len()) },
        0,
        "host completion is accepted"
    );
}

#[test]
fn source_free_socket_coroutine_continues_after_synchronous_completions() {
    let bundle = source_free_bundle();
    assert_eq!(&bundle[..4], b"HBX0");

    let runtime = RuntimeGuard(hoplite_runtime_new());
    assert!(!runtime.0.is_null(), "runtime allocates");
    assert_eq!(
        unsafe { hoplite_bootstrap_bytecode(runtime.0, bundle.as_ptr(), bundle.len()) },
        0,
        "source-free bundle loads"
    );

    let handler_name = b"example.socket-application/show";
    let handler =
        unsafe { hoplite_handler_prepare(runtime.0, handler_name.as_ptr(), handler_name.len()) };
    assert_ne!(handler, 0, "handler prepares");

    let input = hta::encode(&Value::Map(Default::default())).expect("request input encodes");
    let work = unsafe { hoplite_work_call(runtime.0, handler, input.as_ptr(), input.len()) };
    assert_ne!(work, 0, "handler starts");

    let tcp = next_event(runtime.0);
    let tcp_call = host_call(&tcp, "hoplite.socket", "tcp");
    resolve(runtime.0, tcp_call, Value::Number(1));

    let settimeouts = next_event(runtime.0);
    let settimeouts_call = host_call(&settimeouts, "hoplite.socket", "settimeouts");
    resolve(
        runtime.0,
        settimeouts_call,
        Value::Vector(vec![Value::Number(1), Value::Nil].into()),
    );

    let connect = next_event(runtime.0);
    let connect_call = host_call(&connect, "hoplite.socket", "connect");
    resolve(
        runtime.0,
        connect_call,
        Value::Vector(vec![Value::Number(1), Value::Nil].into()),
    );

    let setoption = next_event(runtime.0);
    let setoption_call = host_call(&setoption, "hoplite.socket", "setoption");
    resolve(
        runtime.0,
        setoption_call,
        Value::Vector(vec![Value::Number(1), Value::Nil].into()),
    );

    let receiveany = next_event(runtime.0);
    let receiveany_call = host_call(&receiveany, "hoplite.socket", "receiveany");
    resolve(
        runtime.0,
        receiveany_call,
        Value::Vector(vec![Value::Bytes(b"part".to_vec()), Value::Nil, Value::Nil].into()),
    );

    let receiveuntil = next_event(runtime.0);
    let receiveuntil_call = host_call(&receiveuntil, "hoplite.socket", "receiveuntil");
    resolve(runtime.0, receiveuntil_call, Value::Number(2));

    let reader = next_event(runtime.0);
    let reader_call = host_call(&reader, "hoplite.socket", "receiveuntil-read");
    resolve(
        runtime.0,
        reader_call,
        Value::Vector(vec![Value::Bytes(b"part".to_vec()), Value::Nil, Value::Nil].into()),
    );

    let shutdown = next_event(runtime.0);
    let shutdown_call = host_call(&shutdown, "hoplite.socket", "shutdown");
    resolve(
        runtime.0,
        shutdown_call,
        Value::Vector(vec![Value::Number(1), Value::Nil].into()),
    );

    let unix_tcp = next_event(runtime.0);
    let unix_tcp_call = host_call(&unix_tcp, "hoplite.socket", "tcp");
    resolve(runtime.0, unix_tcp_call, Value::Number(2));

    let unix_connect = next_event(runtime.0);
    let unix_connect_call = host_call(&unix_connect, "hoplite.socket", "connect");
    resolve(
        runtime.0,
        unix_connect_call,
        Value::Vector(vec![Value::Number(1), Value::Nil].into()),
    );

    let pooled_tcp = next_event(runtime.0);
    let pooled_tcp_call = host_call(&pooled_tcp, "hoplite.socket", "tcp");
    resolve(runtime.0, pooled_tcp_call, Value::Number(3));

    let pooled_connect = next_event(runtime.0);
    let pooled_connect_call = host_call(&pooled_connect, "hoplite.socket", "connect");
    resolve(
        runtime.0,
        pooled_connect_call,
        Value::Vector(vec![Value::Number(1), Value::Nil].into()),
    );

    let reused = next_event(runtime.0);
    let reused_call = host_call(&reused, "hoplite.socket", "getreusedtimes");
    resolve(
        runtime.0,
        reused_call,
        Value::Vector(vec![Value::Number(1), Value::Nil].into()),
    );

    let setkeepalive = next_event(runtime.0);
    let setkeepalive_call = host_call(&setkeepalive, "hoplite.socket", "setkeepalive");
    resolve(
        runtime.0,
        setkeepalive_call,
        Value::Vector(vec![Value::Number(1), Value::Nil].into()),
    );

    let completed = next_event(runtime.0);
    let Value::Vector(values) = &completed else {
        panic!(
            "expected completion vector, received {}",
            completed.display()
        )
    };
    assert!(
        matches!(values.get(0), Some(Value::Number(0))),
        "source-free socket handler failed after completion: {}",
        completed.display()
    );
    assert!(
        matches!(values.get(1), Some(Value::Number(value)) if *value == work as i64),
        "completion belongs to its work: {}",
        completed.display()
    );
    assert_eq!(unsafe { hoplite_work_close(runtime.0, work) }, 0);
}
