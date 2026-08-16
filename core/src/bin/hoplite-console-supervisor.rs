#[cfg(unix)]
use hoplite::console::protocol::ConsoleLimits;
#[cfg(unix)]
use hoplite::console::supervisor::{run_supervisor, SupervisorConfig, UnixCommandBroker};
#[cfg(unix)]
use std::collections::BTreeMap;
#[cfg(unix)]
use std::env;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
fn main() {
    if let Err(error) = run() {
        eprintln!("hoplite-console-supervisor: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("hoplite-console-supervisor: the separate-process console requires Unix");
    std::process::exit(1);
}

#[cfg(unix)]
fn run() -> Result<(), String> {
    let values = arguments()?;
    let defaults = ConsoleLimits::default();
    let limits = ConsoleLimits {
        source_bytes: optional_number(&values, "--source-bytes")?.unwrap_or(defaults.source_bytes),
        result_bytes: optional_number(&values, "--result-bytes")?.unwrap_or(defaults.result_bytes),
        evaluation_millis: optional_number(&values, "--evaluation-millis")?
            .unwrap_or(defaults.evaluation_millis),
        memory_bytes: optional_number(&values, "--memory-bytes")?.unwrap_or(defaults.memory_bytes),
    }
    .validate()?;
    let evaluator_path = values
        .get("--evaluator")
        .map(PathBuf::from)
        .unwrap_or(sibling_evaluator()?);
    let config = SupervisorConfig::from_files(
        path(&values, "--socket")?,
        evaluator_path,
        path(&values, "--bundle")?,
        required(&values, "--namespace")?.to_owned(),
        &path(&values, "--descriptors")?,
        &path(&values, "--grant")?,
        limits,
    )?;
    let broker = UnixCommandBroker {
        socket_path: path(&values, "--broker-socket")?,
        maximum_bytes: limits.result_bytes,
        timeout: Duration::from_millis(limits.evaluation_millis),
    };
    run_supervisor(config, Arc::new(broker))
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
            "--socket"
                | "--evaluator"
                | "--bundle"
                | "--namespace"
                | "--descriptors"
                | "--grant"
                | "--broker-socket"
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
fn path(values: &BTreeMap<String, String>, flag: &str) -> Result<PathBuf, String> {
    Ok(PathBuf::from(required(values, flag)?))
}

#[cfg(unix)]
fn optional_number<T>(
    values: &BTreeMap<String, String>,
    flag: &str,
) -> Result<Option<T>, String>
where
    T: std::str::FromStr,
{
    values
        .get(flag)
        .map(|value| {
            value
                .parse::<T>()
                .map_err(|_| format!("{flag} must be an unsigned integer"))
        })
        .transpose()
}

#[cfg(unix)]
fn sibling_evaluator() -> Result<PathBuf, String> {
    let current = env::current_exe()
        .map_err(|error| format!("cannot locate the supervisor executable: {error}"))?;
    Ok(current.with_file_name("hoplite-console-evaluator"))
}

#[cfg(unix)]
fn usage() {
    println!("Run the separate-process Hoplite application console supervisor");
    println!();
    println!("usage: hoplite-console-supervisor \\");
    println!("  --socket PATH --bundle PATH --namespace NAME \\");
    println!("  --descriptors PATH --grant PATH --broker-socket PATH [OPTIONS]");
    println!();
    println!("The public socket is created mode 0600 and accepts only the supervisor's OS user.");
    println!("Each connection receives one fresh evaluator process and one private command grant.");
}
