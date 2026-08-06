use hara_wasm::{core::Value, hta};
use hoplite_runtime::{
    hoplite_buffer_free, hoplite_handler_prepare, hoplite_runtime_free, hoplite_runtime_new,
    hoplite_work_call, hoplite_work_close, hoplite_work_next_event, hoplite_work_poll,
    hoplite_work_start, HopliteBuffer,
};
use std::hint::black_box;
use std::time::Instant;

fn start(runtime: *mut hoplite_runtime::HopliteRuntime, source: &[u8], input: &[u8]) -> u64 {
    unsafe {
        hoplite_work_start(
            runtime,
            source.as_ptr(),
            source.len(),
            input.as_ptr(),
            input.len(),
        )
    }
}

fn drain(runtime: *mut hoplite_runtime::HopliteRuntime) {
    while unsafe { hoplite_work_poll(runtime) } != 0 {
        let mut output = HopliteBuffer {
            data: std::ptr::null_mut(),
            len: 0,
        };
        assert_eq!(unsafe { hoplite_work_next_event(runtime, &mut output) }, 0);
        let bytes = unsafe { std::slice::from_raw_parts(output.data, output.len) };
        let _ = black_box(hta::decode(bytes).expect("event HTA"));
        unsafe { hoplite_buffer_free(output.data, output.len) };
    }
}

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000usize);
    let runtime = hoplite_runtime_new();
    assert!(!runtime.is_null());
    let bootstrap = b"(do (defn bench-handler [request] {:status 200 :headers {} :body (get request :path)}) nil)";
    let boot = start(runtime, bootstrap, &[]);
    drain(runtime);
    unsafe { hoplite_work_close(runtime, boot) };

    let symbol = b"bench-handler";
    let prepared_started = Instant::now();
    let handler = unsafe { hoplite_handler_prepare(runtime, symbol.as_ptr(), symbol.len()) };
    let prepare_ns = prepared_started.elapsed().as_nanos();
    assert_ne!(handler, 0);
    let request = hta::encode(&Value::Map(
        [(
            Value::Keyword("path".into()),
            Value::String("/bench".into()),
        )]
        .into_iter()
        .collect(),
    ))
    .expect("request HTA");
    let source = b"(bench-handler __hoplite_request)";

    let started = Instant::now();
    for _ in 0..iterations {
        let work = start(runtime, source, &request);
        drain(runtime);
        unsafe { hoplite_work_close(runtime, work) };
    }
    let source_ns = started.elapsed().as_nanos() / iterations as u128;

    let started = Instant::now();
    for _ in 0..iterations {
        let work = unsafe { hoplite_work_call(runtime, handler, request.as_ptr(), request.len()) };
        drain(runtime);
        unsafe { hoplite_work_close(runtime, work) };
    }
    let cached_ns = started.elapsed().as_nanos() / iterations as u128;
    println!(
        "{{\"boundary\":\"hoplite-native-abi\",\"prepare_ns\":{},\"source_ns\":{},\"cached_ns\":{},\"speedup\":{:.3},\"request_bytes\":{},\"iterations\":{}}}",
        prepare_ns, source_ns, cached_ns, source_ns as f64 / cached_ns as f64,
        request.len(), iterations
    );
    unsafe { hoplite_runtime_free(runtime) };
}
