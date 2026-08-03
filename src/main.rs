use hara_wasm::kernel::{parse_forms, Form};
use hara_wasm::project::{self, Project};
use hara_wasm::Runtime;
use std::env;
use std::fs;
#[cfg(all(unix, feature = "embedded-nginx"))]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::thread;
use std::time::Duration;

mod app;
mod auth;
mod dev_console;
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
        Some("auth") => run_auth_command(&arguments[1..])?,
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
    println!("  hoplite auth [init|enroll|status] [OPTIONS]");
    println!("  hoplite serve [start|stop|reload|status|build|check] [PROJECT]");
    println!("  hoplite version");
}

fn run_auth_command(arguments: &[String]) -> Result<(), String> {
    if matches!(
        arguments.first().map(String::as_str),
        Some("--help" | "-h" | "help")
    ) {
        auth_usage();
        return Ok(());
    }
    let action = arguments.first().map(String::as_str).unwrap_or("status");
    match action {
        "init" => {
            let root = arguments
                .get(1)
                .map(PathBuf::from)
                .unwrap_or(env::current_dir().map_err(io)?);
            let mut store = auth::Store::open(auth_store_path(&root))?;
            match store.initialize()? {
                Some(token) => {
                    println!("Hoplite management authentication initialized.");
                    println!("Bootstrap token (expires in 15 minutes): {token}");
                    println!("Enroll the first administrator with:");
                    println!(
                        "  hoplite auth enroll {token} <ED25519_PUBLIC_KEY_HEX> {}",
                        root.display()
                    );
                }
                None => println!("Hoplite management authentication is already initialized."),
            }
        }
        "enroll" => {
            let token = arguments
                .get(1)
                .ok_or("auth enroll requires a bootstrap token")?;
            let public_key = arguments
                .get(2)
                .ok_or("auth enroll requires an Ed25519 public key in hexadecimal")?;
            let root = arguments
                .get(3)
                .map(PathBuf::from)
                .unwrap_or(env::current_dir().map_err(io)?);
            let mut store = auth::Store::open(auth_store_path(&root))?;
            let principal = store.enroll_management_device(token, public_key)?;
            println!(
                "enrolled management user {} with device {}",
                principal.id, principal.device_id
            );
        }
        "status" => {
            let root = arguments
                .get(1)
                .map(PathBuf::from)
                .unwrap_or(env::current_dir().map_err(io)?);
            let path = auth_store_path(&root);
            if path.is_file() {
                let store = auth::Store::open(&path)?;
                println!("authentication store: {}", store.path().display());
            } else {
                println!("authentication is not initialized; run `hoplite auth init`");
            }
        }
        value => return Err(format!("unknown auth command: {value}")),
    }
    Ok(())
}

fn auth_store_path(root: &Path) -> PathBuf {
    env::var_os("HOPLITE_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".hoplite"))
        .join("control.db")
}

fn auth_usage() {
    println!("Hoplite-owned authentication");
    println!();
    println!("Usage:");
    println!("  hoplite auth init [PROJECT]");
    println!("  hoplite auth enroll BOOTSTRAP_TOKEN ED25519_PUBLIC_KEY_HEX [PROJECT]");
    println!("  hoplite auth status [PROJECT]");
    println!();
    println!("Set HOPLITE_STATE_DIR to place control.db outside PROJECT/.hoplite.");
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
    let source = bundle_sources(&sources)?;
    let runtime_source = runtime_application_source(&source)?;
    compile_application(&runtime_source)
        .map_err(|error| format!("Hoplite bytecode compilation failed: {error}"))?;
    app::load(&project, settings.profile.as_deref(), settings.production)?;
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

fn build(root: &Path, settings: &BuildSettings) -> Result<PathBuf, String> {
    let project = check(root, settings)?;
    let sources = source_files(&project)?;
    let source = bundle_sources(&sources)?;
    let runtime_source = runtime_application_source(&source)?;
    let bytecode = compile_application(&runtime_source)
        .map_err(|error| format!("Hoplite bytecode compilation failed: {error}"))?;
    let app_config = app::load(&project, settings.profile.as_deref(), settings.production)?;
    let platform_config = platform::load(&project, settings.profile.as_deref())?;
    let output = project.root.join(".hoplite");
    let configuration = output.join("conf");
    fs::create_dir_all(&configuration).map_err(io)?;
    fs::write(output.join("app.hal"), &runtime_source).map_err(io)?;
    fs::write(output.join("app.hbc"), bytecode).map_err(io)?;
    fs::write(output.join("apps.hta"), app::manifest(&app_config)?).map_err(io)?;
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
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        Err(format!("cannot exec Hoplite Nginx: {error}"))
    }
    #[cfg(not(unix))]
    {
        let exit = command
            .status()
            .map_err(|error| format!("cannot run Hoplite Nginx: {error}"))?;
        if exit.success() {
            Ok(())
        } else {
            Err(format!("Nginx exited with {exit}"))
        }
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

fn bundle_sources(files: &[PathBuf]) -> Result<String, String> {
    let mut source = String::new();
    for path in files {
        source.push_str(&format!(";; {}\n", path.display()));
        source.push_str(&fs::read_to_string(path).map_err(io)?);
        source.push_str("\n\n");
    }
    Ok(source)
}

fn compile_application(source: &str) -> Result<Vec<u8>, String> {
    let forms = parse_forms(source)?;
    let compilable = forms
        .into_iter()
        .filter(|form| {
            !matches!(form, Form::List(items) if matches!(items.first(), Some(Form::Symbol(operator)) if operator == "ns"))
        })
        .map(|form| render_form(&form))
        .collect::<Vec<_>>();
    let program = format!("(do {})", compilable.join("\n"));
    hara_wasm::compile_bytecode_artifact(&program)
}

fn runtime_application_source(source: &str) -> Result<String, String> {
    let forms = parse_forms(source)?;
    Ok(forms
        .into_iter()
        .filter(|form| !application_definition(form))
        .map(|form| render_form(&form))
        .collect::<Vec<_>>()
        .join("\n"))
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

fn nginx_app_configuration(project: &Project, config: &app::Config) -> Result<String, String> {
    let bootstrap = project
        .root
        .join(".hoplite/app.hal")
        .canonicalize()
        .unwrap_or_else(|_| project.root.join(".hoplite/app.hal"));
    let manifest = project
        .root
        .join(".hoplite/apps.hta")
        .canonicalize()
        .unwrap_or_else(|_| project.root.join(".hoplite/apps.hta"));
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
        servers.push_str(&format!(
            "    server {{\n        listen {};\n        server_name {};\n        location / {{\n            hoplite_app {};\n        }}\n    }}\n",
            application.port, names, application.id
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
    value
        .chars()
        .any(|character| matches!(character, ';' | '{' | '}' | '\n' | '\r' | '\0'))
}

fn io(error: std::io::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_nginx_configuration_injection() {
        assert!(unsafe_nginx("example.org; return 200"));
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
}
