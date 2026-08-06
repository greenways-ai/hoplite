use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
#[cfg(unix)]
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process;
#[cfg(unix)]
use std::process::Command;

const NGINX_VERSION: &str = "1.30.4";

#[cfg(feature = "embedded-nginx")]
const EMBEDDED_NGINX: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/nginx/sbin/nginx"
));

fn main() {
    if let Err(error) = run(env::args().skip(1)) {
        eprintln!("hoplite-server: {error}");
        process::exit(1);
    }
}

fn run(arguments: impl IntoIterator<Item = String>) -> Result<(), String> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    if matches!(
        arguments.first().map(String::as_str),
        Some("version" | "--version" | "-V")
    ) {
        println!("Hoplite server {}", env!("CARGO_PKG_VERSION"));
        println!("Nginx {NGINX_VERSION} ({})", nginx_distribution());
        return Ok(());
    }
    if matches!(
        arguments.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        usage();
        return Ok(());
    }

    let mut project = None;
    let mut workers = env::var("HOPLITE_WORKERS")
        .ok()
        .filter(|value| !value.is_empty());
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--workers" => {
                index += 1;
                workers = Some(
                    arguments
                        .get(index)
                        .ok_or("--workers requires auto or a positive integer")?
                        .clone(),
                );
            }
            value if value.starts_with("--workers=") => {
                workers = Some(value["--workers=".len()..].to_owned());
            }
            value if value.starts_with('-') => return Err(format!("unknown option: {value}")),
            value if project.is_none() => project = Some(PathBuf::from(value)),
            value => return Err(format!("unexpected argument: {value}")),
        }
        index += 1;
    }

    let root = match project {
        Some(path) => path,
        None => env::current_dir().map_err(io)?,
    };
    run_server(&root, workers.as_deref())
}

fn usage() {
    println!("Hoplite production server");
    println!();
    println!("Usage:");
    println!("  hoplite-server [--workers auto|N] [PROJECT]");
    println!("  hoplite-server version");
    println!();
    println!("Build PROJECT first with `hoplite serve build --mode prod PROJECT`.");
    println!("HOPLITE_WORKERS provides the default worker override.");
}

fn run_server(root: &Path, workers: Option<&str>) -> Result<(), String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("cannot resolve project {}: {error}", root.display()))?;
    let configuration = runtime_configuration(&root, workers)?;
    let nginx = nginx_binary()?;

    #[cfg(unix)]
    {
        let error = Command::new(&nginx)
            .arg("-p")
            .arg(&root)
            .arg("-c")
            .arg(&configuration)
            .arg("-e")
            .arg(".hoplite/error.log")
            .args(["-g", "daemon off;"])
            .exec();
        Err(format!("cannot exec {}: {error}", nginx.display()))
    }

    #[cfg(not(unix))]
    {
        let _ = nginx;
        Err("hoplite-server supports macOS and Linux".into())
    }
}

fn runtime_configuration(root: &Path, workers: Option<&str>) -> Result<PathBuf, String> {
    let source_path = root.join(".hoplite/conf/nginx.conf");
    if !source_path.is_file() {
        return Err(format!(
            "{} does not exist; build the project with `hoplite serve build --mode prod {}`",
            source_path.display(),
            root.display()
        ));
    }
    let Some(workers) = workers else {
        return Ok(source_path);
    };
    validate_workers(workers)?;

    let source = fs::read_to_string(&source_path).map_err(io)?;
    let start = source
        .find("worker_processes ")
        .ok_or("generated Nginx configuration has no worker_processes directive")?;
    let end = source[start..]
        .find(';')
        .map(|offset| start + offset + 1)
        .ok_or("generated Nginx worker_processes directive is incomplete")?;
    let mut rendered = source;
    rendered.replace_range(start..end, &format!("worker_processes {workers};"));

    let mut identity = DefaultHasher::new();
    root.hash(&mut identity);
    workers.hash(&mut identity);
    let directory = server_cache_root()
        .join("hoplite-server")
        .join(env!("CARGO_PKG_VERSION"))
        .join("config");
    let path = directory.join(format!("{:016x}.conf", identity.finish()));
    if fs::read_to_string(&path)
        .map(|current| current == rendered)
        .unwrap_or(false)
    {
        return Ok(path);
    }

    fs::create_dir_all(&directory).map_err(io)?;
    let temporary = directory.join(format!(".config-{}.tmp", process::id()));
    fs::write(&temporary, rendered).map_err(io)?;
    fs::rename(&temporary, &path).map_err(io)?;
    Ok(path)
}

fn validate_workers(value: &str) -> Result<(), String> {
    if value == "auto" {
        return Ok(());
    }
    if value
        .parse::<usize>()
        .ok()
        .is_some_and(|workers| workers > 0)
    {
        Ok(())
    } else {
        Err(format!(
            "worker override must be auto or a positive integer; got {value:?}"
        ))
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
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "HOPLITE_NGINX does not name a file: {}",
            path.display()
        ));
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
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/nginx/sbin/nginx");
        if path.is_file() {
            Ok(path)
        } else {
            Err(format!(
                "cannot find Nginx at {}; set HOPLITE_NGINX or build with embedded-nginx",
                path.display()
            ))
        }
    }
}

#[cfg(feature = "embedded-nginx")]
fn materialize_embedded_nginx() -> Result<PathBuf, String> {
    let cache = server_cache_root()
        .join("hoplite-server")
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

fn server_cache_root() -> PathBuf {
    if let Some(path) = env::var_os("HOPLITE_SERVER_CACHE") {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(path);
    }
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        #[cfg(target_os = "macos")]
        return home.join("Library/Caches");
        #[cfg(not(target_os = "macos"))]
        return home.join(".cache");
    }
    env::temp_dir()
}

fn io(error: std::io::Error) -> String {
    error.to_string()
}
