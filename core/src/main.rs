use hara_wasm::hta;
use hara_wasm::kernel::{parse_forms, Form};
use hara_wasm::project::{self, Project};
use hara_wasm::vm::{self, BytecodeBundleModule};
use hara_wasm::Runtime;
use hoplite_application_bundle as application_bundle;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
#[cfg(all(unix, feature = "embedded-nginx"))]
use std::os::unix::fs::PermissionsExt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::thread;
use std::time::Duration;

mod app;
mod dev_console;
mod diagnostics;
mod doctor;
mod host;
mod package;
mod platform;
mod repl;

const NGINX_VERSION: &str = "1.30.4";
#[cfg(feature = "embedded-nginx")]
const EMBEDDED_NGINX: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/nginx/sbin/nginx"
));

fn main() {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if let Err(error) = run(arguments) {
        eprintln!("hoplite: {error}");
        process::exit(1);
    }
}

fn run(arguments: Vec<String>) -> Result<(), String> {
    match arguments.first().map(String::as_str) {
        Some("version" | "--version" | "-V") => {
            println!("Hoplite {}", env!("CARGO_PKG_VERSION"));
            println!("Hara {}", env!("CARGO_PKG_VERSION"));
            println!("Nginx {} ({})", NGINX_VERSION, nginx_distribution());
        }
        Some("serve") => run_serve_command(&arguments[1..])?,
        Some("doctor") => doctor::run(&arguments[1..])?,
        Some("inspect") => diagnostics::run(&arguments[1..])?,
        Some("verify") => run_verify_command(&arguments[1..])?,
        Some("package") => package::run(&arguments[1..])?,
        Some("eval") => {
            let source = arguments.get(1..).unwrap_or_default().join(" ");
            if source.is_empty() {
                return Err("eval requires a Hara expression".into());
            }
            println!("{}", Runtime::new().eval_native(&source)?);
        }
        Some("run") => {
            let path = arguments.get(1).ok_or("run requires a .hal file")?;
            let source = fs::read_to_string(path).map_err(io)?;
            println!("{}", Runtime::new().eval_native(&source)?);
        }
        Some("repl") | None => repl::run()?,
        Some("help" | "--help" | "-h") => usage(),
        Some(command) => return Err(format!("unknown command: {command}")),
    }
    Ok(())
}

fn usage() {
    println!("Hoplite · Hara on Nginx");
    println!();
    println!("Usage:");
    println!("  hoplite [repl]");
    println!("  hoplite eval EXPRESSION");
    println!("  hoplite run FILE");
    println!("  hoplite doctor [--json] [--show-paths] [--deep] [--strict] [PROJECT]");
    println!("  hoplite inspect [--json] [--show-paths] [PROJECT|OUTPUT|BUNDLE]");
    println!("  hoplite verify [--manifest FILE] [PROJECT|OUTPUT|BUNDLE]");
    println!("  hoplite package [check|build|inspect|install|verify] [OPTIONS]");
    println!("  hoplite serve [start|stop|reload|status|build|check] [PROJECT]");
    println!("  hoplite version");
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ApplicationVerification {
    bundle_path: PathBuf,
    manifest_path: PathBuf,
    bundle_bytes: usize,
    manifest_bytes: usize,
    bytecode_bytes: usize,
    manifest_digest: [u8; 32],
}

fn run_verify_command(arguments: &[String]) -> Result<(), String> {
    if matches!(
        arguments.first().map(String::as_str),
        Some("--help" | "-h" | "help")
    ) {
        verify_usage();
        return Ok(());
    }

    let mut target = None;
    let mut manifest_override = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--manifest" => {
                index += 1;
                manifest_override = Some(PathBuf::from(
                    arguments
                        .get(index)
                        .ok_or("verify --manifest requires a file")?,
                ));
            }
            value if target.is_none() => target = Some(PathBuf::from(value)),
            value => return Err(format!("unexpected verify argument: {value}")),
        }
        index += 1;
    }

    let target = target.unwrap_or(env::current_dir().map_err(io)?);
    let (bundle_path, default_manifest_path) = verification_paths(&target);
    let manifest_path = manifest_override.unwrap_or(default_manifest_path);
    let verified = verify_application_bundle(&bundle_path, &manifest_path)?;
    println!("verified {}", application_bundle::FORMAT);
    println!(
        "bundle: {} ({} bytes)",
        verified.bundle_path.display(),
        verified.bundle_bytes
    );
    println!(
        "manifest: {} ({} bytes)",
        verified.manifest_path.display(),
        verified.manifest_bytes
    );
    println!("runtime ABI: {}", application_bundle::RUNTIME_ABI_VERSION);
    println!("manifest sha256: {}", lower_hex(&verified.manifest_digest));
    println!("embedded HBX0: {} bytes", verified.bytecode_bytes);
    Ok(())
}

fn verify_usage() {
    println!("Verify a built Hoplite application without executing it");
    println!();
    println!("usage: hoplite verify [--manifest FILE] [PROJECT|OUTPUT|BUNDLE]");
}

fn verification_paths(target: &Path) -> (PathBuf, PathBuf) {
    let explicit_bundle = target.is_file()
        || target
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("hbx"));
    if explicit_bundle {
        let manifest = target
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("apps.hta");
        return (target.to_path_buf(), manifest);
    }

    let is_output = target
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == ".hoplite")
        || target.join("app.hbx").is_file()
        || target.join("apps.hta").is_file();
    let output = if is_output {
        target.to_path_buf()
    } else {
        target.join(".hoplite")
    };
    (output.join("app.hbx"), output.join("apps.hta"))
}

fn verify_application_bundle(
    bundle_path: &Path,
    manifest_path: &Path,
) -> Result<ApplicationVerification, String> {
    let bundle =
        application_bundle::read_bundle_file(bundle_path).map_err(|error| error.to_string())?;
    let manifest =
        application_bundle::read_manifest_file(manifest_path).map_err(|error| error.to_string())?;
    let decoded =
        application_bundle::decode(&bundle, &manifest).map_err(|error| error.to_string())?;
    hta::decode(&manifest)
        .map_err(|error| format!("hoplite/application-manifest-invalid: {error}"))?;
    Ok(ApplicationVerification {
        bundle_path: bundle_path.to_path_buf(),
        manifest_path: manifest_path.to_path_buf(),
        bundle_bytes: bundle.len(),
        manifest_bytes: manifest.len(),
        bytecode_bytes: decoded.bytecode().len(),
        manifest_digest: decoded.manifest_digest(),
    })
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub(crate) fn run_serve_command(arguments: &[String]) -> Result<(), String> {
    if matches!(
        arguments.first().map(String::as_str),
        Some("--help" | "-h" | "help")
    ) {
        serve_usage();
        return Ok(());
    }
    let mut action = "start";
    let mut project = None;
    let mut settings = BuildSettings::default();
    let mut index = 0;
    if let Some(
        candidate @ ("start" | "foreground" | "install" | "uninstall" | "status" | "reload"
        | "stop" | "build" | "check"),
    ) = arguments.first().map(String::as_str)
    {
        action = candidate;
        index = 1;
    }
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--profile" => {
                index += 1;
                settings.profile = Some(
                    arguments
                        .get(index)
                        .ok_or("--profile requires a name")?
                        .clone(),
                );
            }
            "--mode" => {
                index += 1;
                settings.production = match arguments.get(index).map(String::as_str) {
                    Some("dev") => false,
                    Some("prod") => true,
                    _ => return Err("--mode must be dev or prod".into()),
                };
            }
            value if project.is_none() => project = Some(value.to_owned()),
            value => return Err(format!("unexpected serve argument: {value}")),
        }
        index += 1;
    }
    let root = project
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or(env::current_dir().map_err(io)?);
    match action {
        "start" => serve(&root, &settings),
        "foreground" => run_foreground(&root, &settings),
        "install" => launchd_install(&root, &settings),
        "uninstall" => launchd_uninstall(&root),
        "status" => status(&root),
        "reload" => signal(&root, "reload"),
        "stop" => signal(&root, "quit"),
        "build" => build(&root, &settings).map(|output| println!("built {}", output.display())),
        "check" => {
            check(&root, &settings).map(|project| println!("{} is ready for Hoplite", project.id))
        }
        _ => unreachable!(),
    }
}

#[derive(Clone, Debug, Default)]
struct BuildSettings {
    profile: Option<String>,
    production: bool,
}

fn serve_usage() {
    println!(
        "Hoplite {} — Hara on Nginx {}",
        env!("CARGO_PKG_VERSION"),
        NGINX_VERSION
    );
    println!("usage: hoplite serve [--profile NAME] [--mode dev|prod] [PROJECT]");
    println!("       hoplite serve <foreground|install|uninstall|status|reload|stop|build|check> [--profile NAME] [--mode dev|prod] [PROJECT]");
}

fn check(root: &Path, settings: &BuildSettings) -> Result<Project, String> {
    let project = project::discover(root)?;
    reject_legacy_extension_manifest(&project.root)?;
    let sources = source_files(&project)?;
    if sources.is_empty() {
        return Err("project has no .hal source files".into());
    }
    let modules = application_modules(&sources)?;
    let hbx0 = compile_application_modules(&modules)
        .map_err(|error| format!("Hoplite bytecode compilation failed: {error}"))?;
    let app_config = app::load(&project, settings.profile.as_deref(), settings.production)?;
    let manifest = app::manifest(&app_config)?;
    application_bundle::encode(&manifest, &hbx0)
        .map_err(|error| format!("cannot encode Hoplite application bundle: {error}"))?;
    platform::load(&project, settings.profile.as_deref())?;
    Ok(project)
}

fn reject_legacy_extension_manifest(root: &Path) -> Result<(), String> {
    let legacy = root.join("hara.extension.edn");
    if legacy.is_file() {
        return Err(format!(
            "{} is no longer loaded by Hoplite; move its extension contract into project.edn under the selected profile's :profile/extensions map",
            legacy.display()
        ));
    }
    Ok(())
}

fn write_runtime_source_projection(
    output: &Path,
    runtime_source: Option<&str>,
) -> Result<(), String> {
    let path = output.join("app.hal");
    match runtime_source {
        Some(source) => fs::write(&path, source).map_err(io),
        None => match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "cannot remove production source projection {}: {error}",
                path.display()
            )),
        },
    }
}

fn build(root: &Path, settings: &BuildSettings) -> Result<PathBuf, String> {
    let project = check(root, settings)?;
    let sources = source_files(&project)?;
    let modules = application_modules(&sources)?;
    let runtime_source = if settings.production {
        None
    } else {
        Some(runtime_application_modules(&modules)?)
    };
    let hbx0 = compile_application_modules(&modules)
        .map_err(|error| format!("Hoplite bytecode compilation failed: {error}"))?;
    let app_config = app::load(&project, settings.profile.as_deref(), settings.production)?;
    let manifest = app::manifest(&app_config)?;
    let bundle = application_bundle::encode(&manifest, &hbx0)
        .map_err(|error| format!("cannot encode Hoplite application bundle: {error}"))?;
    let platform_config = platform::load(&project, settings.profile.as_deref())?;
    let output = project.root.join(".hoplite");
    let configuration = output.join("conf");
    fs::create_dir_all(&configuration).map_err(io)?;
    write_runtime_source_projection(&output, runtime_source.as_deref())?;
    fs::write(output.join("app.hbx"), bundle).map_err(io)?;
    fs::write(output.join("apps.hta"), manifest).map_err(io)?;
    fs::write(
        output.join("platform.edn"),
        platform::readable_manifest(&platform_config),
    )
    .map_err(io)?;
    fs::write(
        output.join("platform.hta"),
        platform::manifest(&platform_config)?,
    )
    .map_err(io)?;
    let openapi_dir = output.join("openapi");
    fs::create_dir_all(&openapi_dir).map_err(io)?;
    for application in &app_config.apps {
        fs::write(
            openapi_dir.join(format!("{}.json", application.name)),
            app::openapi(application),
        )
        .map_err(io)?;
    }
    fs::write(
        configuration.join("nginx.conf"),
        nginx_app_configuration(&project, &app_config)?,
    )
    .map_err(io)?;
    Ok(output)
}

fn serve(root: &Path, settings: &BuildSettings) -> Result<(), String> {
    let output = build(root, settings)?;
    let project_root = output.parent().ok_or("invalid Hoplite output path")?;
    let exit = Command::new(nginx_binary()?)
        .arg("-p")
        .arg(project_root)
        .arg("-c")
        .arg(".hoplite/conf/nginx.conf")
        .arg("-e")
        .arg(".hoplite/error.log")
        .status()
        .map_err(|error| format!("cannot start Hoplite Nginx: {error}"))?;
    if !exit.success() {
        return Err(format!("Nginx exited with {exit}"));
    }
    wait_for_status(project_root)
}

fn run_foreground(root: &Path, settings: &BuildSettings) -> Result<(), String> {
    let output = build(root, settings)?;
    let project_root = output.parent().ok_or("invalid Hoplite output path")?;
    let mut command = Command::new(nginx_binary()?);
    command
        .arg("-p")
        .arg(project_root)
        .arg("-c")
        .arg(".hoplite/conf/nginx.conf")
        .arg("-e")
        .arg(".hoplite/error.log")
        .args(["-g", "daemon off;"]);
    let exit = command
        .status()
        .map_err(|error| format!("cannot run Hoplite Nginx: {error}"))?;
    if exit.success() {
        Ok(())
    } else {
        Err(format!("Nginx exited with {exit}"))
    }
}

#[cfg(target_os = "macos")]
fn launchd_install(root: &Path, settings: &BuildSettings) -> Result<(), String> {
    let output = build(root, settings)?;
    let project_root = output.parent().ok_or("invalid Hoplite output path")?;
    let project = project::discover(project_root)?;
    let executable = env::current_exe().map_err(io)?.canonicalize().map_err(io)?;
    let label = launchd_label(&project.id);
    let home = env::var_os("HOME").ok_or("HOME is not set")?;
    let agents = PathBuf::from(home).join("Library/LaunchAgents");
    fs::create_dir_all(&agents).map_err(io)?;
    let plist = agents.join(format!("{label}.plist"));
    fs::write(&plist, launchd_plist(&label, &executable, project_root)).map_err(io)?;

    let uid = command_output("id", &["-u"])?;
    let domain = format!("gui/{uid}");
    let service = format!("{domain}/{label}");
    if Command::new("launchctl")
        .args(["print", &service])
        .output()
        .map_err(io)?
        .status
        .success()
    {
        let _ = Command::new("launchctl")
            .args(["bootout", &service])
            .status();
    }
    let status = Command::new("launchctl")
        .arg("bootstrap")
        .arg(&domain)
        .arg(&plist)
        .status()
        .map_err(io)?;
    if !status.success() {
        return Err(format!("launchctl bootstrap failed with {status}"));
    }
    println!("installed {label}");
    println!("launchd plist: {}", plist.display());
    wait_for_status(project_root)
}

#[cfg(not(target_os = "macos"))]
fn launchd_install(_root: &Path, _settings: &BuildSettings) -> Result<(), String> {
    Err("hoplite install requires macOS launchd".into())
}

#[cfg(target_os = "macos")]
fn launchd_uninstall(root: &Path) -> Result<(), String> {
    let project = project::discover(root)?;
    let label = launchd_label(&project.id);
    let uid = command_output("id", &["-u"])?;
    let service = format!("gui/{uid}/{label}");
    let status = Command::new("launchctl")
        .args(["bootout", &service])
        .status()
        .map_err(io)?;
    if !status.success() {
        return Err(format!("launchctl bootout failed with {status}"));
    }
    if let Some(home) = env::var_os("HOME") {
        let plist = PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist"));
        if plist.exists() {
            fs::remove_file(&plist).map_err(io)?;
        }
    }
    println!("uninstalled {label}");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn launchd_uninstall(_root: &Path) -> Result<(), String> {
    Err("hoplite uninstall requires macOS launchd".into())
}

fn launchd_label(project_id: &str) -> String {
    let suffix = project_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("org.hara.hoplite.{suffix}")
}

fn launchd_plist(label: &str, executable: &Path, project_root: &Path) -> String {
    let output = project_root.join(".hoplite/launchd.stdout.log");
    let error = project_root.join(".hoplite/launchd.stderr.log");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
  <key>Label</key><string>{}</string>\n\
  <key>ProgramArguments</key>\n\
  <array>\n\
    <string>{}</string>\n\
    <string>serve</string>\n\
    <string>foreground</string>\n\
    <string>{}</string>\n\
  </array>\n\
  <key>WorkingDirectory</key><string>{}</string>\n\
  <key>RunAtLoad</key><true/>\n\
  <key>KeepAlive</key><true/>\n\
  <key>ProcessType</key><string>Background</string>\n\
  <key>StandardOutPath</key><string>{}</string>\n\
  <key>StandardErrorPath</key><string>{}</string>\n\
</dict>\n\
</plist>\n",
        xml_escape(label),
        xml_escape(&executable.display().to_string()),
        xml_escape(&project_root.display().to_string()),
        xml_escape(&project_root.display().to_string()),
        xml_escape(&output.display().to_string()),
        xml_escape(&error.display().to_string())
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "macos")]
fn command_output(command: &str, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new(command).args(arguments).output().map_err(io)?;
    if !output.status.success() {
        return Err(format!("{command} exited with {}", output.status));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|error| error.to_string())
}

fn wait_for_status(root: &Path) -> Result<(), String> {
    let mut last_error = None;
    for _ in 0..200 {
        match status(root) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(last_error.unwrap_or_else(|| "Hoplite failed to start".into()))
}

fn status(root: &Path) -> Result<(), String> {
    let project = project::discover(root)?;
    let pid_path = project.root.join(".hoplite/nginx.pid");
    let pid = fs::read_to_string(&pid_path)
        .map_err(|_| format!("Hoplite is stopped (no {})", pid_path.display()))?;
    let pid = pid.trim();
    let running = Command::new("kill")
        .args(["-0", pid])
        .status()
        .map_err(|error| format!("cannot inspect Hoplite process {pid}: {error}"))?
        .success();
    if !running {
        return Err(format!("Hoplite is stopped (stale pid {pid})"));
    }
    println!("Hoplite is running (pid {pid})");
    Ok(())
}

fn signal(root: &Path, signal: &str) -> Result<(), String> {
    let project = project::discover(root)?;
    let status = Command::new(nginx_binary()?)
        .arg("-p")
        .arg(&project.root)
        .arg("-c")
        .arg(".hoplite/conf/nginx.conf")
        .arg("-e")
        .arg(".hoplite/error.log")
        .arg("-s")
        .arg(signal)
        .status()
        .map_err(|error| format!("cannot signal Hoplite Nginx: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Nginx {signal} failed with {status}"))
    }
}

fn nginx_distribution() -> &'static str {
    if cfg!(feature = "embedded-nginx") {
        "embedded"
    } else {
        "external"
    }
}

fn nginx_binary() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("HOPLITE_NGINX") {
        return Ok(path.into());
    }
    #[cfg(feature = "embedded-nginx")]
    return materialize_embedded_nginx();

    #[cfg(not(feature = "embedded-nginx"))]
    {
        if let Ok(executable) = env::current_exe() {
            if let Some(directory) = executable.parent() {
                let sibling = directory.join("nginx");
                if sibling.is_file() {
                    return Ok(sibling);
                }
            }
        }
        Ok(Path::new(env!("CARGO_MANIFEST_DIR")).join("target/hoplite/nginx/sbin/nginx"))
    }
}

#[cfg(feature = "embedded-nginx")]
fn materialize_embedded_nginx() -> Result<PathBuf, String> {
    let cache = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches")))
        .unwrap_or_else(env::temp_dir)
        .join("hoplite")
        .join(env!("CARGO_PKG_VERSION"));
    let path = cache.join(format!("nginx-{NGINX_VERSION}"));
    if fs::read(&path)
        .map(|contents| contents == EMBEDDED_NGINX)
        .unwrap_or(false)
    {
        return Ok(path);
    }
    fs::create_dir_all(&cache).map_err(io)?;
    let temporary = cache.join(format!(".nginx-{}.tmp", process::id()));
    fs::write(&temporary, EMBEDDED_NGINX).map_err(io)?;
    #[cfg(unix)]
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700)).map_err(io)?;
    fs::rename(&temporary, &path).map_err(io)?;
    Ok(path)
}

fn source_files(project: &Project) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for path in &project.source_paths {
        collect_hal(&project.root.join(path), &mut files)?;
    }
    files.sort();
    Ok(files)
}

fn collect_hal(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.file_name().and_then(|value| value.to_str()) == Some(".hoplite") {
        return Ok(());
    }
    if path.is_file() {
        if path.extension().and_then(|value| value.to_str()) == Some("hal") {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    let entries = fs::read_dir(path).map_err(io)?;
    for entry in entries {
        let entry = entry.map_err(io)?;
        let path = entry.path();
        if path.is_dir() {
            collect_hal(&path, files)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("hal") {
            files.push(path);
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct ApplicationModule {
    namespace: String,
    requires: Vec<String>,
    source: String,
}

fn namespace_declaration(forms: &[Form]) -> Result<(&Form, String, Vec<String>), String> {
    let form = forms
        .iter()
        .find(|form| matches!(form, Form::List(items) if matches!(items.first(), Some(Form::Symbol(operator)) if operator == "ns")))
        .ok_or("HAL module is missing ns form")?;
    let Form::List(items) = form else {
        unreachable!()
    };
    let namespace = match items.get(1) {
        Some(Form::Symbol(namespace)) if !namespace.contains('/') => namespace.clone(),
        _ => return Err("HAL module ns name must be an unqualified symbol".into()),
    };
    let mut requires = Vec::new();
    for clause in &items[2..] {
        let Form::List(values) = clause else { continue };
        if !matches!(values.first(), Some(Form::Keyword(keyword)) if keyword == "require") {
            continue;
        }
        for entry in &values[1..] {
            if let Form::Vector(require) = entry {
                if let Some(Form::Symbol(dependency)) = require.first() {
                    requires.push(dependency.clone());
                }
            }
        }
    }
    requires.sort();
    requires.dedup();
    Ok((form, namespace, requires))
}

fn application_module(source: &str) -> Result<ApplicationModule, String> {
    let forms = parse_forms(source)?;
    let (_, namespace, requires) = namespace_declaration(&forms)?;
    Ok(ApplicationModule {
        namespace,
        requires,
        source: source.to_owned(),
    })
}

fn visit_application_module(
    namespace: &str,
    modules: &HashMap<String, ApplicationModule>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    ordered: &mut Vec<ApplicationModule>,
) -> Result<(), String> {
    if visited.contains(namespace) {
        return Ok(());
    }
    if !visiting.insert(namespace.to_owned()) {
        return Err(format!(
            "Hoplite application namespace cycle includes {namespace}"
        ));
    }
    let module = modules
        .get(namespace)
        .ok_or_else(|| format!("missing Hoplite application namespace {namespace}"))?;
    for dependency in &module.requires {
        if modules.contains_key(dependency) {
            visit_application_module(dependency, modules, visiting, visited, ordered)?;
        }
    }
    visiting.remove(namespace);
    visited.insert(namespace.to_owned());
    ordered.push(module.clone());
    Ok(())
}

fn application_modules(files: &[PathBuf]) -> Result<Vec<ApplicationModule>, String> {
    let mut modules = HashMap::new();
    let builtins = vec![
        app::CORE_SOURCE,
        app::HOST_SOURCE,
        app::INTERNAL_SOURCE,
        app::RAW_SOURCE,
        app::RESPONSE_SOURCE,
    ];
    for source in builtins {
        let module = application_module(source)?;
        modules.insert(module.namespace.clone(), module);
    }
    for path in files {
        let source = fs::read_to_string(path).map_err(io)?;
        let module =
            application_module(&source).map_err(|error| format!("{}: {error}", path.display()))?;
        if modules.insert(module.namespace.clone(), module).is_some() {
            return Err(format!(
                "duplicate Hoplite application namespace in {}",
                path.display()
            ));
        }
    }
    let mut namespaces = modules.keys().cloned().collect::<Vec<_>>();
    namespaces.sort();
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    let mut ordered = Vec::with_capacity(modules.len());
    for namespace in namespaces {
        visit_application_module(
            &namespace,
            &modules,
            &mut visiting,
            &mut visited,
            &mut ordered,
        )?;
    }
    Ok(ordered)
}

fn runtime_application_module(module: &ApplicationModule) -> Result<String, String> {
    let forms = parse_forms(&module.source)?;
    Ok(forms
        .into_iter()
        .filter(|form| !application_definition(form))
        .map(|form| render_form(&form))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn runtime_application_modules(modules: &[ApplicationModule]) -> Result<String, String> {
    modules
        .iter()
        .map(runtime_application_module)
        .collect::<Result<Vec<_>, _>>()
        .map(|sources| sources.join("\n\n"))
}

fn compile_application_modules(modules: &[ApplicationModule]) -> Result<Vec<u8>, String> {
    let mut runtime = Runtime::new();
    app::register_resources(&mut runtime);
    for module in modules {
        runtime.register_resource(&module.namespace, &module.source);
    }
    runtime.eval_native(
        "(ns hoplite.raw.native)\n\
         (defn respond [_exchange _status _headers _body] nil)\n\
         (defn start [_exchange _status _headers] nil)\n\
         (defn write [_exchange _chunk] nil)\n\
         (defn finish [_exchange] nil)",
    )?;
    let mut artifacts = Vec::with_capacity(modules.len());
    for module in modules {
        let source = runtime_application_module(module)?;
        let forms = parse_forms(&source)?;
        let (declaration, namespace, _) = namespace_declaration(&forms)?;
        let declaration = render_form(declaration);
        runtime
            .eval_native(&declaration)
            .map_err(|error| format!("{namespace}: cannot load namespace: {error}"))?;
        let body = forms
            .into_iter()
            .filter(|form| !matches!(form, Form::List(items) if matches!(items.first(), Some(Form::Symbol(operator)) if operator == "ns")))
            .map(|form| render_form(&form))
            .collect::<Vec<_>>()
            .join("\n");
        let artifact = catch_unwind(AssertUnwindSafe(|| {
            runtime.compile_bytecode_artifact(&body)
        }))
        .map_err(|_| format!("{namespace}: bytecode compiler panicked"))?
        .map_err(|error| format!("{namespace}: {error}"))?;
        runtime
            .eval_bytecode_artifact(&artifact)
            .map_err(|error| format!("{namespace}: cannot load bytecode: {error}"))?;
        artifacts.push(BytecodeBundleModule {
            resource: namespace,
            namespace_form: declaration,
            source_digest: Sha256::digest(module.source.as_bytes()).into(),
            dependencies: module.requires.clone(),
            eager: true,
            artifact,
        });
    }
    vm::encode_bytecode_bundle(&artifacts)
}

fn application_definition(form: &Form) -> bool {
    let Form::List(definition) = form else {
        return false;
    };
    if !matches!(definition.first(), Some(Form::Symbol(operator)) if operator == "def") {
        return false;
    }
    let Some(Form::List(expression)) = definition.get(2) else {
        return false;
    };
    matches!(expression.first(), Some(Form::Symbol(operator))
        if matches!(operator.as_str(), "h/app" | "hoplite.core/app" | "internal/config" | "hoplite.internal/config"))
}

fn render_form(form: &Form) -> String {
    match form {
        Form::Metadata(metadata, value) => {
            format!("^{} {}", render_form(metadata), render_form(value))
        }
        Form::Tagged(tag, value) => format!("#{tag}{}", render_form(value)),
        Form::List(values) => render_sequence(values, "(", ")"),
        Form::Vector(values) => render_sequence(values, "[", "]"),
        Form::Set(values) => render_sequence(values, "#{", "}"),
        Form::Map(entries) => {
            let values = entries
                .iter()
                .flat_map(|(key, value)| [render_form(key), render_form(value)])
                .collect::<Vec<_>>();
            format!("{{{}}}", values.join(" "))
        }
        _ => form.to_string(),
    }
}

fn render_sequence(values: &[Form], prefix: &str, suffix: &str) -> String {
    format!(
        "{prefix}{}{suffix}",
        values.iter().map(render_form).collect::<Vec<_>>().join(" ")
    )
}

fn configured_ca_bundle() -> Result<Option<PathBuf>, String> {
    let configured = env::var_os("HOPLITE_CA_BUNDLE").or_else(|| env::var_os("SSL_CERT_FILE"));
    let Some(path) = configured.map(PathBuf::from) else {
        return Ok(None);
    };
    if !path.is_file() {
        return Err(format!(
            "configured proxy CA bundle does not exist: {}",
            path.display()
        ));
    }
    validate_ca_bundle(path).map(Some)
}

fn validate_ca_bundle(path: PathBuf) -> Result<PathBuf, String> {
    let path = path.canonicalize().unwrap_or(path);
    let value = path.display().to_string();
    if unsafe_nginx(&value) || value.contains(char::is_whitespace) {
        return Err(format!(
            "proxy CA bundle path is not safe for generated Nginx configuration: {value:?}"
        ));
    }
    Ok(path)
}

fn system_ca_bundle() -> Result<PathBuf, String> {
    if let Some(path) = configured_ca_bundle()? {
        return Ok(path);
    }
    for candidate in [
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/pki/tls/certs/ca-bundle.crt",
        "/etc/ssl/ca-bundle.pem",
        "/etc/ssl/cert.pem",
        "/opt/homebrew/etc/openssl@3/cert.pem",
        "/usr/local/etc/openssl@3/cert.pem",
    ] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return validate_ca_bundle(path);
        }
    }
    Err("a secure static upstream requires a trusted CA bundle; set HOPLITE_CA_BUNDLE to a PEM certificate bundle".into())
}

fn proxy_ca_bundle(config: &app::Config) -> Result<Option<PathBuf>, String> {
    if config
        .apps
        .iter()
        .flat_map(|application| &application.proxies)
        .any(|proxy| proxy.secure)
    {
        system_ca_bundle().map(Some)
    } else {
        Ok(None)
    }
}

fn nginx_proxy_location(proxy: &app::Proxy, trusted_ca: Option<&Path>) -> Result<String, String> {
    for (label, value) in [
        ("path", proxy.path.as_str()),
        ("upstream", proxy.upstream.as_str()),
        ("authority", proxy.authority.as_str()),
        ("server name", proxy.server_name.as_str()),
    ] {
        if unsafe_nginx(value) || value.contains(char::is_whitespace) {
            return Err(format!("invalid proxy {label} {value:?}"));
        }
    }
    let tls = if proxy.secure {
        let trusted_ca = trusted_ca.ok_or("secure proxy is missing a trusted CA bundle")?;
        let trusted_ca = trusted_ca.display().to_string();
        if unsafe_nginx(&trusted_ca) || trusted_ca.contains(char::is_whitespace) {
            return Err(format!("invalid proxy CA bundle path {trusted_ca:?}"));
        }
        format!(
            "            proxy_ssl_server_name on;\n            proxy_ssl_name {};\n            proxy_ssl_verify on;\n            proxy_ssl_verify_depth 5;\n            proxy_ssl_trusted_certificate {};\n",
            proxy.server_name, trusted_ca
        )
    } else {
        String::new()
    };
    Ok(format!(
        "        location ^~ {} {{\n            proxy_http_version 1.1;\n{}            proxy_set_header Host {};\n            proxy_set_header Connection \"\";\n            proxy_set_header Cookie \"\";\n            proxy_set_header Origin \"\";\n            proxy_set_header Referer \"\";\n            proxy_set_header X-Forwarded-For \"\";\n            proxy_set_header X-Forwarded-Host \"\";\n            proxy_set_header X-Forwarded-Proto \"\";\n            proxy_pass_request_headers on;\n            proxy_redirect off;\n            proxy_pass {};\n        }}\n",
        proxy.path, tls, proxy.authority, proxy.upstream
    ))
}

fn nginx_app_configuration(project: &Project, config: &app::Config) -> Result<String, String> {
    let bootstrap = project
        .root
        .join(".hoplite/app.hbx")
        .canonicalize()
        .unwrap_or_else(|_| project.root.join(".hoplite/app.hbx"));
    let manifest = project
        .root
        .join(".hoplite/apps.hta")
        .canonicalize()
        .unwrap_or_else(|_| project.root.join(".hoplite/apps.hta"));
    let trusted_ca = proxy_ca_bundle(config)?;
    let mut servers = String::new();
    for application in &config.apps {
        if application
            .hostnames
            .iter()
            .any(|value| unsafe_nginx(value) || value.contains(char::is_whitespace))
        {
            return Err(format!("invalid hostname in app {:?}", application.name));
        }
        let names = if application.hostnames.is_empty() {
            "_".to_owned()
        } else {
            application.hostnames.join(" ")
        };
        let mut locations = String::new();
        for proxy in &application.proxies {
            locations.push_str(&nginx_proxy_location(proxy, trusted_ca.as_deref())?);
        }
        let request_body = application
            .request_body
            .as_ref()
            .map(|policy| {
                format!(
                    "            client_max_body_size {};\n            hoplite_request_body on;\n            hoplite_request_body_max {};\n            hoplite_request_body_chunk {};\n",
                    policy.max_bytes, policy.max_bytes, policy.max_chunk_bytes
                )
            })
            .unwrap_or_default();
        locations.push_str(&format!(
            "        location / {{\n{}            hoplite_app {};\n        }}\n",
            request_body, application.id
        ));
        servers.push_str(&format!(
            "    server {{\n        listen {};\n        server_name {};\n{}    }}\n",
            application.port, names, locations
        ));
    }
    Ok(format!(
        "worker_processes {};\npid .hoplite/nginx.pid;\nerror_log .hoplite/error.log;\nevents {{}}\nhttp {{\n    access_log .hoplite/access.log;\n    hoplite_bootstrap {};\n    hoplite_manifest {};\n{} }}\n",
        config.workers,
        bootstrap.display(),
        manifest.display(),
        servers
    ))
}

fn unsafe_nginx(value: &str) -> bool {
    value.chars().any(|character| {
        matches!(
            character,
            ';' | '{' | '}' | '$' | '\\' | '"' | '\'' | '\n' | '\r' | '\0'
        )
    })
}

fn io(error: std::io::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(source: &str) -> ApplicationModule {
        application_module(source).expect("valid HAL module")
    }

    #[test]
    fn application_bundle_excludes_retired_value_contract() {
        let modules = application_modules(&[]).unwrap();
        assert!(!modules
            .iter()
            .any(|module| module.namespace == "hoplite.value"));
    }

    #[test]
    fn orders_application_modules_before_their_dependents() {
        let dependency = module("(ns example.dependency) (defn answer [] 42)");
        let application = module(
            "(ns example.application (:require [example.dependency :as dependency])) \
             (defn answer [] (dependency/answer))",
        );
        let modules = HashMap::from([
            (application.namespace.clone(), application),
            (dependency.namespace.clone(), dependency),
        ]);
        let mut ordered = Vec::new();
        visit_application_module(
            "example.application",
            &modules,
            &mut HashSet::new(),
            &mut HashSet::new(),
            &mut ordered,
        )
        .unwrap();
        assert_eq!(
            ordered
                .iter()
                .map(|module| module.namespace.as_str())
                .collect::<Vec<_>>(),
            ["example.dependency", "example.application"]
        );
    }

    #[test]
    fn compiles_hara_bytecode_and_wraps_the_hoplite_application_contract() {
        let modules = [
            module("(ns example.dependency) (defn answer [] 42)"),
            module(
                "(ns example.application (:require [example.dependency :as dependency])) \
                 (defn answer [] (dependency/answer))",
            ),
        ];
        let hbx0 = compile_application_modules(&modules).unwrap();
        assert_eq!(&hbx0[..4], b"HBX0");

        let manifest = b"exact-app-manifest";
        let bundle = application_bundle::encode(manifest, &hbx0).unwrap();
        assert_eq!(&bundle[..4], application_bundle::MAGIC);
        assert_eq!(
            application_bundle::decode(&bundle, manifest)
                .unwrap()
                .bytecode(),
            hbx0
        );
    }

    #[test]
    fn rejects_application_namespace_cycles() {
        let first = module("(ns example.first (:require [example.second :as second]))");
        let second = module("(ns example.second (:require [example.first :as first]))");
        let modules = HashMap::from([
            (first.namespace.clone(), first),
            (second.namespace.clone(), second),
        ]);
        let error = visit_application_module(
            "example.first",
            &modules,
            &mut HashSet::new(),
            &mut HashSet::new(),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(error.contains("namespace cycle"), "{error}");
    }

    #[test]
    fn rejects_nginx_configuration_injection() {
        assert!(unsafe_nginx("example.org; return 200"));
        assert!(unsafe_nginx("https://example.org/$request_uri"));
    }

    #[test]
    fn renders_verified_fixed_proxy_locations_without_forwarding_local_ambient_authority() {
        let proxy = app::Proxy {
            path: "/space/".into(),
            upstream: "https://greenways.space/beacon/v1/".into(),
            authority: "greenways.space".into(),
            server_name: "greenways.space".into(),
            secure: true,
        };
        let location = nginx_proxy_location(
            &proxy,
            Some(Path::new("/etc/ssl/certs/ca-certificates.crt")),
        )
        .unwrap();
        assert!(location.contains("location ^~ /space/"));
        assert!(location.contains("proxy_ssl_name greenways.space;"));
        assert!(location.contains("proxy_ssl_verify on;"));
        assert!(location.contains("proxy_ssl_verify_depth 5;"));
        assert!(
            location.contains("proxy_ssl_trusted_certificate /etc/ssl/certs/ca-certificates.crt;")
        );
        assert!(location.contains("proxy_set_header Host greenways.space;"));
        assert!(location.contains("proxy_set_header Cookie \"\";"));
        assert!(location.contains("proxy_set_header Origin \"\";"));
        assert!(location.contains("proxy_pass https://greenways.space/beacon/v1/;"));
        assert!(!location.contains("$request_uri"));
    }

    #[test]
    fn secure_proxy_requires_a_ca_bundle() {
        let proxy = app::Proxy {
            path: "/space/".into(),
            upstream: "https://greenways.space/beacon/v1/".into(),
            authority: "greenways.space".into(),
            server_name: "greenways.space".into(),
            secure: true,
        };
        assert!(nginx_proxy_location(&proxy, None)
            .unwrap_err()
            .contains("trusted CA bundle"));
    }

    #[test]
    fn creates_safe_launchd_definition() {
        let label = launchd_label("example/web api");
        assert_eq!(label, "org.hara.hoplite.example-web-api");
        let plist = launchd_plist(
            &label,
            Path::new("/Applications/Hara & Hoplite/hoplite"),
            Path::new("/tmp/example <web>"),
        );
        assert!(plist.contains("<string>serve</string>"));
        assert!(plist.contains("<string>foreground</string>"));
        assert!(plist.contains("Hara &amp; Hoplite"));
        assert!(plist.contains("example &lt;web&gt;"));
        assert!(plist.contains("<key>KeepAlive</key><true/>"));
    }

    #[test]
    fn verifies_a_built_application_without_source_execution() {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "hoplite-verify-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let output = root.join(".hoplite");
        std::fs::create_dir_all(&output).unwrap();
        let manifest = hta::encode(&hara_wasm::core::Value::Map(Default::default())).unwrap();
        let bundle = application_bundle::encode(&manifest, b"HBX0verification").unwrap();
        std::fs::write(output.join("app.hbx"), &bundle).unwrap();
        std::fs::write(output.join("apps.hta"), &manifest).unwrap();

        let (bundle_path, manifest_path) = verification_paths(&root);
        let verified = verify_application_bundle(&bundle_path, &manifest_path).unwrap();
        assert_eq!(verified.bundle_bytes, bundle.len());
        assert_eq!(verified.manifest_bytes, manifest.len());
        assert_eq!(verified.bytecode_bytes, b"HBX0verification".len());
        assert_eq!(lower_hex(&verified.manifest_digest).len(), 64);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn production_projection_removes_stale_development_source() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let output = std::env::temp_dir().join(format!(
            "hoplite-source-projection-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&output).unwrap();
        write_runtime_source_projection(&output, Some("(ns demo)")).unwrap();
        assert!(output.join("app.hal").is_file());
        write_runtime_source_projection(&output, None).unwrap();
        assert!(!output.join("app.hal").exists());
        fs::remove_dir_all(output).unwrap();
    }
}
