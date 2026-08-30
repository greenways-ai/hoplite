use hara_native::core::{self, Value};
use hara_native::kernel::{parse_forms, Form};
use hara_native::project::Project;
use hara_native::Runtime;
use hoplite::hara_source;
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

pub const CORE_SOURCE: &str = include_str!("../lib/src/hoplite/core.hal");
pub const HOST_SOURCE: &str = include_str!("../lib/src/hoplite/host.hal");
pub const INTERNAL_SOURCE: &str = include_str!("../lib/src/hoplite/internal.hal");
pub const NCHAN_SOURCE: &str = include_str!("../lib/src/hoplite/nchan.hal");
pub const RAW_SOURCE: &str = include_str!("../lib/src/hoplite/raw.hal");
pub const RTC_SOURCE: &str = include_str!("../lib/src/hoplite/rtc.hal");
pub const RESPONSE_SOURCE: &str = include_str!("../lib/src/hoplite/response_source.hal");
pub const SOCKET_SOURCE: &str = include_str!("../lib/src/hoplite/socket.hal");

const APPLICATION_RESOURCES: &[(&str, &str)] = &[
    ("hoplite.core", CORE_SOURCE),
    ("hoplite.host", HOST_SOURCE),
    ("hoplite.internal", INTERNAL_SOURCE),
    ("hoplite.nchan", NCHAN_SOURCE),
    ("hoplite.raw", RAW_SOURCE),
    ("hoplite.rtc", RTC_SOURCE),
    ("hoplite.socket", SOCKET_SOURCE),
];
const CONTRACT_RESOURCES: &[(&str, &str)] = &[("hoplite.response-source", RESPONSE_SOURCE)];
#[cfg(test)]
const CORE_TEST_SOURCE: &str = include_str!("../lib/test/hoplite/core_test.hal");
#[cfg(test)]
const RESPONSE_SOURCE_TEST_SOURCE: &str =
    include_str!("../lib/test/hoplite/response_source_test.hal");
#[cfg(test)]
const SOCKET_TEST_SOURCE: &str = include_str!("../lib/test/hoplite/socket_test.hal");

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteAdapter {
    Raw,
    Request,
    RequestHta,
}

impl RouteAdapter {
    fn parse(value: Option<String>, context: &str) -> Result<Self, String> {
        match value.as_deref().unwrap_or("request") {
            "raw" => Ok(Self::Raw),
            "request" => Ok(Self::Request),
            "request+hta" => Ok(Self::RequestHta),
            value => Err(format!(
                "{context} :route/adapter must be :raw, :request, or :request+hta; got :{value}"
            )),
        }
    }

    fn keyword(&self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Request => "request",
            Self::RequestHta => "request+hta",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub name: Option<String>,
    pub summary: Option<String>,
    pub adapter: RouteAdapter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestBodyPolicy {
    pub max_bytes: usize,
    pub max_chunk_bytes: usize,
}

/// A fixed-prefix, fixed-origin Nginx upstream owned by the Hoplite build.
///
/// Proxies are deliberately not selected from request data. They are inert app
/// configuration, validated before Nginx configuration is emitted, and remain
/// outside the Hara handler manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proxy {
    pub path: String,
    pub upstream: String,
    pub authority: String,
    pub server_name: String,
    pub secure: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    pub name: String,
    pub path_prefix: String,
    pub authorize: String,
    pub admit: String,
    pub message_buffer: usize,
    pub message_timeout_seconds: usize,
    pub max_channel_id_bytes: usize,
    pub max_subscribers: usize,
    pub transports: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub name: String,
    pub channel: String,
    pub handler: String,
    pub label: String,
    pub max_message_bytes: usize,
    pub idle_timeout_seconds: usize,
}

impl Channel {
    pub fn authorize_path(&self, app_id: u64) -> String {
        format!("/__hoplite/nchan/{app_id}/{}/authorize", self.name)
    }

    pub fn admit_path(&self, app_id: u64) -> String {
        format!("/__hoplite/nchan/{app_id}/{}/admit", self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub id: u64,
    pub name: String,
    pub port: u16,
    pub hostnames: Vec<String>,
    pub routes: Vec<Route>,
    pub console: Option<String>,
    pub request_body: Option<RequestBodyPolicy>,
    pub proxies: Vec<Proxy>,
    pub channels: Vec<Channel>,
    pub peers: Vec<Peer>,
    pub openapi_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub workers: usize,
    pub apps: Vec<App>,
}

pub fn register_contract_resources(runtime: &mut Runtime) {
    for &(namespace, source) in CONTRACT_RESOURCES {
        runtime.register_resource(namespace, source);
    }
}

pub fn register_resources(runtime: &mut Runtime) {
    for &(namespace, source) in APPLICATION_RESOURCES {
        runtime.register_resource(namespace, source);
    }
    register_contract_resources(runtime);
}

fn generated_output(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .any(|component| component.as_os_str() == OsStr::new(".hoplite"))
}

fn excluded_application_paths(project: &Project, root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![root.join(".hoplite")];
    paths.extend(project.artifact_paths.iter().map(|path| root.join(path)));
    if let Some(path) = &project.runtime_target_path {
        paths.push(root.join(path));
    }
    paths
}

fn excluded_application_path(path: &Path, excluded: &[PathBuf]) -> bool {
    excluded
        .iter()
        .any(|root| path == root || path.starts_with(root))
}

fn application_source_files(project: &Project) -> Result<Vec<PathBuf>, String> {
    let root = project.root.canonicalize().map_err(|error| {
        format!(
            "cannot resolve project root {}: {error}",
            project.root.display()
        )
    })?;
    let excluded = excluded_application_paths(project, &root);
    let mut pending = VecDeque::new();

    for relative in &project.source_paths {
        let declared = project.root.join(relative);
        if !declared.exists() {
            continue;
        }
        let metadata = fs::symlink_metadata(&declared).map_err(|error| {
            format!("cannot inspect source root {}: {error}", declared.display())
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "application source root cannot be a symlink: {}",
                declared.display()
            ));
        }
        if !metadata.is_dir() {
            return Err(format!(
                "application source root is not a directory: {}",
                declared.display()
            ));
        }
        let directory = declared.canonicalize().map_err(|error| {
            format!("cannot resolve source root {}: {error}", declared.display())
        })?;
        if !directory.starts_with(&root) {
            return Err(format!(
                "application source root escapes the project: {}",
                declared.display()
            ));
        }
        if generated_output(&root, &directory) || excluded_application_path(&directory, &excluded) {
            return Err(format!(
                "project source path {:?} points inside generated output",
                relative
            ));
        }
        pending.push_back(directory);
    }

    let mut visited = BTreeSet::new();
    let mut sources = BTreeSet::new();
    while let Some(directory) = pending.pop_front() {
        if !visited.insert(directory.clone()) {
            continue;
        }
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?;
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let path = entry.path();
            if entry.file_name() == OsStr::new(".hoplite")
                || excluded_application_path(&path, &excluded)
            {
                continue;
            }
            let kind = entry
                .file_type()
                .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
            if kind.is_symlink() {
                return Err(format!(
                    "application source discovery does not follow symlinks: {}",
                    path.display()
                ));
            }
            if kind.is_dir() {
                pending.push_back(path);
            } else if kind.is_file() && path.extension().and_then(OsStr::to_str) == Some("hal") {
                sources.insert(path);
            }
        }
    }

    Ok(sources.into_iter().collect())
}

fn declared_namespace(source: &str) -> Result<Option<String>, String> {
    Ok(parse_forms(source)?.into_iter().find_map(|form| match form {
        Form::List(values)
            if matches!(values.first(), Some(Form::Symbol(head)) if head == "ns" || head == "ns+") =>
        {
            match values.get(1) {
                Some(Form::Symbol(namespace)) => Some(namespace.clone()),
                _ => None,
            }
        }
        _ => None,
    }))
}

fn register_project_sources(project: &Project, runtime: &mut Runtime) -> Result<(), String> {
    for path in application_source_files(project)? {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let namespace = declared_namespace(&source)
            .map_err(|error| format!("{}: {error}", path.display()))?
            .ok_or_else(|| format!("{} does not declare an ns or ns+ namespace", path.display()))?;
        runtime.register_resource(&namespace, &source);
    }
    Ok(())
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
    let mut runtime = hara_source::compiler_runtime()?;
    register_resources(&mut runtime);
    register_project_sources(project, &mut runtime)?;
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
    let console = field(&value, "console")
        .map(|value| callable_name(&value))
        .transpose()
        .map_err(|_| {
            format!("Hoplite app {name:?} :console must be a Var such as #'app/console")
        })?;
    let default_adapter = RouteAdapter::parse(
        keyword_field(&value, "route/adapter"),
        &format!("Hoplite app {name:?}"),
    )?;
    let request_body = parse_request_body(
        field(&value, "request/body"),
        &format!("Hoplite app {name:?}"),
    )?;
    let proxies = sequence_field(&value, "proxies")
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, value)| parse_proxy(&value, &format!("Hoplite app {name:?} proxy {index}")))
        .collect::<Result<Vec<_>, _>>()?;
    validate_proxies(&proxies, &name)?;
    let channels = sequence_field(&value, "channels")
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            parse_channel(&value, &format!("Hoplite app {name:?} channel {index}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (index, channel) in channels.iter().enumerate() {
        if channels[..index]
            .iter()
            .any(|other| other.name == channel.name || other.path_prefix == channel.path_prefix)
        {
            return Err(format!(
                "Hoplite app {name:?} has a duplicate channel name or path"
            ));
        }
        if proxies.iter().any(|proxy| {
            channel
                .path_prefix
                .starts_with(proxy.path.trim_end_matches('/'))
                || proxy.path.starts_with(&channel.path_prefix)
        }) {
            return Err(format!(
                "Hoplite app {name:?} channel {:?} overlaps a proxy path",
                channel.name
            ));
        }
    }
    let peers = sequence_field(&value, "peers")
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            parse_peer(
                &value,
                &channels,
                &format!("Hoplite app {name:?} peer {index}"),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (index, peer) in peers.iter().enumerate() {
        if peers[..index].iter().any(|other| other.name == peer.name) {
            return Err(format!("Hoplite app {name:?} has a duplicate peer name"));
        }
    }

    let mut routes = Vec::new();
    if let Some(handler) = field(&value, "handler") {
        routes.push(Route {
            method: "ANY".into(),
            path: "/*path".into(),
            handler: callable_name(&handler)?,
            name: Some(format!("{name}/handler")),
            summary: None,
            adapter: default_adapter.clone(),
        });
    }
    for resource in sequence_field(&value, "resources").unwrap_or_default() {
        flatten_resource(&resource, "", &default_adapter, &mut routes)?;
    }
    for channel in &channels {
        routes.push(Route {
            method: "GET".into(),
            path: channel.authorize_path(id),
            handler: channel.authorize.clone(),
            name: Some(format!("{}/authorize", channel.name)),
            summary: None,
            adapter: RouteAdapter::Request,
        });
        routes.push(Route {
            method: "POST".into(),
            path: channel.admit_path(id),
            handler: channel.admit.clone(),
            name: Some(format!("{}/admit", channel.name)),
            summary: None,
            adapter: RouteAdapter::Request,
        });
    }
    if routes.is_empty() {
        return Err(format!("Hoplite app {name:?} has no resource operations"));
    }
    if request_body.is_some()
        && routes
            .iter()
            .any(|route| route.adapter == RouteAdapter::RequestHta)
    {
        return Err(format!(
            "Hoplite app {name:?} cannot combine :request/body with :request+hta routes"
        ));
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
        console,
        request_body,
        proxies,
        channels,
        peers,
        openapi_path,
    })
}

fn parse_peer(value: &Value, channels: &[Channel], context: &str) -> Result<Peer, String> {
    if keyword_field(value, "hoplite/type").as_deref() != Some("peer") {
        return Err(format!(
            "{context} must be constructed with hoplite.core/peer"
        ));
    }
    let entries = core::map_entries(value).ok_or_else(|| format!("{context} must be a map"))?;
    for (key, _) in entries {
        let Value::Keyword(key) = key else {
            return Err(format!("{context} keys must be keywords"));
        };
        if !matches!(
            key.as_str(),
            "hoplite/type"
                | "name"
                | "channel"
                | "handler"
                | "label"
                | "max-message-bytes"
                | "idle-timeout-seconds"
        ) {
            return Err(format!(
                "{context} contains unsupported field :{}",
                key.as_str()
            ));
        }
    }
    let name = text_field(value, "name").ok_or_else(|| format!("{context} requires :name"))?;
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(format!("{context} :name must be safe ASCII"));
    }
    let channel =
        text_field(value, "channel").ok_or_else(|| format!("{context} requires :channel"))?;
    if !channels.iter().any(|candidate| candidate.name == channel) {
        return Err(format!("{context} references unknown channel {channel:?}"));
    }
    let handler = field(value, "handler")
        .ok_or_else(|| format!("{context} requires :handler"))
        .and_then(|value| callable_name(&value))?;
    let label = text_field(value, "label").unwrap_or_else(|| name.clone());
    if label.is_empty()
        || label.len() > 64
        || !label.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(format!("{context} :label must be safe ASCII"));
    }
    Ok(Peer {
        name,
        channel,
        handler,
        label,
        max_message_bytes: bounded_channel_number(
            value,
            "max-message-bytes",
            65_536,
            1,
            1_048_576,
            context,
        )?,
        idle_timeout_seconds: bounded_channel_number(
            value,
            "idle-timeout-seconds",
            120,
            5,
            3600,
            context,
        )?,
    })
}

fn bounded_channel_number(
    value: &Value,
    field_name: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
    context: &str,
) -> Result<usize, String> {
    let candidate = number_field(value, field_name)
        .and_then(|number| usize::try_from(number).ok())
        .unwrap_or(default);
    (minimum..=maximum)
        .contains(&candidate)
        .then_some(candidate)
        .ok_or_else(|| format!("{context} :{field_name} must be between {minimum} and {maximum}"))
}

fn parse_channel(value: &Value, context: &str) -> Result<Channel, String> {
    if keyword_field(value, "hoplite/type").as_deref() != Some("channel") {
        return Err(format!(
            "{context} must be constructed with hoplite.core/channel"
        ));
    }
    let entries = core::map_entries(value).ok_or_else(|| format!("{context} must be a map"))?;
    for (key, _) in entries {
        let Value::Keyword(key) = key else {
            return Err(format!("{context} keys must be keywords"));
        };
        if !matches!(
            key.as_str(),
            "hoplite/type"
                | "name"
                | "path"
                | "profile"
                | "authorize"
                | "admit"
                | "message-buffer"
                | "message-timeout-seconds"
                | "max-channel-id-bytes"
                | "max-subscribers"
                | "transports"
        ) {
            return Err(format!(
                "{context} contains unsupported field :{}",
                key.as_str()
            ));
        }
    }
    if keyword_field(value, "profile")
        .as_deref()
        .unwrap_or("ephemeral")
        != "ephemeral"
    {
        return Err(format!("{context} :profile must be :ephemeral"));
    }
    let name = text_field(value, "name").ok_or_else(|| format!("{context} requires :name"))?;
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(format!(
            "{context} :name must contain only ASCII letters, digits, - or _"
        ));
    }
    let path = text_field(value, "path").ok_or_else(|| format!("{context} requires :path"))?;
    let path_prefix = path
        .strip_suffix("/:channel")
        .filter(|prefix| !prefix.is_empty() && safe_proxy_path(prefix))
        .ok_or_else(|| format!("{context} :path must be a safe path ending in /:channel"))?
        .to_owned();
    let authorize = field(value, "authorize")
        .ok_or_else(|| format!("{context} requires :authorize"))
        .and_then(|value| callable_name(&value))?;
    let admit = field(value, "admit")
        .ok_or_else(|| format!("{context} requires :admit"))
        .and_then(|value| callable_name(&value))?;
    let transports = match field(value, "transports") {
        Some(value) => {
            sequence(&value).ok_or_else(|| format!("{context} :transports must be a collection"))?
        }
        None => vec![
            Value::Keyword("websocket".into()),
            Value::Keyword("sse".into()),
        ],
    };
    let mut transport_names = Vec::new();
    for transport in transports {
        let transport = match transport {
            Value::Keyword(value) if value.as_str() == "websocket" => "websocket",
            Value::Keyword(value) if value.as_str() == "sse" => "eventsource",
            _ => {
                return Err(format!(
                    "{context} :transports may contain only :websocket and :sse"
                ))
            }
        };
        if !transport_names.iter().any(|value| value == transport) {
            transport_names.push(transport.to_owned());
        }
    }
    if transport_names.is_empty() {
        return Err(format!("{context} :transports must not be empty"));
    }
    Ok(Channel {
        name,
        path_prefix,
        authorize,
        admit,
        message_buffer: bounded_channel_number(value, "message-buffer", 8, 0, 64, context)?,
        message_timeout_seconds: bounded_channel_number(
            value,
            "message-timeout-seconds",
            30,
            1,
            300,
            context,
        )?,
        max_channel_id_bytes: bounded_channel_number(
            value,
            "max-channel-id-bytes",
            128,
            22,
            256,
            context,
        )?,
        max_subscribers: bounded_channel_number(value, "max-subscribers", 4, 1, 64, context)?,
        transports: transport_names,
    })
}

fn parse_request_body(
    value: Option<Value>,
    context: &str,
) -> Result<Option<RequestBodyPolicy>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let entries = core::map_entries(&value)
        .ok_or_else(|| format!("{context} :request/body must be a map"))?;
    for (key, _) in &entries {
        let Value::Keyword(key) = key else {
            return Err(format!("{context} :request/body keys must be keywords"));
        };
        if !matches!(key.as_str(), "max-bytes" | "max-chunk-bytes") {
            return Err(format!(
                "{context} :request/body contains unsupported field :{}",
                key.as_str()
            ));
        }
    }
    let max_bytes = number_field(&value, "max-bytes")
        .ok_or_else(|| format!("{context} :request/body requires :max-bytes"))?;
    let max_bytes = usize::try_from(max_bytes)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{context} :request/body :max-bytes must be positive"))?;
    let max_chunk_bytes = number_field(&value, "max-chunk-bytes")
        .map(|value| {
            usize::try_from(value)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| format!("{context} :request/body :max-chunk-bytes must be positive"))
        })
        .transpose()?
        .unwrap_or(max_bytes.min(64 * 1024));
    if max_chunk_bytes > max_bytes {
        return Err(format!(
            "{context} :request/body :max-chunk-bytes cannot exceed :max-bytes"
        ));
    }
    Ok(Some(RequestBodyPolicy {
        max_bytes,
        max_chunk_bytes,
    }))
}

fn parse_proxy(value: &Value, context: &str) -> Result<Proxy, String> {
    let entries = core::map_entries(value).ok_or_else(|| format!("{context} must be a map"))?;
    for (key, _) in &entries {
        let Value::Keyword(key) = key else {
            return Err(format!("{context} keys must be keywords"));
        };
        if !matches!(key.as_str(), "path" | "upstream") {
            return Err(format!(
                "{context} contains unsupported field :{}",
                key.as_str()
            ));
        }
    }

    let path = text(
        &field(value, "path").ok_or_else(|| format!("{context} requires :path"))?,
        &format!("{context} :path"),
    )?;
    validate_proxy_path(&path, context)?;

    let upstream = text(
        &field(value, "upstream").ok_or_else(|| format!("{context} requires :upstream"))?,
        &format!("{context} :upstream"),
    )?;
    let (authority, server_name, secure) = parse_proxy_upstream(&upstream, context)?;

    Ok(Proxy {
        path,
        upstream,
        authority,
        server_name,
        secure,
    })
}

fn validate_proxies(proxies: &[Proxy], app: &str) -> Result<(), String> {
    for (index, proxy) in proxies.iter().enumerate() {
        if proxies[..index]
            .iter()
            .any(|candidate| candidate.path == proxy.path)
        {
            return Err(format!(
                "Hoplite app {app:?} declares duplicate proxy path {:?}",
                proxy.path
            ));
        }
    }
    Ok(())
}

fn validate_proxy_path(path: &str, context: &str) -> Result<(), String> {
    if path == "/" || !path.starts_with('/') || !path.ends_with('/') {
        return Err(format!(
            "{context} :path must be a non-root prefix beginning and ending with /"
        ));
    }
    if !safe_proxy_path(path) || dot_segment(path) {
        return Err(format!("{context} :path is not a safe static prefix"));
    }
    Ok(())
}

fn parse_proxy_upstream(upstream: &str, context: &str) -> Result<(String, String, bool), String> {
    let (secure, rest) = if let Some(rest) = upstream.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = upstream.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(format!(
            "{context} :upstream must use https:// or loopback http://"
        ));
    };
    let slash = rest
        .find('/')
        .ok_or_else(|| format!("{context} :upstream must include a path ending with /"))?;
    let authority = &rest[..slash];
    let path = &rest[slash..];
    if authority.is_empty()
        || authority.contains('@')
        || path.contains('?')
        || path.contains('#')
        || !path.ends_with('/')
        || !safe_proxy_path(path)
        || dot_segment(path)
    {
        return Err(format!(
            "{context} :upstream is not a fixed, safe origin and path"
        ));
    }
    let (server_name, loopback) = parse_proxy_authority(authority, context)?;
    if !secure && !loopback {
        return Err(format!("{context} remote upstreams must use https://"));
    }
    Ok((authority.to_owned(), server_name, secure))
}

fn parse_proxy_authority(authority: &str, context: &str) -> Result<(String, bool), String> {
    if authority.starts_with('[') {
        let close = authority
            .find(']')
            .ok_or_else(|| format!("{context} :upstream has an invalid IPv6 authority"))?;
        let host = &authority[1..close];
        let suffix = &authority[close + 1..];
        if host != "::1" || (!suffix.is_empty() && !valid_port_suffix(suffix)) {
            return Err(format!(
                "{context} :upstream has an unsupported IPv6 authority"
            ));
        }
        return Ok((host.to_owned(), true));
    }

    if authority.matches(':').count() > 1 {
        return Err(format!("{context} :upstream authority is invalid"));
    }
    let (host, port) = authority
        .split_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    if host.is_empty()
        || host.starts_with('-')
        || host.ends_with('-')
        || host.starts_with('.')
        || host.ends_with('.')
        || !host
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '.'))
        || port.is_some_and(|value| {
            value.is_empty() || !value.chars().all(|character| character.is_ascii_digit())
        })
    {
        return Err(format!("{context} :upstream authority is invalid"));
    }
    if let Some(port) = port {
        let value = port
            .parse::<u16>()
            .ok()
            .filter(|value| *value != 0)
            .ok_or_else(|| format!("{context} :upstream port must be between 1 and 65535"))?;
        let _ = value;
    }
    let loopback = host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1";
    Ok((host.to_owned(), loopback))
}

fn valid_port_suffix(value: &str) -> bool {
    value
        .strip_prefix(':')
        .and_then(|port| port.parse::<u16>().ok())
        .is_some_and(|port| port != 0)
}

fn safe_proxy_path(value: &str) -> bool {
    value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '/' | '-' | '_' | '.' | '~')
    })
}

fn dot_segment(path: &str) -> bool {
    path.split('/').any(|segment| matches!(segment, "." | ".."))
}

fn flatten_resource(
    value: &Value,
    parent: &str,
    default_adapter: &RouteAdapter,
    output: &mut Vec<Route>,
) -> Result<(), String> {
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
            let context = format!("{method:?} operation at {full_path:?}");
            let handler = field(&operation, "handler")
                .ok_or_else(|| format!("{context} requires :handler"))?;
            if field(&operation, "route/auth").is_some() {
                return Err(format!(
                    "{context} :route/auth is not a Hoplite transport field; authorize inside the HAL handler"
                ));
            }
            output.push(Route {
                method: method.to_ascii_uppercase(),
                path: full_path.clone(),
                handler: callable_name(&handler)?,
                name: text_field(&operation, "name"),
                summary: text_field(&operation, "summary"),
                adapter: RouteAdapter::parse(
                    keyword_field(&operation, "route/adapter")
                        .or_else(|| Some(default_adapter.keyword().to_owned())),
                    &context,
                )?,
            });
        }
    }
    let children_start = usize::from(data.is_some()) + 1;
    for child in items.iter().skip(children_start) {
        flatten_resource(child, &full_path, default_adapter, output)?;
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
    let format = 2;
    let apps = config
        .apps
        .iter()
        .map(|app| {
            let mut fields = vec![
                (keyword("id"), Value::Number(app.id as i64)),
                (
                    keyword("peers"),
                    Value::Vector(
                        app.peers
                            .iter()
                            .map(|peer| {
                                map_value(vec![
                                    (keyword("name"), Value::String(peer.name.clone())),
                                    (keyword("channel"), Value::String(peer.channel.clone())),
                                    (keyword("handler"), Value::String(peer.handler.clone())),
                                    (keyword("label"), Value::String(peer.label.clone())),
                                    (
                                        keyword("max-message-bytes"),
                                        Value::Number(peer.max_message_bytes as i64),
                                    ),
                                    (
                                        keyword("idle-timeout-seconds"),
                                        Value::Number(peer.idle_timeout_seconds as i64),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
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
                                    (
                                        keyword("adapter"),
                                        Value::Keyword(route.adapter.keyword().into()),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ];
            if let Some(console) = &app.console {
                fields.push((keyword("console"), Value::String(console.clone())));
            }
            map_value(fields)
        })
        .collect();
    hara_native::hta::encode(&map_value(vec![
        (keyword("format"), Value::Number(format)),
        (keyword("apps"), Value::Vector(apps)),
    ]))
}

pub fn openapi(app: &App) -> String {
    let mut paths = JsonMap::new();
    for route in &app.routes {
        if route.method == "ANY" || route.path.starts_with("/__hoplite/") {
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
    let proxies = app
        .proxies
        .iter()
        .map(|proxy| json!({"path": proxy.path, "upstream": proxy.upstream}))
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&json!({
        "openapi": "3.1.0",
        "info": {"title": app.name, "version": "0.1.0"},
        "paths": paths,
        "x-hoplite-static-proxies": proxies
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
        Value::Set(values) => Some(values.iter().cloned().collect()),
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

fn option_port(options: &hara_native::kernel::Form) -> Result<Option<u16>, String> {
    option_number(options, "port")?.map(valid_port).transpose()
}

fn option_workers(options: &hara_native::kernel::Form) -> Result<Option<usize>, String> {
    option_number(options, "workers")?
        .map(|value| usize::try_from(value).map_err(|_| "profile :workers must be positive".into()))
        .transpose()
}

fn option_number(options: &hara_native::kernel::Form, name: &str) -> Result<Option<i64>, String> {
    let hara_native::kernel::Form::Map(entries) = options else {
        return Err("profile options must be a map".into());
    };
    Ok(entries.iter().find_map(|(key, value)| match (key, value) {
        (hara_native::kernel::Form::Keyword(key), hara_native::kernel::Form::Number(value))
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

    fn source_runtime() -> Runtime {
        hara_source::compiler_runtime().unwrap()
    }

    fn temp_project(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("hoplite-{name}-{}-{unique}", std::process::id()))
    }

    fn write_project(root: &Path, source_paths: &str) {
        fs::create_dir_all(root).unwrap();
        let manifest = r#"{:hara/type :project
 :hara/version "1.0.0"
 :project/id demo/app
 :project/version "0.1.0"
 :project/source-paths [SOURCE_PATHS]
 :project/test-paths []
 :project/extension-paths []
 :project/capabilities #{}
 :project/main demo.app
 :project/default-profile :server
 :project/profiles {:server {:profile/language :hoplite
                             :profile/main demo.app/app}}}"#
            .replace("SOURCE_PATHS", source_paths);
        fs::write(root.join("project.edn"), manifest).unwrap();
    }

    fn write_minimal_app(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(
            path,
            r#"(ns demo.app (:require [hoplite.core :as h]))
(defn hello [_request] {:status 200 :body "ok"})
(def app
  (h/app {:name "demo"
          :resources [["/hello" {:get {:handler #'hello}}]]}))"#,
        )
        .unwrap();
    }

    #[test]
    fn app_sources_evaluate_and_preserve_handler_vars() {
        let mut runtime = source_runtime();
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
    fn app_console_is_fixed_in_the_manifest() {
        let mut runtime = source_runtime();
        runtime.register_resource("hoplite.core", CORE_SOURCE);
        let value = runtime
            .eval_native_value(
                "(ns sample.console (:require [hoplite.core :as h])) \
                 (defn dispatch [request] request) \
                 (defn root [_request] {:status 200 :body \"ok\"}) \
                 (h/app {:name :sample \
                         :console #'dispatch \
                         :resources [[\"/\" {:get {:handler #'root}}]]})",
            )
            .unwrap();
        let app = parse_app(value, 1, 8080, vec![], false).unwrap();
        assert_eq!(app.console.as_deref(), Some("sample.console/dispatch"));

        let encoded = manifest(&Config {
            workers: 1,
            apps: vec![app],
        })
        .unwrap();
        let decoded = hara_native::hta::decode(&encoded).unwrap();
        let apps = sequence_field(&decoded, "apps").unwrap();
        assert_eq!(
            text_field(&apps[0], "console").as_deref(),
            Some("sample.console/dispatch")
        );
    }

    #[test]
    fn semantic_route_authorization_belongs_to_the_hal_handler() {
        let mut runtime = source_runtime();
        runtime.register_resource("hoplite.core", CORE_SOURCE);
        let value = runtime.eval_native_value("(ns sample (:require [hoplite.core :as h])) (defn submit [request] request) (h/app {:name :sample :resources [[\"/objects\" {:post {:handler #'submit :route/auth {:application \"example\"}}}]]})").unwrap();
        let error = parse_app(value, 1, 8080, vec![], false).unwrap_err();
        assert!(error.contains(":route/auth is not a Hoplite transport field"));
        assert!(error.contains("authorize inside the HAL handler"));
    }

    #[test]
    fn parses_fixed_https_and_loopback_proxy_prefixes() {
        let mut runtime = source_runtime();
        runtime.register_resource("hoplite.core", CORE_SOURCE);
        let value = runtime
            .eval_native_value(
                "(ns sample.proxy (:require [hoplite.core :as h])) (defn health [_] {:status 200}) (h/app {:name :proxy :proxies [{:path \"/space/\" :upstream \"https://greenways.space/beacon/v1/\"} {:path \"/dev/\" :upstream \"http://127.0.0.1:5173/api/\"}] :resources [[\"/health\" {:get {:handler #'health}}]]})",
            )
            .unwrap();
        let app = parse_app(value, 1, 58100, vec![], false).unwrap();
        assert_eq!(app.proxies.len(), 2);
        assert_eq!(app.proxies[0].path, "/space/");
        assert_eq!(app.proxies[0].authority, "greenways.space");
        assert_eq!(app.proxies[0].server_name, "greenways.space");
        assert!(app.proxies[0].secure);
        assert!(!app.proxies[1].secure);
        assert!(openapi(&app).contains("x-hoplite-static-proxies"));
    }

    #[test]
    fn parses_closed_nchan_channel_declarations() {
        let mut runtime = source_runtime();
        runtime.register_resource("hoplite.core", CORE_SOURCE);
        let value = runtime
            .eval_native_value(
                "(ns sample.channel (:require [hoplite.core :as h])) \
                 (defn authorize [_request] {:status 204}) \
                 (defn admit [_request] {:status 304}) \
                 (defn connected [_duplex] nil) \
                 (h/app {:name :sample \
                         :channels [(h/channel \
                                    {:name :signals \
                                     :path \"/tahto/signals/:channel\" \
                                     :profile :ephemeral \
                                     :authorize #'authorize \
                                     :admit #'admit})] \
                         :peers [(h/peer \
                                 {:name :sync \
                                  :channel :signals \
                                  :handler #'connected \
                                  :label \"tahto.sync\"})]})",
            )
            .unwrap();
        let app = parse_app(value, 7, 58100, vec![], false).unwrap();
        assert_eq!(app.channels.len(), 1);
        assert_eq!(app.channels[0].path_prefix, "/tahto/signals");
        assert_eq!(app.channels[0].authorize, "sample.channel/authorize");
        assert_eq!(app.channels[0].admit, "sample.channel/admit");
        assert_eq!(app.channels[0].message_buffer, 8);
        assert_eq!(app.channels[0].transports, ["websocket", "eventsource"]);
        assert_eq!(app.peers.len(), 1);
        assert_eq!(app.peers[0].channel, "signals");
        assert_eq!(app.peers[0].handler, "sample.channel/connected");
        assert_eq!(app.peers[0].max_message_bytes, 65_536);
        assert_eq!(app.routes[0].path, "/__hoplite/nchan/7/signals/authorize");
        assert_eq!(app.routes[1].path, "/__hoplite/nchan/7/signals/admit");
        assert!(!openapi(&app).contains("/__hoplite/"));
    }

    #[test]
    fn rejects_open_ended_nchan_configuration() {
        let value = map_value(vec![
            (keyword("hoplite/type"), keyword("channel")),
            (keyword("name"), keyword("signals")),
            (
                keyword("path"),
                Value::String("/signals/$request_uri".into()),
            ),
            (
                keyword("authorize"),
                Value::Symbol("sample/authorize".into()),
            ),
            (keyword("admit"), Value::Symbol("sample/admit".into())),
            (
                keyword("nginx/directives"),
                Value::String("return 200;".into()),
            ),
        ]);
        let error = parse_channel(&value, "test channel").unwrap_err();
        assert!(error.contains("unsupported field"), "{error}");
    }

    #[test]
    fn rejects_dynamic_and_insecure_remote_proxy_configuration() {
        let invalid = [
            ("/space/", "http://greenways.space/beacon/v1/"),
            ("/space/$path/", "https://greenways.space/beacon/v1/"),
            ("/space/", "https://user@greenways.space/beacon/v1/"),
            ("/space/", "https://greenways.space/beacon/../private/"),
        ];
        for (path, upstream) in invalid {
            let value = map_value(vec![
                (keyword("path"), Value::String(path.into())),
                (keyword("upstream"), Value::String(upstream.into())),
            ]);
            assert!(
                parse_proxy(&value, "test proxy").is_err(),
                "accepted {path} -> {upstream}"
            );
        }
    }

    #[test]
    fn public_hal_contract_evaluates_from_disk() {
        let mut runtime = source_runtime();
        runtime.register_resource("hoplite.core", CORE_SOURCE);
        runtime.register_resource("hoplite.internal", INTERNAL_SOURCE);
        assert_eq!(
            runtime.eval_native_value(CORE_TEST_SOURCE).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn raw_hal_accessors_evaluate_from_disk() {
        let mut runtime = source_runtime();
        runtime.register_resource("hoplite.raw", RAW_SOURCE);
        assert_eq!(
            runtime
                .eval_native_value(
                    "(ns sample.raw (:require [hoplite.raw :as raw])) [(raw/method {:method \"GET\" :headers {\"x-test\" \"yes\"}}) (raw/header {:method \"GET\" :headers {\"x-test\" \"yes\"}} \"x-test\")]",
                )
                .unwrap(),
            Value::Vector(vec![Value::String("GET".into()), Value::String("yes".into())].into())
        );
    }

    #[test]
    fn production_resources_cover_the_non_development_hal_library() {
        let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib/src/hoplite");
        let expected = fs::read_dir(&library)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", library.display()))
            .map(|entry| entry.expect("HAL library entry must be readable").path())
            .filter(|path| path.extension().and_then(OsStr::to_str) == Some("hal"))
            .filter_map(|path| {
                let stem = path.file_stem()?.to_str()?;
                (stem != "dev").then(|| format!("hoplite.{}", stem.replace('_', "-")))
            })
            .collect::<BTreeSet<_>>();
        let registered = APPLICATION_RESOURCES
            .iter()
            .chain(CONTRACT_RESOURCES.iter())
            .map(|(namespace, _)| (*namespace).to_owned())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            registered, expected,
            "every non-development Hoplite HAL namespace must be available to application builds"
        );

        let mut runtime = source_runtime();
        register_resources(&mut runtime);
        for (index, namespace) in registered.iter().enumerate() {
            let probe = format!(
                "(ns hoplite.resource-probe.n{index} (:require [{namespace} :as subject])) true"
            );
            assert_eq!(
                runtime.eval_native_value(&probe).unwrap_or_else(|error| {
                    panic!("registered resource {namespace} did not load: {error}")
                }),
                Value::Bool(true),
                "registered resource {namespace} did not evaluate"
            );
        }
    }

    #[test]
    fn socket_hal_contract_evaluates_from_the_production_registry() {
        let mut runtime = source_runtime();
        register_resources(&mut runtime);
        assert_eq!(
            runtime.eval_native_value(SOCKET_TEST_SOURCE).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn resources_exclude_the_retired_value_contract() {
        let mut runtime = source_runtime();
        register_resources(&mut runtime);
        assert!(runtime
            .eval_native_value("(ns sample.value (:require [hoplite.value :as value])) true",)
            .is_err());
    }

    #[test]
    fn response_source_boundary_contract_evaluates_from_hoplite() {
        let mut runtime = source_runtime();
        runtime.register_resource("hoplite.response-source", RESPONSE_SOURCE);
        runtime
            .eval_native_value(RESPONSE_SOURCE_TEST_SOURCE)
            .unwrap();
    }

    #[test]
    fn application_source_discovery_prunes_generated_output_before_descent() {
        let root = temp_project("generated-source-filter");
        write_project(&root, r#"".""#);
        fs::write(root.join("app.hal"), "(ns demo.app)").unwrap();
        fs::create_dir_all(root.join(".hoplite/nested")).unwrap();
        fs::write(
            root.join(".hoplite/nested/app.hal"),
            "this generated projection is deliberately not valid HAL",
        )
        .unwrap();

        let project = hara_native::project::read(&root).unwrap();
        let sources = application_source_files(&project).unwrap();
        assert_eq!(sources, vec![root.join("app.hal").canonicalize().unwrap()]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn application_source_discovery_deduplicates_overlapping_roots() {
        let root = temp_project("overlapping-source-roots");
        write_project(&root, r#""." "src""#);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("app.hal"), "(ns demo.app)").unwrap();
        fs::write(root.join("src/helper.hal"), "(ns demo.helper)").unwrap();

        let project = hara_native::project::read(&root).unwrap();
        let sources = application_source_files(&project).unwrap();
        let mut expected = vec![
            root.join("app.hal").canonicalize().unwrap(),
            root.join("src/helper.hal").canonicalize().unwrap(),
        ];
        expected.sort();
        assert_eq!(sources, expected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn application_source_discovery_rejects_generated_source_roots() {
        let root = temp_project("generated-source-root");
        write_project(&root, r#"".hoplite""#);
        fs::create_dir_all(root.join(".hoplite")).unwrap();
        fs::write(root.join(".hoplite/app.hal"), "(ns demo.app)").unwrap();

        let project = hara_native::project::read(&root).unwrap();
        let error = application_source_files(&project).unwrap_err();
        assert!(error.contains("inside generated output"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn application_source_discovery_rejects_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let root = temp_project("source-symlink");
        write_project(&root, r#""src""#);
        fs::create_dir_all(root.join("src/real")).unwrap();
        fs::write(root.join("src/real/app.hal"), "(ns demo.app)").unwrap();
        symlink(root.join("src/real"), root.join("src/link")).unwrap();

        let project = hara_native::project::read(&root).unwrap();
        let error = application_source_files(&project).unwrap_err();
        assert!(error.contains("does not follow symlinks"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn application_source_discovery_handles_deep_trees_iteratively() {
        let root = temp_project("deep-source-tree");
        write_project(&root, r#""src""#);
        let mut directory = root.join("src");
        for index in 0..256 {
            directory.push(format!("d{index}"));
        }
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("deep.hal"), "(ns demo.deep)").unwrap();

        let project = hara_native::project::read(&root).unwrap();
        let sources = application_source_files(&project).unwrap();
        assert_eq!(
            sources,
            vec![directory.join("deep.hal").canonicalize().unwrap()]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn minimal_application_project_loads() {
        let root = temp_project("minimal-application-load");
        write_project(&root, r#""src""#);
        write_minimal_app(&root.join("src/demo/app.hal"));

        let project = hara_native::project::read(&root).unwrap();
        let config = load(&project, None, true).unwrap();
        assert_eq!(config.apps[0].name, "demo");
        fs::remove_dir_all(root).unwrap();
    }
}
