#[cfg(unix)]
use hara_native::kernel::{parse_forms, Form};
#[cfg(unix)]
use hoplite::console::protocol::ClientBundle;
#[cfg(unix)]
use std::collections::BTreeMap;
#[cfg(unix)]
use std::env;
#[cfg(unix)]
use std::fs::{self, OpenOptions};
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::path::{Path, PathBuf};

#[cfg(unix)]
fn main() {
    if let Err(error) = run() {
        eprintln!("hoplite-console-bundle: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("hoplite-console-bundle: the separate-process console requires Unix");
    std::process::exit(1);
}

#[cfg(unix)]
fn run() -> Result<(), String> {
    let values = arguments()?;
    let namespace = required(&values, "--namespace")?.to_owned();
    let source_path = path(&values, "--source")?;
    let output_path = path(&values, "--output")?;
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("cannot read {}: {error}", source_path.display()))?;
    validate_declared_namespace(&source, &namespace)?;
    let encoded = ClientBundle::new(namespace.clone(), source)?.encode()?;
    write_immutable(&output_path, &encoded)?;
    println!(
        "wrote {} for {} ({} bytes)",
        output_path.display(),
        namespace,
        encoded.len()
    );
    Ok(())
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
        if !matches!(flag.as_str(), "--namespace" | "--source" | "--output") {
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
fn validate_declared_namespace(source: &str, expected: &str) -> Result<(), String> {
    let declared = parse_forms(source)?.into_iter().find_map(|form| match form {
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

#[cfg(unix)]
fn write_immutable(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "cannot create output directory {}: {error}",
                    parent.display()
                )
            })?;
        }
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o444)
        .open(path)
        .map_err(|error| {
            format!(
                "cannot create immutable console bundle {}: {error}",
                path.display()
            )
        })?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

#[cfg(unix)]
fn usage() {
    println!("Build one immutable HCB0 application-console client bundle");
    println!();
    println!("usage: hoplite-console-bundle --namespace NAME --source FILE --output FILE");
    println!();
    println!("The output is created with mode 0444 and is never overwritten in place.");
}
