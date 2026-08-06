use std::env;
#[cfg(feature = "embedded-nginx")]
use std::fs;
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
    let mut arguments = arguments.into_iter();
    let first = arguments.next();
    match first.as_deref() {
        Some("version" | "--version" | "-V") => {
            println!("Hoplite server {}", env!("CARGO_PKG_VERSION"));
            println!("Nginx {NGINX_VERSION} ({})", nginx_distribution());
            Ok(())
        }
        Some("help" | "--help" | "-h") => {
            usage();
            Ok(())
        }
        Some(value) if value.starts_with('-') => Err(format!("unknown option: {value}")),
        project => {
            if let Some(argument) = arguments.next() {
                return Err(format!("unexpected argument: {argument}"));
            }
            let root = match project {
                Some(path) => PathBuf::from(path),
                None => env::current_dir().map_err(io)?,
            };
            run_server(&root)
        }
    }
}

fn usage() {
    println!("Hoplite production server");
    println!();
    println!("Usage:");
    println!("  hoplite-server [PROJECT]");
    println!("  hoplite-server version");
    println!();
    println!("Build PROJECT first with `hoplite serve build --mode prod PROJECT`.");
}

fn run_server(root: &Path) -> Result<(), String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("cannot resolve project {}: {error}", root.display()))?;
    let configuration = root.join(".hoplite/conf/nginx.conf");
    if !configuration.is_file() {
        return Err(format!(
            "{} does not exist; build the project with `hoplite serve build --mode prod {}`",
            configuration.display(),
            root.display()
        ));
    }
    let nginx = nginx_binary()?;

    #[cfg(unix)]
    {
        let error = Command::new(&nginx)
            .arg("-p")
            .arg(&root)
            .arg("-c")
            .arg(".hoplite/conf/nginx.conf")
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

#[cfg(feature = "embedded-nginx")]
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
