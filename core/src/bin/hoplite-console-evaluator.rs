#[cfg(unix)]
use hoplite::console::evaluator::{run_evaluator, EvaluatorConfig};
#[cfg(unix)]
use hoplite::console::protocol::ConsoleLimits;
#[cfg(unix)]
use std::collections::BTreeMap;
#[cfg(unix)]
use std::env;
#[cfg(unix)]
use std::os::fd::RawFd;

#[cfg(unix)]
fn main() {
    if let Err(error) = run() {
        eprintln!("hoplite-console-evaluator: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("hoplite-console-evaluator: the separate-process console requires Unix");
    std::process::exit(1);
}

#[cfg(unix)]
fn run() -> Result<(), String> {
    let values = arguments()?;
    let limits = ConsoleLimits {
        source_bytes: number(&values, "--source-bytes")?,
        result_bytes: number(&values, "--result-bytes")?,
        evaluation_millis: number(&values, "--evaluation-millis")?,
        memory_bytes: number(&values, "--memory-bytes")?,
    }
    .validate()?;
    let config = EvaluatorConfig {
        evaluation_fd: file_descriptor(&values, "--evaluation-fd")?,
        broker_fd: file_descriptor(&values, "--broker-fd")?,
        bundle_fd: file_descriptor(&values, "--bundle-fd")?,
        namespace: required(&values, "--namespace")?.to_owned(),
        limits,
    };
    run_evaluator(config)
}

#[cfg(unix)]
fn arguments() -> Result<BTreeMap<String, String>, String> {
    let mut arguments = env::args().skip(1);
    let mut values = BTreeMap::new();
    while let Some(flag) = arguments.next() {
        if matches!(flag.as_str(), "--help" | "-h") {
            usage();
            std::process::exit(0);
        }
        if !matches!(
            flag.as_str(),
            "--evaluation-fd"
                | "--broker-fd"
                | "--bundle-fd"
                | "--namespace"
                | "--source-bytes"
                | "--result-bytes"
                | "--evaluation-millis"
                | "--memory-bytes"
        ) {
            return Err(format!("unknown argument {flag:?}"));
        }
        let value = arguments
            .next()
            .ok_or_else(|| format!("{flag} requires a value"))?;
        if values.insert(flag.clone(), value).is_some() {
            return Err(format!("duplicate argument {flag}"));
        }
    }
    Ok(values)
}

#[cfg(unix)]
fn required<'a>(values: &'a BTreeMap<String, String>, flag: &str) -> Result<&'a str, String> {
    values
        .get(flag)
        .map(String::as_str)
        .ok_or_else(|| format!("missing required argument {flag}"))
}

#[cfg(unix)]
fn number<T>(values: &BTreeMap<String, String>, flag: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    required(values, flag)?
        .parse::<T>()
        .map_err(|_| format!("{flag} must be an unsigned integer"))
}

#[cfg(unix)]
fn file_descriptor(values: &BTreeMap<String, String>, flag: &str) -> Result<RawFd, String> {
    let descriptor = required(values, flag)?
        .parse::<RawFd>()
        .map_err(|_| format!("{flag} must be a file descriptor"))?;
    if descriptor < 0 {
        return Err(format!("{flag} must be non-negative"));
    }
    Ok(descriptor)
}

#[cfg(unix)]
fn usage() {
    println!("Internal Hoplite application-console evaluator");
    println!();
    println!("This executable is spawned by the console supervisor and accepts only");
    println!("pre-opened evaluator, broker, and immutable-bundle file descriptors.");
}
