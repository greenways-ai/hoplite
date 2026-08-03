use hara_wasm::{vm, Runtime};
use std::hint::black_box;
use std::rc::Rc;
use std::time::Instant;

const CORE: &str = include_str!("../../lib/src/hoplite/core.hal");
const INTERNAL: &str = include_str!("../../lib/src/hoplite/internal.hal");
const DEV: &str = include_str!("../../lib/src/hoplite/dev.hal");

fn body(source: &str) -> String {
    let (_, forms) = source
        .split_once("\n\n")
        .expect("Hoplite library namespace followed by forms");
    format!("(do {forms})")
}

fn percentile(samples: &mut [u128], numerator: usize, denominator: usize) -> u128 {
    samples.sort_unstable();
    samples[(samples.len() - 1) * numerator / denominator]
}

fn measure(name: &str, source: &str, iterations: usize) -> Result<(), String> {
    let source = body(source);
    let compiler = Runtime::new();
    let artifact = compiler.compile_bytecode_artifact(&source)?;
    let decoded = Rc::new(vm::decode_program(&artifact)?);
    let mut source_runtime = Runtime::new();
    let mut artifact_runtime = Runtime::new();
    let mut decoded_runtime = Runtime::new();
    let mut source_samples = Vec::with_capacity(iterations);
    let mut artifact_samples = Vec::with_capacity(iterations);
    let mut decoded_samples = Vec::with_capacity(iterations);
    let mut compile_samples = Vec::with_capacity(iterations);
    let mut decode_samples = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let started = Instant::now();
        black_box(compiler.compile_bytecode(&source)?);
        compile_samples.push(started.elapsed().as_nanos());

        let started = Instant::now();
        black_box(vm::decode_program(&artifact)?);
        decode_samples.push(started.elapsed().as_nanos());

        let started = Instant::now();
        let program = source_runtime.compile_bytecode(&source)?;
        black_box(source_runtime.execute_compiled_bytecode_value(program)?);
        source_samples.push(started.elapsed().as_nanos());

        let started = Instant::now();
        black_box(artifact_runtime.eval_bytecode_artifact(&artifact)?);
        artifact_samples.push(started.elapsed().as_nanos());

        let started = Instant::now();
        black_box(decoded_runtime.execute_compiled_bytecode_value(decoded.clone())?);
        decoded_samples.push(started.elapsed().as_nanos());
    }

    let source_median = percentile(&mut source_samples, 1, 2);
    let artifact_median = percentile(&mut artifact_samples, 1, 2);
    let decoded_median = percentile(&mut decoded_samples, 1, 2);
    let compile_median = percentile(&mut compile_samples, 1, 2);
    let decode_median = percentile(&mut decode_samples, 1, 2);
    println!(
        "{{\"library\":\"{name}\",\"source_bytes\":{},\"artifact_bytes\":{},\"iterations\":{iterations},\"compile_only_median_ns\":{compile_median},\"decode_only_median_ns\":{decode_median},\"decode_speedup\":{:.3},\"source_compile_execute_median_ns\":{source_median},\"artifact_decode_execute_median_ns\":{artifact_median},\"decoded_execute_median_ns\":{decoded_median},\"artifact_speedup\":{:.3},\"decoded_speedup\":{:.3}}}",
        source.len(),
        artifact.len(),
        compile_median as f64 / decode_median as f64,
        source_median as f64 / artifact_median as f64,
        source_median as f64 / decoded_median as f64,
    );
    Ok(())
}

fn main() -> Result<(), String> {
    let iterations = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_000);
    measure("hoplite.core", CORE, iterations)?;
    measure("hoplite.internal", INTERNAL, iterations)?;
    measure("hoplite.dev", DEV, iterations)?;
    Ok(())
}
