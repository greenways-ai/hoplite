#!/usr/bin/env python3
from pathlib import Path

root = Path(__file__).resolve().parents[2]


def replace_once(path: Path, old: str, new: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(
            f"{path}: expected one replacement, found {count}: {old[:100]!r}"
        )
    path.write_text(text.replace(old, new, 1))


app = root / "core/src/app.rs"
replace_once(
    app,
    '''use hara_wasm::core::{self, Value};
use hara_wasm::kernel::{parse_forms, Form};
use hara_wasm::project::Project;
use hara_wasm::Runtime;
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use std::collections::{BTreeSet, VecDeque};''',
    '''use hara_wasm::core::{self, Value};
use hara_wasm::hta;
use hara_wasm::kernel::{parse_forms, Form};
use hara_wasm::project::Project;
use hara_wasm::Runtime;
use hoplite::console::protocol::{ClientBundle, CommandSet, ConsoleGrant};
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use std::collections::{BTreeMap, BTreeSet, VecDeque};''',
)
replace_once(
    app,
    '''use std::fs;
use std::path::{Path, PathBuf};''',
    '''use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};''',
)
replace_once(
    app,
    '''pub const RESPONSE_SOURCE: &str = include_str!("../lib/src/hoplite/response_source.hal");''',
    '''pub const RESPONSE_SOURCE: &str = include_str!("../lib/src/hoplite/response_source.hal");
pub const CONSOLE_ARTIFACT_PROTOCOL: &str = "hoplite.console-artifacts/0-alpha";''',
)
replace_once(
    app,
    '''#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {''',
    '''#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleArtifactConfig {
    pub client_namespace: String,
    pub descriptors_hta: Vec<u8>,
    pub grant_hta: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleBuildArtifact {
    pub app_id: u64,
    pub app_name: String,
    pub client_namespace: String,
    pub client_bundle: Vec<u8>,
    pub descriptors_hta: Vec<u8>,
    pub grant_hta: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {''',
)
replace_once(
    app,
    '''    pub routes: Vec<Route>,
    pub console: Option<String>,
    pub request_body: Option<RequestBodyPolicy>,''',
    '''    pub routes: Vec<Route>,
    pub console: Option<String>,
    pub console_artifacts: Option<ConsoleArtifactConfig>,
    pub request_body: Option<RequestBodyPolicy>,''',
)
replace_once(
    app,
    '''    let name = text_field(&value, "name").unwrap_or_else(|| format!("app-{id}"));
    let console = field(&value, "console")
        .map(|value| callable_name(&value))
        .transpose()
        .map_err(|_| {
            format!("Hoplite app {name:?} :console must be a Var such as #'app/console")
        })?;
    let default_adapter = RouteAdapter::parse(''',
    '''    let name = text_field(&value, "name").unwrap_or_else(|| format!("app-{id}"));
    let (console, console_artifacts) =
        parse_console(field(&value, "console"), &format!("Hoplite app {name:?}"))?;
    let default_adapter = RouteAdapter::parse(''',
)
replace_once(
    app,
    '''        routes,
        console,
        request_body,''',
    '''        routes,
        console,
        console_artifacts,
        request_body,''',
)

parse_peer_marker = '''fn parse_peer(value: &Value, channels: &[Channel], context: &str) -> Result<Peer, String> {'''
parse_console = r'''fn parse_console(
    value: Option<Value>,
    context: &str,
) -> Result<(Option<String>, Option<ConsoleArtifactConfig>), String> {
    let Some(value) = value else {
        return Ok((None, None));
    };
    if let Ok(handler) = callable_name(&value) {
        return Ok((Some(handler), None));
    }
    let entries = core::map_entries(&value)
        .ok_or_else(|| format!("{context} :console must be a Var or exact map"))?;
    for (key, _) in entries {
        let Value::Keyword(key) = key else {
            return Err(format!("{context} :console keys must be keywords"));
        };
        if !matches!(
            key.as_str(),
            "handler" | "client" | "descriptors" | "grant"
        ) {
            return Err(format!(
                "{context} :console contains unsupported field :{}",
                key.as_str()
            ));
        }
    }
    let handler = field(&value, "handler")
        .ok_or_else(|| format!("{context} :console requires :handler"))
        .and_then(|value| {
            callable_name(&value).map_err(|_| {
                format!("{context} :console :handler must be a Var such as #'app/console")
            })
        })?;
    let client_namespace = match field(&value, "client") {
        Some(Value::String(namespace)) if !namespace.is_empty() => namespace,
        _ => {
            return Err(format!(
                "{context} :console :client must be a non-empty namespace string"
            ))
        }
    };
    let descriptors = field(&value, "descriptors")
        .ok_or_else(|| format!("{context} :console requires :descriptors"))?;
    let grant = field(&value, "grant")
        .ok_or_else(|| format!("{context} :console requires :grant"))?;
    let commands = CommandSet::parse(descriptors.clone())
        .map_err(|error| format!("{context} :console descriptors are invalid: {error}"))?;
    let parsed_grant = ConsoleGrant::parse(&grant)
        .map_err(|error| format!("{context} :console grant is invalid: {error}"))?;
    commands
        .validate_grant(&parsed_grant)
        .map_err(|error| format!("{context} :console grant is invalid: {error}"))?;
    let descriptors_hta = hta::encode(&descriptors)
        .map_err(|error| format!("{context} :console descriptors are not immutable HTA: {error}"))?;
    let grant_hta = hta::encode(&grant)
        .map_err(|error| format!("{context} :console grant is not immutable HTA: {error}"))?;
    Ok((
        Some(handler),
        Some(ConsoleArtifactConfig {
            client_namespace,
            descriptors_hta,
            grant_hta,
        }),
    ))
}

'''
replace_once(app, parse_peer_marker, parse_console + parse_peer_marker)

manifest_marker = '''pub fn manifest(config: &Config) -> Result<Vec<u8>, String> {'''
artifact_code = r'''fn client_namespace(source: &str) -> Result<Option<String>, String> {
    Ok(parse_forms(source)?.into_iter().find_map(|form| match form {
        Form::List(values)
            if matches!(values.first(), Some(Form::Symbol(head)) if head == "ns") =>
        {
            match values.get(1) {
                Some(Form::Symbol(namespace)) => Some(namespace.clone()),
                _ => None,
            }
        }
        _ => None,
    }))
}

fn client_sources(project: &Project) -> Result<BTreeMap<String, String>, String> {
    let mut sources = BTreeMap::new();
    for path in application_source_files(project)? {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let Some(namespace) = client_namespace(&source)
            .map_err(|error| format!("{}: {error}", path.display()))?
        else {
            continue;
        };
        if sources.insert(namespace.clone(), source).is_some() {
            return Err(format!(
                "application console client namespace {namespace:?} is declared by more than one source file"
            ));
        }
    }
    Ok(sources)
}

pub fn console_artifacts(
    project: &Project,
    config: &Config,
) -> Result<Vec<ConsoleBuildArtifact>, String> {
    if !config
        .apps
        .iter()
        .any(|app| app.console_artifacts.is_some())
    {
        return Ok(Vec::new());
    }
    let sources = client_sources(project)?;
    config
        .apps
        .iter()
        .filter_map(|app| {
            app.console_artifacts
                .as_ref()
                .map(|console| (app, console))
        })
        .map(|(app, console)| {
            let source = sources.get(&console.client_namespace).ok_or_else(|| {
                format!(
                    "Hoplite app {:?} console client namespace {:?} has no exact ns source file",
                    app.name, console.client_namespace
                )
            })?;
            let client_bundle = ClientBundle::new(
                console.client_namespace.clone(),
                source.clone(),
            )?
            .encode()?;
            Ok(ConsoleBuildArtifact {
                app_id: app.id,
                app_name: app.name.clone(),
                client_namespace: console.client_namespace.clone(),
                client_bundle,
                descriptors_hta: console.descriptors_hta.clone(),
                grant_hta: console.grant_hta.clone(),
            })
        })
        .collect()
}

fn console_artifact_index(artifacts: &[ConsoleBuildArtifact]) -> Result<Vec<u8>, String> {
    let apps = artifacts
        .iter()
        .map(|artifact| {
            let directory = artifact.app_id.to_string();
            map_value(vec![
                (keyword("app"), Value::Number(artifact.app_id as i64)),
                (keyword("name"), Value::String(artifact.app_name.clone())),
                (
                    keyword("namespace"),
                    Value::String(artifact.client_namespace.clone()),
                ),
                (
                    keyword("bundle"),
                    Value::String(format!("{directory}/client.hcb")),
                ),
                (
                    keyword("descriptors"),
                    Value::String(format!("{directory}/commands.hta")),
                ),
                (
                    keyword("grant"),
                    Value::String(format!("{directory}/grant.hta")),
                ),
            ])
        })
        .collect();
    hta::encode(&map_value(vec![
        (
            keyword("protocol"),
            Value::String(CONSOLE_ARTIFACT_PROTOCOL.into()),
        ),
        (keyword("apps"), Value::Vector(apps)),
    ]))
    .map_err(|error| format!("cannot encode console artifact index: {error}"))
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|error| format!("cannot create console artifact directory {}: {error}", path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("cannot protect console artifact directory {}: {error}", path.display()))?;
    Ok(())
}

fn write_read_only(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes)
        .map_err(|error| format!("cannot write console artifact {}: {error}", path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o444))
        .map_err(|error| format!("cannot protect console artifact {}: {error}", path.display()))?;
    Ok(())
}

pub fn write_console_artifacts(
    output: &Path,
    project: &Project,
    config: &Config,
) -> Result<(), String> {
    let artifacts = console_artifacts(project, config)?;
    let root = output.join("console");
    match fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(format!(
                "refusing to replace non-directory console artifact path {}",
                root.display()
            ))
        }
        Ok(_) => fs::remove_dir_all(&root)
            .map_err(|error| format!("cannot replace console artifacts {}: {error}", root.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot inspect console artifact path {}: {error}",
                root.display()
            ))
        }
    }
    if artifacts.is_empty() {
        return Ok(());
    }
    create_private_directory(&root)?;
    for artifact in &artifacts {
        let directory = root.join(artifact.app_id.to_string());
        create_private_directory(&directory)?;
        write_read_only(&directory.join("client.hcb"), &artifact.client_bundle)?;
        write_read_only(
            &directory.join("commands.hta"),
            &artifact.descriptors_hta,
        )?;
        write_read_only(&directory.join("grant.hta"), &artifact.grant_hta)?;
    }
    write_read_only(&root.join("apps.hta"), &console_artifact_index(&artifacts)?)
}

'''
replace_once(app, manifest_marker, artifact_code + manifest_marker)

console_test_marker = '''    #[test]
    fn semantic_route_authorization_belongs_to_the_hal_handler() {'''
console_test = r'''    #[test]
    fn application_console_artifacts_are_deterministic_and_source_free() {
        let root = temp_project("console-artifacts");
        write_project(&root, r#""src""#);
        let source = root.join("src/demo");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("app.hal"),
            r#"(ns demo.app (:require [hoplite.core :as h]))
(defn dispatch [request] request)
(defn root [_request] {:status 200 :body "ok"})
(def app
  (h/app
   {:name "demo"
    :console
    {:handler #'dispatch
     :client "demo.console"
     :descriptors
     [{:command "status"
       :effect :read
       :input {:type :map :required #{} :optional #{}}}]
     :grant
     {:protocol "hoplite.console-grant/0-alpha"
      :console "console.template"
      :commands #{"status"}
      :write false}}
    :resources [["/" {:get {:handler #'root}}]]}))"#,
        )
        .unwrap();
        fs::write(
            source.join("console.hal"),
            r#"(ns demo.console)
(defn commands [] [])
(defn call [command input] [command input])"#,
        )
        .unwrap();

        let project = hara_wasm::project::read(&root).unwrap();
        let config = load(&project, None, false).unwrap();
        assert_eq!(config.apps[0].console.as_deref(), Some("demo.app/dispatch"));
        assert!(config.apps[0].console_artifacts.is_some());
        let first = console_artifacts(&project, &config).unwrap();
        let second = console_artifacts(&project, &config).unwrap();
        assert_eq!(first, second);
        let client = ClientBundle::decode(&first[0].client_bundle).unwrap();
        assert_eq!(client.namespace, "demo.console");
        assert!(client.source.contains("(ns demo.console)"));
        CommandSet::parse(hta::decode(&first[0].descriptors_hta).unwrap()).unwrap();
        ConsoleGrant::parse(&hta::decode(&first[0].grant_hta).unwrap()).unwrap();

        let output = root.join(".hoplite");
        fs::create_dir_all(&output).unwrap();
        write_console_artifacts(&output, &project, &config).unwrap();
        let directory = output.join("console/1");
        assert!(directory.join("client.hcb").is_file());
        assert!(directory.join("commands.hta").is_file());
        assert!(directory.join("grant.hta").is_file());
        let index = hta::decode(&fs::read(output.join("console/apps.hta")).unwrap()).unwrap();
        assert_eq!(
            text_field(&index, "protocol").as_deref(),
            Some(CONSOLE_ARTIFACT_PROTOCOL)
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(directory.join("client.hcb"))
                .unwrap()
                .permissions()
                .mode()
                & 0o222,
            0
        );
        assert!(!output.join("console/1/client.hal").exists());
        fs::remove_dir_all(root).unwrap();
    }

'''
replace_once(app, console_test_marker, console_test + console_test_marker)
replace_once(
    app,
    '''        assert_eq!(app.console.as_deref(), Some("sample.console/dispatch"));

        let encoded = manifest''',
    '''        assert_eq!(app.console.as_deref(), Some("sample.console/dispatch"));
        assert!(app.console_artifacts.is_none());

        let encoded = manifest''',
)

main = root / "core/src/main.rs"
replace_once(
    main,
    '''    let manifest = app::manifest(&app_config)?;
    application_bundle::encode(&manifest, &hbx0)
        .map_err(|error| format!("cannot encode Hoplite application bundle: {error}"))?;
    Ok(project)''',
    '''    let manifest = app::manifest(&app_config)?;
    application_bundle::encode(&manifest, &hbx0)
        .map_err(|error| format!("cannot encode Hoplite application bundle: {error}"))?;
    app::console_artifacts(&compilation_project, &app_config)?;
    Ok(project)''',
)
replace_once(
    main,
    '''    fs::create_dir_all(&configuration).map_err(io)?;
    write_runtime_source_projection(&output, runtime_source.as_deref())?;''',
    '''    fs::create_dir_all(&configuration).map_err(io)?;
    app::write_console_artifacts(&output, &compilation_project, &app_config)?;
    write_runtime_source_projection(&output, runtime_source.as_deref())?;''',
)
replace_once(
    main,
    '''            routes: Vec::new(),
            console: None,
            request_body: None,''',
    '''            routes: Vec::new(),
            console: None,
            console_artifacts: None,
            request_body: None,''',
)

diagnostics = root / "core/src/diagnostics.rs"
replace_once(
    diagnostics,
    '''                console: None,
                routes: vec![''',
    '''                console: None,
                console_artifacts: None,
                routes: vec![''',
)
