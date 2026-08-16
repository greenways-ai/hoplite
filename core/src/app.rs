use hara_wasm::core::{self, Value};
use hara_wasm::kernel::{parse_forms, Form};
use hara_wasm::project::Project;
use hara_wasm::Runtime;
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use std::any::Any;
use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

pub const CORE_SOURCE: &str = include_str!("../lib/src/hoplite/core.hal");
pub const HOST_SOURCE: &str = include_str!("../lib/src/hoplite/host.hal");
pub const INTERNAL_SOURCE: &str = include_str!("../lib/src/hoplite/internal.hal");
pub const RAW_SOURCE: &str = include_str!("../lib/src/hoplite/raw.hal");
pub const RTC_SOURCE: &str = include_str!("../lib/src/hoplite/rtc.hal");
pub const RESPONSE_SOURCE: &str = include_str!("../lib/src/hoplite/response_source.hal");

// Loading a registered application namespace adds one nested Hara resource
// boundary beyond direct source evaluation. The reviewed alpha runtime needs
// more than libtest's 2 MiB worker stack, so keep that finite evaluator depth
// inside one explicit Hoplite-owned build thread rather than changing every
// process or test through RUST_MIN_STACK.
const APPLICATION_BUILD_STACK_BYTES: usize = 8 * 1024 * 1024;
#[cfg(test)]
const CORE_TEST_SOURCE: &str = include_str!("../lib/test/hoplite/core_test.hal");
#[cfg(test)]
const RESPONSE_SOURCE_TEST_SOURCE: &str =
    include_str!("../lib/test/hoplite/response_source_test.hal");

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
    runtime.register_resource("hoplite.response-source", RESPONSE_SOURCE);
}

pub fn register_resources(runtime: &mut Runtime) {
    runtime.register_resource("hoplite.core", CORE_SOURCE);
    runtime.register_resource("hoplite.host", HOST_SOURCE);
    runtime.register_resource("hoplite.internal", INTERNAL_SOURCE);
    runtime.register_resource("hoplite.raw", RAW_SOURCE);
    runtime.register_resource("hoplite.rtc", RTC_SOURCE);
    register_contract_resources(runtime);
}

fn generated_output(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .any(|component| component.as_os_str() == OsStr::new(".hoplite"))
}

fn editor_artifact(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            name.starts_with(".#") || (name.starts_with('#') && name.ends_with('#'))
        })
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
        let metadata = fs::symlink_metadata(&declared)
            .map_err(|error| format!("cannot inspect source root {}: {error}", declared.display()))?;
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
        if generated_output(&root, &directory)
            || excluded_application_path(&directory, &excluded)
        {
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
                || editor_artifact(&path)
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
            } else if kind.is_file()
                && path.extension().and_then(OsStr::to_str) == Some("hal")
            {
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
            .ok_or_else(|| {
                format!(
                    "{} does not declare an ns or ns+ namespace",
                    path.display()
                )
            })?;
        runtime.register_resource(&namespace, &source);
    }
    Ok(())
}

pub fn load(project: &Project, profile: Option<&str>, production: bool) -> Result<Config, String> {
    let project = project.clone();
    let profile = profile.map(str::to_owned);
    let build = std::thread::Builder::new()
        .name("hoplite-application-build".into())
        .stack_size(APPLICATION_BUILD_STACK_BYTES)
        .spawn(move || load_inner(&project, profile.as_deref(), production))
        .map_err(|error| format!("cannot start Hoplite application build: {error}"))?;
    match build.join() {
        Ok(result) => result,
        Err(payload) => Err(format!(
            "Hoplite application build panicked: {}",
            panic_message(payload)
        )),
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".into()
    }
}

fn load_inner(
    project: &Project,
    profile: Option<&str>,
    production: bool,
) -> Result<Config, String> {
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
        request_body,
        proxies,
        channels,
        peers,
        openapi_path,
    })
}

fn parse_peer(value: &Value, channels: &[Channel], context: &str) -> Result<Peer, String> {
    if keyword_field(value, "hoplite/type").as_deref() != Some("peer") {
        return Err(format!("{context} must be built with hoplite.core/peer"));
    }
    reject_fields(
        value,
        &["hoplite/type", "name", "channel", "handler", "label", "max-message-bytes", "idle-timeout-seconds"],
        context,
    )?;
    let name = required_text_field(value, "name", context)?;
    valid_token(&name, "peer name")?;
    let channel = required_text_field(value, "channel", context)?;
    if !channels.iter().any(|candidate| candidate.name == channel) {
        return Err(format!("{context} references unknown channel {channel:?}"));
    }
    let handler = field(value, "handler")
        .ok_or_else(|| format!("{context} requires :handler"))
        .and_then(|value| callable_name(&value))?;
    let label = required_text_field(value, "label", context)?;
    valid_token(&label, "peer label")?;
    let max_message_bytes = number_field(value, "max-message-bytes")
        .map(valid_positive_usize)
        .transpose()?
        .unwrap_or(65_536);
    let idle_timeout_seconds = number_field(value, "idle-timeout-seconds")
        .map(valid_positive_usize)
        .transpose()?
        .unwrap_or(30);
    Ok(Peer {
        name,
        channel,
        handler,
        label,
        max_message_bytes,
        idle_timeout_seconds,
    })
}

fn parse_channel(value: &Value, context: &str) -> Result<Channel, String> {
    if keyword_field(value, "hoplite/type").as_deref() != Some("channel") {
        return Err(format!("{context} must be built with hoplite.core/channel"));
    }
    reject_fields(
        value,
        &[
            "hoplite/type",
            "name",
            "path",
            "profile",
            "authorize",
            "admit",
            "message-buffer",
            "message-timeout-seconds",
            "max-channel-id-bytes",
            "max-subscribers",
            "transports",
        ],
        context,
    )?;
    let name = required_text_field(value, "name", context)?;
    valid_token(&name, "channel name")?;
    let path = required_text_field(value, "path", context)?;
    let marker = "/:channel";
    let path_prefix = path
        .strip_suffix(marker)
        .filter(|prefix| prefix.starts_with('/') && !prefix.is_empty())
        .ok_or_else(|| format!("{context} :path must end with /:channel"))?
        .to_owned();
    if path_prefix.contains(['$', '{', '}', '\\']) || path_prefix.contains("..") {
        return Err(format!("{context} :path contains dynamic or unsafe segments"));
    }
    if keyword_field(value, "profile").as_deref() != Some("ephemeral") {
        return Err(format!("{context} :profile must be :ephemeral"));
    }
    let authorize = field(value, "authorize")
        .ok_or_else(|| format!("{context} requires :authorize"))
        .and_then(|value| callable_name(&value))?;
    let admit = field(value, "admit")
        .ok_or_else(|| format!("{context} requires :admit"))
        .and_then(|value| callable_name(&value))?;
    let message_buffer = number_field(value, "message-buffer")
        .map(valid_positive_usize)
        .transpose()?
        .unwrap_or(8);
    let message_timeout_seconds = number_field(value, "message-timeout-seconds")
        .map(valid_positive_usize)
        .transpose()?
        .unwrap_or(30);
    let max_channel_id_bytes = number_field(value, "max-channel-id-bytes")
        .map(valid_positive_usize)
        .transpose()?
        .unwrap_or(128);
    let max_subscribers = number_field(value, "max-subscribers")
        .map(valid_positive_usize)
        .transpose()?
        .unwrap_or(4);
    let transports = sequence_field(value, "transports")
        .unwrap_or_else(|| vec![keyword("websocket"), keyword("eventsource")])
        .into_iter()
        .map(|transport| text(&transport, "channel transport"))
        .collect::<Result<Vec<_>, _>>()?;
    if transports.is_empty()
        || transports
            .iter()
            .any(|transport| !matches!(transport.as_str(), "websocket" | "eventsource"))
    {
        return Err(format!(
            "{context} :transports must contain websocket and/or eventsource"
        ));
    }
    Ok(Channel {
        name,
        path_prefix,
        authorize,
        admit,
        message_buffer,
        message_timeout_seconds,
        max_channel_id_bytes,
        max_subscribers,
        transports,
    })
}

fn validate_proxies(proxies: &[Proxy], app_name: &str) -> Result<(), String> {
    for (index, proxy) in proxies.iter().enumerate() {
        for other in &proxies[..index] {
            if proxy.path.starts_with(&other.path) || other.path.starts_with(&proxy.path) {
                return Err(format!(
                    "Hoplite app {app_name:?} has overlapping proxy paths {:?} and {:?}",
                    other.path, proxy.path
                ));
            }
        }
    }
    Ok(())
}

fn parse_proxy(value: &Value, context: &str) -> Result<Proxy, String> {
    reject_fields(value, &["path", "upstream"], context)?;
    let path = required_text_field(value, "path", context)?;
    let upstream = required_text_field(value, "upstream", context)?;
    if !path.starts_with('/')
        || !path.ends_with('/')
        || path == "/"
        || path.contains(['$', '{', '}', '\\'])
        || dot_segment(&path)
        || !safe_proxy_path(&path)
    {
        return Err(format!(
            "{context} :path must be a fixed absolute prefix ending in /"
        ));
    }
    let (authority, server_name, secure) = parse_proxy_upstream(&upstream, context)?;
    Ok(Proxy {
        path,
        upstream,
        authority,
        server_name,
        secure,
    })
}

fn parse_proxy_upstream(upstream: &str, context: &str) -> Result<(String, String, bool), String> {
    let (secure, remainder) = if let Some(remainder) = upstream.strip_prefix("https://") {
        (true, remainder)
    } else if let Some(remainder) = upstream.strip_prefix("http://") {
        (false, remainder)
    } else {
        return Err(format!("{context} :upstream must use http:// or https://"));
    };
    let authority_end = remainder.find('/').unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let path = &remainder[authority_end..];
    if authority.is_empty()
        || authority.contains(['@', '$', '{', '}', '\\'])
        || path.contains(['$', '{', '}', '\\'])
        || dot_segment(path)
        || !safe_proxy_path(path)
    {
        return Err(format!(
            "{context} :upstream must be a fixed origin and path"
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
            map_value(vec![
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
                                let fields = vec![
                                    (keyword("method"), Value::String(route.method.clone())),
                                    (keyword("path"), Value::String(route.path.clone())),
                                    (keyword("handler"), Value::String(route.handler.clone())),
                                    (
                                        keyword("adapter"),
                                        Value::Keyword(route.adapter.keyword().into()),
                                    ),
                                ];
                                map_value(fields)
                            })
                            .collect(),
                    ),
                ),
            ])
        })
        .collect();
    hara_wasm::hta::encode(&map_value(vec![
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
            json!({"200": {"description": "Handler response"}}),
        );
        methods.insert(route.method.to_ascii_lowercase(), JsonValue::Object(operation));
    }
    let proxies = app
        .proxies
        .iter()
        .map(|proxy| {
            json!({
                "path": proxy.path,
                "upstream": proxy.upstream,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&json!({
        "openapi": "3.1.0",
        "info": {"title": app.name, "version": "0.2.0"},
        "paths": paths,
        "x-hoplite-static-proxies": proxies,
    }))
    .expect("OpenAPI document is JSON")
}

fn openapi_path(path: &str) -> String {
    let mut output = String::new();
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b':' {
            let start = index + 1;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            output.push('{');
            output.push_str(&path[start..end]);
            output.push('}');
            index = end;
        } else if path[index..].starts_with("*path") {
            output.push_str("{path}");
            index += 5;
        } else {
            output.push(bytes[index] as char);
            index += 1;
        }
    }
    output
}

pub fn manifest_json(config: &Config) -> JsonValue {
    json!({
        "format": 2,
        "apps": config.apps.iter().map(|app| {
            json!({
                "id": app.id,
                "name": app.name,
                "port": app.port,
                "hostnames": app.hostnames,
                "request_body": app.request_body.as_ref().map(|body| json!({
                    "max_bytes": body.max_bytes,
                    "max_chunk_bytes": body.max_chunk_bytes,
                })),
                "proxies": app.proxies.iter().map(|proxy| json!({
                    "path": proxy.path,
                    "upstream": proxy.upstream,
                    "authority": proxy.authority,
                    "server_name": proxy.server_name,
                    "secure": proxy.secure,
                })).collect::<Vec<_>>(),
                "channels": app.channels.iter().map(|channel| json!({
                    "name": channel.name,
                    "path_prefix": channel.path_prefix,
                    "authorize": channel.authorize,
                    "admit": channel.admit,
                    "message_buffer": channel.message_buffer,
                    "message_timeout_seconds": channel.message_timeout_seconds,
                    "max_channel_id_bytes": channel.max_channel_id_bytes,
                    "max_subscribers": channel.max_subscribers,
                    "transports": channel.transports,
                })).collect::<Vec<_>>(),
                "peers": app.peers.iter().map(|peer| json!({
                    "name": peer.name,
                    "channel": peer.channel,
                    "handler": peer.handler,
                    "label": peer.label,
                    "max_message_bytes": peer.max_message_bytes,
                    "idle_timeout_seconds": peer.idle_timeout_seconds,
                })).collect::<Vec<_>>(),
                "routes": app.routes.iter().map(|route| json!({
                    "method": route.method,
                    "path": route.path,
                    "handler": route.handler,
                    "adapter": route.adapter.keyword(),
                })).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
    })
}

fn parse_request_body(value: Option<Value>, context: &str) -> Result<Option<RequestBodyPolicy>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    reject_fields(&value, &["max-bytes", "max-chunk-bytes"], context)?;
    let max_bytes = number_field(&value, "max-bytes")
        .map(valid_positive_usize)
        .transpose()?
        .unwrap_or(1024 * 1024);
    let max_chunk_bytes = number_field(&value, "max-chunk-bytes")
        .map(valid_positive_usize)
        .transpose()?
        .unwrap_or(64 * 1024);
    if max_chunk_bytes > max_bytes {
        return Err(format!("{context} request max-chunk-bytes exceeds max-bytes"));
    }
    Ok(Some(RequestBodyPolicy {
        max_bytes,
        max_chunk_bytes,
    }))
}

fn reject_fields(value: &Value, allowed: &[&str], context: &str) -> Result<(), String> {
    let entries = core::map_entries(value).ok_or_else(|| format!("{context} must be a map"))?;
    for (key, _) in entries {
        let name = match key {
            Value::Keyword(keyword) => keyword.as_str(),
            _ => return Err(format!("{context} field keys must be keywords")),
        };
        if !allowed.contains(&name) {
            return Err(format!("{context} has unsupported field :{name}"));
        }
    }
    Ok(())
}

fn option_port(options: &Form) -> Result<Option<u16>, String> {
    let Form::Map(entries) = options else {
        return Err("profile options must be a map".into());
    };
    entries
        .iter()
        .find(|(key, _)| matches!(key, Form::Keyword(name) if name == "port"))
        .map(|(_, value)| match value {
            Form::Number(value) => value
                .parse::<u16>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| "profile port must be between 1 and 65535".to_string()),
            _ => Err("profile port must be a number".into()),
        })
        .transpose()
}

fn option_workers(options: &Form) -> Result<Option<usize>, String> {
    let Form::Map(entries) = options else {
        return Err("profile options must be a map".into());
    };
    entries
        .iter()
        .find(|(key, _)| matches!(key, Form::Keyword(name) if name == "workers"))
        .map(|(_, value)| match value {
            Form::Number(value) => value
                .parse::<usize>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| "profile workers must be positive".to_string()),
            _ => Err("profile workers must be a number".into()),
        })
        .transpose()
}

fn available_workers() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

fn required_text_field(value: &Value, key: &str, context: &str) -> Result<String, String> {
    field(value, key)
        .ok_or_else(|| format!("{context} requires :{key}"))
        .and_then(|value| text(&value, key))
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    field(value, key).and_then(|value| text(&value, key).ok())
}

fn keyword_field(value: &Value, key: &str) -> Option<String> {
    field(value, key).and_then(|value| match value {
        Value::Keyword(keyword) => Some(keyword.as_str().to_owned()),
        _ => None,
    })
}

fn number_field(value: &Value, key: &str) -> Option<i64> {
    field(value, key).and_then(|value| match value {
        Value::Number(value) => Some(value),
        _ => None,
    })
}

fn sequence_field(value: &Value, key: &str) -> Option<Vec<Value>> {
    field(value, key).and_then(|value| sequence(&value))
}

fn field(value: &Value, key: &str) -> Option<Value> {
    core::map_entries(value)?
        .into_iter()
        .find(|(candidate, _)| matches!(candidate, Value::Keyword(value) if value.as_str() == key))
        .map(|(_, value)| value)
}

fn sequence(value: &Value) -> Option<Vec<Value>> {
    core::sequence_values(value)
}

fn callable_name(value: &Value) -> Result<String, String> {
    match value {
        Value::Var(var) => Ok(var.symbol().to_string()),
        Value::Symbol(name) if name.contains('/') => Ok(name.clone()),
        _ => Err("handler must be a Var or qualified symbol".into()),
    }
}

fn text(value: &Value, context: &str) -> Result<String, String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Keyword(value) | Value::Symbol(value) => Ok(value.as_str().to_owned()),
        _ => Err(format!("{context} must be text")),
    }
}

fn valid_port(value: i64) -> Result<u16, String> {
    u16::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| "port must be between 1 and 65535".into())
}

fn valid_positive_usize(value: i64) -> Result<usize, String> {
    usize::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| "value must be positive".into())
}

fn valid_token(value: &str, context: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        Err(format!("{context} is invalid"))
    } else {
        Ok(())
    }
}

fn map_value(values: Vec<(Value, Value)>) -> Value {
    Value::Map(values.into_iter().collect())
}

fn keyword(value: &str) -> Value {
    Value::Keyword(value.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "hoplite-{name}-{}-{unique}",
            std::process::id()
        ))
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
    fn semantic_route_authorization_belongs_to_the_hal_handler() {
        let mut runtime = Runtime::new();
        runtime.register_resource("hoplite.core", CORE_SOURCE);
        let value = runtime.eval_native_value("(ns sample (:require [hoplite.core :as h])) (defn submit [request] request) (h/app {:name :sample :resources [[\"/objects\" {:post {:handler #'submit :route/auth {:application \"example\"}}}]]})").unwrap();
        let error = parse_app(value, 1, 8080, vec![], false).unwrap_err();
        assert!(error.contains(":route/auth is not a Hoplite transport field"));
        assert!(error.contains("authorize inside the HAL handler"));
    }

    #[test]
    fn parses_fixed_https_and_loopback_proxy_prefixes() {
        let mut runtime = Runtime::new();
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
        let mut runtime = Runtime::new();
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
        let mut runtime = Runtime::new();
        runtime.register_resource("hoplite.core", CORE_SOURCE);
        runtime.register_resource("hoplite.internal", INTERNAL_SOURCE);
        assert_eq!(
            runtime.eval_native_value(CORE_TEST_SOURCE).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn raw_hal_accessors_evaluate_from_disk() {
        let mut runtime = Runtime::new();
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
    fn resources_exclude_the_retired_value_contract() {
        let mut runtime = Runtime::new();
        register_resources(&mut runtime);
        assert!(
            runtime.require_resource("hoplite.value").is_err(),
            "register_resources must not reintroduce the retired hoplite.value namespace"
        );
    }

    #[test]
    fn response_source_boundary_contract_evaluates_from_hoplite() {
        let mut runtime = Runtime::new();
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

        let project = hara_wasm::project::read(&root).unwrap();
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

        let project = hara_wasm::project::read(&root).unwrap();
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
    fn application_source_discovery_ignores_editor_artifacts() {
        let root = temp_project("editor-artifacts");
        write_project(&root, r#""src""#);
        fs::create_dir_all(root.join("src/demo")).unwrap();
        fs::write(root.join("src/demo/core.hal"), "(ns demo.core)").unwrap();
        fs::write(root.join("src/demo/.#core.hal"), "unreadable editor lock").unwrap();
        fs::write(root.join("src/demo/#core.hal#"), "invalid editor backup").unwrap();

        let project = hara_wasm::project::read(&root).unwrap();
        let sources = application_source_files(&project).unwrap();
        assert_eq!(
            sources,
            vec![root.join("src/demo/core.hal").canonicalize().unwrap()]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn application_source_discovery_rejects_generated_source_roots() {
        let root = temp_project("generated-source-root");
        write_project(&root, r#"".hoplite""#);
        fs::create_dir_all(root.join(".hoplite")).unwrap();
        fs::write(root.join(".hoplite/app.hal"), "(ns demo.app)").unwrap();

        let project = hara_wasm::project::read(&root).unwrap();
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

        let project = hara_wasm::project::read(&root).unwrap();
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

        let project = hara_wasm::project::read(&root).unwrap();
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

        let project = hara_wasm::project::read(&root).unwrap();
        let config = load(&project, None, true).unwrap();
        assert_eq!(config.apps[0].name, "demo");
        fs::remove_dir_all(root).unwrap();
    }
}
