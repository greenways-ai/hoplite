use hara_wasm::core::{self, Value};
use hara_wasm::project;
use hara_wasm::Runtime;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

pub const SOURCE: &str = include_str!("../lib/src/hoplite/dev.hal");

#[derive(Clone, Debug)]
struct ServerRecord {
    app: String,
    project: PathBuf,
    profile: Option<String>,
    production: bool,
}

#[derive(Default)]
struct Console {
    servers: BTreeMap<String, ServerRecord>,
}

pub fn install(runtime: &mut Runtime) {
    runtime.register_resource("hoplite.core", super::app::CORE_SOURCE);
    runtime.register_resource("hoplite.host", super::app::HOST_SOURCE);
    runtime.register_resource("hoplite.internal", super::app::INTERNAL_SOURCE);
    runtime.register_resource("hoplite.dev", SOURCE);
    if let Ok(directory) = env::current_dir() {
        if let Ok(project) = project::discover(&directory) {
            let _ = project::register_sources(&project, runtime);
        }
    }
    let console = Rc::new(RefCell::new(Console::default()));
    runtime.install_native_host_handler(Rc::new(move |service, method, args| {
        if service == "hoplite.host" {
            return super::host::dispatch(service, method, args);
        }
        if service != "hoplite.dev" {
            return Err(format!("unknown Hoplite console service {service:?}"));
        }
        dispatch(&mut console.borrow_mut(), &method, args)
    }));
}

fn dispatch(console: &mut Console, method: &str, args: Vec<Value>) -> Result<Value, String> {
    match method {
        "start" => {
            let (app, options) = app_options(&args)?;
            start(console, app, options, false)
        }
        "restart" => {
            let (app, options) = app_options(&args)?;
            start(console, app, options, true)
        }
        "stop" => stop(
            console,
            app_name(args.first().ok_or("dev/stop requires an app Var")?)?,
        ),
        "status" => status(
            console,
            app_name(args.first().ok_or("dev/status requires an app Var")?)?,
        ),
        "list-all" if args.is_empty() => Ok(Value::Vector(
            console.servers.values().map(record_value).collect(),
        )),
        "logs" => {
            let (app, options) = app_options(&args)?;
            logs(console, &app, &options)
        }
        _ => Err(format!("unknown hoplite.dev operation {method:?}")),
    }
}

fn start(
    console: &mut Console,
    app: String,
    options: Value,
    restart: bool,
) -> Result<Value, String> {
    let project_root = option_text(&options, "project")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir().map_err(super::io)?);
    let profile = option_text(&options, "profile");
    let production = option_text(&options, "mode").as_deref() == Some("prod");
    let project = project::discover(&project_root)?;
    let selected = project
        .resolve_profile(profile.as_deref())?
        .ok_or("dev/start requires a Hoplite project profile")?;
    if selected.main != app {
        return Err(format!(
            "app Var #{app} is not the selected profile main #{}",
            selected.main
        ));
    }
    if let Some(existing) = console.servers.get(&app) {
        if !restart && running(&existing.project) {
            return Err(format!("{app} is already running; use hoplite.dev/restart"));
        }
        if running(&existing.project) {
            let _ = super::signal(&existing.project, "quit");
        }
    }
    let settings = super::BuildSettings {
        profile: profile.clone(),
        production,
    };
    super::serve(&project.root, &settings)?;
    let record = ServerRecord {
        app: app.clone(),
        project: project.root,
        profile,
        production,
    };
    console.servers.insert(app, record.clone());
    Ok(record_value(&record))
}

fn stop(console: &mut Console, app: String) -> Result<Value, String> {
    let record = console
        .servers
        .get(&app)
        .cloned()
        .ok_or_else(|| format!("{app} is not managed by this console"))?;
    if running(&record.project) {
        super::signal(&record.project, "quit")?;
    }
    console.servers.remove(&app);
    Ok(record_value_with_status(&record, false))
}

fn status(console: &Console, app: String) -> Result<Value, String> {
    let record = console
        .servers
        .get(&app)
        .ok_or_else(|| format!("{app} is not managed by this console"))?;
    Ok(record_value(record))
}

fn logs(console: &Console, app: &str, options: &Value) -> Result<Value, String> {
    let record = console
        .servers
        .get(app)
        .ok_or_else(|| format!("{app} is not managed by this console"))?;
    let limit = option_number(options, "bytes").unwrap_or(16_384).max(0) as usize;
    let path = record.project.join(".hoplite/error.log");
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let start = bytes.len().saturating_sub(limit);
    Ok(Value::String(
        String::from_utf8_lossy(&bytes[start..]).into_owned(),
    ))
}

fn app_options(args: &[Value]) -> Result<(String, Value), String> {
    let app = app_name(args.first().ok_or("operation requires an app Var")?)?;
    let options = args.get(1).cloned().unwrap_or_else(empty_map);
    if core::map_entries(&options).is_none() {
        return Err("development options must be a map".into());
    }
    Ok((app, options))
}

fn app_name(value: &Value) -> Result<String, String> {
    match value {
        Value::Var(var) => Ok(var.symbol().as_str().to_owned()),
        _ => Err("development servers must be identified by an app Var such as #'app/app".into()),
    }
}

fn record_value(record: &ServerRecord) -> Value {
    record_value_with_status(record, running(&record.project))
}

fn record_value_with_status(record: &ServerRecord, is_running: bool) -> Value {
    map(vec![
        ("app", Value::Symbol(record.app.clone().into())),
        (
            "project",
            Value::String(record.project.display().to_string()),
        ),
        (
            "profile",
            record
                .profile
                .as_ref()
                .map(|value| Value::Keyword(value.clone().into()))
                .unwrap_or(Value::Nil),
        ),
        (
            "mode",
            Value::Keyword(if record.production { "prod" } else { "dev" }.into()),
        ),
        (
            "status",
            Value::Keyword(if is_running { "running" } else { "stopped" }.into()),
        ),
    ])
}

fn running(root: &Path) -> bool {
    let Ok(pid) = fs::read_to_string(root.join(".hoplite/nginx.pid")) else {
        return false;
    };
    Command::new("kill")
        .args(["-0", pid.trim()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn option(value: &Value, name: &str) -> Option<Value> {
    core::map_entries(value)?
        .into_iter()
        .find_map(|(key, value)| {
            matches!(&key, Value::Keyword(keyword) if keyword.as_str() == name).then_some(value)
        })
}

fn option_text(value: &Value, name: &str) -> Option<String> {
    match option(value, name)? {
        Value::String(value) => Some(value),
        Value::Keyword(value) => Some(value.as_str().to_owned()),
        Value::Symbol(value) => Some(value.as_str().to_owned()),
        _ => None,
    }
}

fn option_number(value: &Value, name: &str) -> Option<i64> {
    match option(value, name)? {
        Value::Number(value) => Some(value),
        _ => None,
    }
}

fn empty_map() -> Value {
    Value::Map(Default::default())
}

fn map(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (Value::Keyword(key.into()), value))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_all_reports_only_console_managed_apps() {
        let mut console = Console::default();
        console.servers.insert(
            "sample/app".into(),
            ServerRecord {
                app: "sample/app".into(),
                project: PathBuf::from("/definitely/not/running"),
                profile: Some("server".into()),
                production: false,
            },
        );
        let Value::Vector(values) = dispatch(&mut console, "list-all", vec![]).unwrap() else {
            panic!("list-all vector")
        };
        assert_eq!(values.len(), 1);
        assert!(
            matches!(option(&values[0], "status"), Some(Value::Keyword(value)) if value.as_str() == "stopped")
        );
    }

    #[test]
    fn dev_namespace_calls_the_native_console_service() {
        let mut runtime = Runtime::new();
        install(&mut runtime);
        let value = runtime
            .eval_native_value("(ns console-test (:require [hoplite.dev :as dev])) (dev/list-all)")
            .unwrap();
        assert!(
            matches!(&value, Value::Vector(values) if values.is_empty())
                || matches!(&value, Value::Tuple(values) if values.is_empty()),
            "list-all value={value:?}"
        );
    }
}
