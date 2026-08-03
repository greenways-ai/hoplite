use hara_wasm::core::{self, Value};
use hara_wasm::project::{self, Project};
use hara_wasm::Runtime;
use serde_json::{json, Map as JsonMap, Value as JsonValue};

pub const CORE_SOURCE: &str = include_str!("../lib/src/hoplite/core.hal");
pub const INTERNAL_SOURCE: &str = include_str!("../lib/src/hoplite/internal.hal");
#[cfg(test)]
const CORE_TEST_SOURCE: &str = include_str!("../lib/test/hoplite/core_test.hal");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub name: Option<String>,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub id: u64,
    pub name: String,
    pub port: u16,
    pub hostnames: Vec<String>,
    pub routes: Vec<Route>,
    pub openapi_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub workers: usize,
    pub apps: Vec<App>,
}

pub fn load(project: &Project, profile: Option<&str>, production: bool) -> Result<Config, String> {
    if project.root.join("server.edn").is_file() || project.root.join("routes.edn").is_file() {
        return Err("server.edn and routes.edn are no longer supported; define a hoplite.core/app and select it with :project/profiles".into());
    }
    let selected = project
        .resolve_profile(profile)?
        .ok_or("Hoplite requires :project/profiles with :profile/language :hoplite")?;
    if selected.language != "hoplite" {
        return Err(format!(
            "project profile {:?} uses language {:?}, not :hoplite",
            selected.name, selected.language
        ));
    }
    let namespace = selected
        .main
        .split_once('/')
        .map(|(namespace, _)| namespace)
        .ok_or("Hoplite profile main must be a qualified app var such as app.core/app")?;
    let mut runtime = Runtime::new();
    runtime.register_resource("hoplite.core", CORE_SOURCE);
    runtime.register_resource("hoplite.internal", INTERNAL_SOURCE);
    project::register_sources(project, &mut runtime)?;
    let source = format!(
        "(ns hoplite.build (:require [{namespace}]))\n{}",
        selected.main
    );
    let value = runtime
        .eval_native_value(&source)
        .map_err(|error| format!("cannot evaluate Hoplite app {}: {error}", selected.main))?;
    let default_port = option_port(&selected.options)?.unwrap_or(8080);
    let default_workers = option_workers(&selected.options)?.unwrap_or(if production {
        available_workers()
    } else {
        1
    });
    parse_root(value, default_port, default_workers, production)
}

fn parse_root(
    value: Value,
    default_port: u16,
    default_workers: usize,
    production: bool,
) -> Result<Config, String> {
    match keyword_field(&value, "hoplite/type").as_deref() {
        Some("application") => Ok(Config {
            workers: default_workers,
            apps: vec![parse_app(value, 1, default_port, Vec::new(), production)?],
        }),
        Some("config") => parse_config(value, default_port, default_workers, production),
        _ => {
            Err("profile main must evaluate to hoplite.core/app or hoplite.internal/config".into())
        }
    }
}

fn parse_config(
    value: Value,
    default_port: u16,
    default_workers: usize,
    production: bool,
) -> Result<Config, String> {
    let workers = number_field(&value, "worker-processes")
        .map(|value| usize::try_from(value).map_err(|_| "worker-processes must be positive"))
        .transpose()?
        .unwrap_or(default_workers);
    if workers == 0 {
        return Err("worker-processes must be positive".into());
    }
    let app_values = sequence_field(&value, "apps").ok_or("internal config requires :apps")?;
    let mut apps = Vec::new();
    for (index, instance) in app_values.into_iter().enumerate() {
        let app_value = field(&instance, "app").ok_or("app instance requires :app")?;
        let app_value = match app_value {
            Value::Var(var) => var.deref_value(),
            value => value,
        };
        let id = u64::try_from(index + 1).map_err(|_| "too many apps")?;
        let port = number_field(&instance, "port")
            .map(valid_port)
            .transpose()?
            .unwrap_or_else(|| {
                if index == 0 {
                    default_port
                } else {
                    default_port.saturating_add(index as u16)
                }
            });
        let hostnames = sequence_field(&instance, "hostnames")
            .unwrap_or_default()
            .into_iter()
            .map(|value| text(&value, "hostname"))
            .collect::<Result<Vec<_>, _>>()?;
        let mut app = parse_app(app_value, id, port, hostnames, production)?;
        if let Some(name) = text_field(&instance, "id") {
            app.name = name;
        }
        apps.push(app);
    }
    if apps.is_empty() {
        return Err("internal config must contain at least one app".into());
    }
    validate_instances(&apps)?;
    Ok(Config { workers, apps })
}

fn parse_app(
    value: Value,
    id: u64,
    port: u16,
    hostnames: Vec<String>,
    production: bool,
) -> Result<App, String> {
    if keyword_field(&value, "hoplite/type").as_deref() != Some("application") {
        return Err("app instance :app must be a hoplite.core/app".into());
    }
    let name = text_field(&value, "name").unwrap_or_else(|| format!("app-{id}"));
    let mut routes = Vec::new();
    if let Some(handler) = field(&value, "handler") {
        routes.push(Route {
            method: "ANY".into(),
            path: "/*path".into(),
            handler: callable_name(&handler)?,
            name: Some(format!("{name}/handler")),
            summary: None,
        });
    }
    for resource in sequence_field(&value, "resources").unwrap_or_default() {
        flatten_resource(&resource, "", &mut routes)?;
    }
    if routes.is_empty() {
        return Err(format!("Hoplite app {name:?} has no resource operations"));
    }
    let openapi_path = field(&value, "openapi")
        .and_then(|openapi| text_field(&openapi, "path"))
        .or_else(|| (!production).then(|| "/openapi.json".into()));
    Ok(App {
        id,
        name,
        port,
        hostnames,
        routes,
        openapi_path,
    })
}

fn flatten_resource(value: &Value, parent: &str, output: &mut Vec<Route>) -> Result<(), String> {
    let items = sequence(value).ok_or("resource must be a vector")?;
    let path = text(
        items.first().ok_or("resource requires a path")?,
        "resource path",
    )?;
    if !path.starts_with('/') && !path.is_empty() {
        return Err(format!("resource path {path:?} must start with /"));
    }
    let full_path = join_path(parent, &path);
    let data = items
        .get(1)
        .filter(|value| core::map_entries(value).is_some());
    if let Some(data) = data {
        for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
            let Some(operation) = field(data, method) else {
                continue;
            };
            let handler = field(&operation, "handler").ok_or_else(|| {
                format!("{method:?} operation at {full_path:?} requires :handler")
            })?;
            output.push(Route {
                method: method.to_ascii_uppercase(),
                path: full_path.clone(),
                handler: callable_name(&handler)?,
                name: text_field(&operation, "name"),
                summary: text_field(&operation, "summary"),
            });
        }
    }
    let children_start = usize::from(data.is_some()) + 1;
    for child in items.iter().skip(children_start) {
        flatten_resource(child, &full_path, output)?;
    }
    Ok(())
}

fn validate_instances(apps: &[App]) -> Result<(), String> {
    for (index, app) in apps.iter().enumerate() {
        if apps[..index].iter().any(|other| other.name == app.name) {
            return Err(format!("duplicate app id {:?}", app.name));
        }
        if app.hostnames.is_empty()
            && apps[..index]
                .iter()
                .any(|other| other.port == app.port && other.hostnames.is_empty())
        {
            return Err(format!("multiple default apps listen on port {}", app.port));
        }
    }
    Ok(())
}

pub fn manifest(config: &Config) -> Result<Vec<u8>, String> {
    let apps = config
        .apps
        .iter()
        .map(|app| {
            map_value(vec![
                (keyword("id"), Value::Number(app.id as i64)),
                (
                    keyword("routes"),
                    Value::Vector(
                        app.routes
                            .iter()
                            .map(|route| {
                                map_value(vec![
                                    (keyword("method"), Value::String(route.method.clone())),
                                    (keyword("path"), Value::String(route.path.clone())),
                                    (keyword("handler"), Value::String(route.handler.clone())),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ])
        })
        .collect();
    hara_wasm::hta::encode(&map_value(vec![(keyword("apps"), Value::Vector(apps))]))
}

pub fn openapi(app: &App) -> String {
    let mut paths = JsonMap::new();
    for route in &app.routes {
        if route.method == "ANY" {
            continue;
        }
        let path = openapi_path(&route.path);
        let methods = paths.entry(path).or_insert_with(|| json!({}));
        let JsonValue::Object(methods) = methods else {
            continue;
        };
        let mut operation = JsonMap::new();
        if let Some(name) = &route.name {
            operation.insert(
                "operationId".into(),
                JsonValue::String(name.replace('/', ".")),
            );
        }
        if let Some(summary) = &route.summary {
            operation.insert("summary".into(), JsonValue::String(summary.clone()));
        }
        operation.insert(
            "responses".into(),
            json!({"200": {"description": "Success"}}),
        );
        methods.insert(
            route.method.to_ascii_lowercase(),
            JsonValue::Object(operation),
        );
    }
    serde_json::to_string_pretty(&json!({
        "openapi": "3.1.0",
        "info": {"title": app.name, "version": "0.1.0"},
        "paths": paths
    }))
    .expect("JSON serialization")
        + "\n"
}

fn openapi_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if let Some(name) = segment.strip_prefix(':') {
                format!("{{{name}}}")
            } else if let Some(name) = segment.strip_prefix('*') {
                format!("{{{name}}}")
            } else {
                segment.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn field(value: &Value, name: &str) -> Option<Value> {
    core::map_entries(value)?
        .into_iter()
        .find_map(|(key, value)| {
            matches!(&key, Value::Keyword(keyword) if keyword.as_str() == name).then_some(value)
        })
}

fn keyword_field(value: &Value, name: &str) -> Option<String> {
    match field(value, name)? {
        Value::Keyword(value) => Some(value.as_str().to_owned()),
        _ => None,
    }
}

fn text_field(value: &Value, name: &str) -> Option<String> {
    field(value, name).and_then(|value| text(&value, name).ok())
}

fn number_field(value: &Value, name: &str) -> Option<i64> {
    match field(value, name)? {
        Value::Number(value) => Some(value),
        _ => None,
    }
}

fn sequence_field(value: &Value, name: &str) -> Option<Vec<Value>> {
    field(value, name).and_then(|value| sequence(&value))
}

fn sequence(value: &Value) -> Option<Vec<Value>> {
    match value {
        Value::Vector(values) => Some(values.iter().cloned().collect()),
        Value::List(values) => Some(values.iter().cloned().collect()),
        Value::Tuple(values) => Some(values.iter().cloned().collect()),
        _ => None,
    }
}

fn text(value: &Value, label: &str) -> Result<String, String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Keyword(value) => Ok(value.as_str().to_owned()),
        Value::Symbol(value) => Ok(value.as_str().to_owned()),
        _ => Err(format!("{label} must be text")),
    }
}

fn callable_name(value: &Value) -> Result<String, String> {
    match value {
        Value::Var(var) => Ok(var.symbol().as_str().to_owned()),
        Value::Symbol(symbol) => Ok(symbol.as_str().to_owned()),
        _ => Err("operation :handler must be a Var such as #'app/handler".into()),
    }
}

fn join_path(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        return if child.is_empty() {
            "/".into()
        } else {
            child.into()
        };
    }
    if child.is_empty() || child == "/" {
        return parent.into();
    }
    format!("{}{}", parent.trim_end_matches('/'), child)
}

fn option_port(options: &hara_wasm::kernel::Form) -> Result<Option<u16>, String> {
    option_number(options, "port")?.map(valid_port).transpose()
}

fn option_workers(options: &hara_wasm::kernel::Form) -> Result<Option<usize>, String> {
    option_number(options, "workers")?
        .map(|value| usize::try_from(value).map_err(|_| "profile :workers must be positive".into()))
        .transpose()
}

fn option_number(options: &hara_wasm::kernel::Form, name: &str) -> Result<Option<i64>, String> {
    let hara_wasm::kernel::Form::Map(entries) = options else {
        return Err("profile options must be a map".into());
    };
    Ok(entries.iter().find_map(|(key, value)| match (key, value) {
        (hara_wasm::kernel::Form::Keyword(key), hara_wasm::kernel::Form::Number(value))
            if key == name =>
        {
            Some(*value)
        }
        _ => None,
    }))
}

fn valid_port(value: i64) -> Result<u16, String> {
    u16::try_from(value)
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| "port must be between 1 and 65535".into())
}

fn available_workers() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
}

fn keyword(name: &str) -> Value {
    Value::Keyword(name.into())
}

fn map_value(entries: Vec<(Value, Value)>) -> Value {
    Value::Map(entries.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_sources_evaluate_and_preserve_handler_vars() {
        let mut runtime = Runtime::new();
        runtime.register_resource("hoplite.core", CORE_SOURCE);
        let value = runtime.eval_native_value("(ns sample (:require [hoplite.core :as h])) (defn get-user [request] request) (h/app {:name :sample :resources [[\"/users/:id\" {:get {:name :users/get :summary \"Get user\" :handler #'get-user}}]]})").unwrap();
        let resources = sequence_field(&value, "resources").expect("resources");
        let resource = sequence(&resources[0]).expect("resource vector");
        assert!(
            field(&resource[1], "get").is_some(),
            "resource={:?}",
            resource[1]
        );
        let app = parse_app(value, 1, 8080, vec![], false).unwrap();
        assert_eq!(app.routes[0].handler, "sample/get-user");
        assert_eq!(app.routes[0].path, "/users/:id");
        assert!(openapi(&app).contains("/users/{id}"));
    }

    #[test]
    fn public_hal_contract_evaluates_from_disk() {
        let mut runtime = Runtime::new();
        runtime.register_resource("hoplite.core", CORE_SOURCE);
        runtime.register_resource("hoplite.internal", INTERNAL_SOURCE);
        assert_eq!(
            runtime.eval_native_value(CORE_TEST_SOURCE).unwrap(),
            Value::Bool(true)
        );
    }
}
