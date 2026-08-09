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

// Nginx deliberately removes inherited environment variables from workers
// unless each name is allowlisted in the main configuration context.  These
// names configure generic host providers during worker init; their values
// remain in the process environment and are never written into generated
// configuration files.
const TRUSTED_WORKER_ENVIRONMENT: &[&str] = &[
    "HOPLITE_HARA_BLOB_ROOT",
    "HOPLITE_HARA_BLOB_MAX_OBJECT_BYTES",
    "HOPLITE_HARA_BLOB_MAX_APPEND_BYTES",
    "HOPLITE_HARA_BLOB_MAX_SOURCE_CHUNK_BYTES",
    "HOPLITE_HARA_BLOB_MAX_STAGING_KEY_BYTES",
    "HOPLITE_HARA_BLOB_MAX_MEDIA_TYPE_BYTES",
    "HOPLITE_HARA_BLOB_MAX_STAGING_ENTRIES",
    "HOPLITE_HARA_BLOB_MAX_OBJECTS",
    "HOPLITE_HARA_STORE_PATH",
    "HOPLITE_HARA_STORE_MAX_VALUE_BYTES",
    "HOPLITE_HARA_STORE_MAX_RECEIPT_BYTES",
];

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
    let global_directives = nginx_global_directives();

    #[cfg(unix)]
    {
        let error = Command::new(&nginx)
            .arg("-p")
            .arg(&root)
            .arg("-c")
            .arg(&configuration)
            .arg("-e")
            .arg(".hoplite/error.log")
            .arg("-g")
            .arg(&global_directives)
            .exec();
        Err(format!("cannot exec {}: {error}", nginx.display()))
    }

    #[cfg(not(unix))]
    {
        let _ = nginx;
        let _ = global_directives;
        Err("hoplite-server supports macOS and Linux".into())
    }
}

fn nginx_global_directives() -> String {
    let mut directives = String::from("daemon off;");
    for name in TRUSTED_WORKER_ENVIRONMENT {
        directives.push_str(" env ");
        directives.push_str(name);
        directives.push(';');
    }
    directives
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

    let source = fs::read_to_string(&source_path).map_err(io)?;
    let rendered = render_worker_configuration(&source, workers)?;

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

fn render_worker_configuration(source: &str, workers: &str) -> Result<String, String> {
    validate_workers(workers)?;
    let start = source
        .find("worker_processes ")
        .ok_or("generated Nginx configuration has no worker_processes directive")?;
    let end = source[start..]
        .find(';')
        .map(|offset| start + offset + 1)
        .ok_or("generated Nginx worker_processes directive is incomplete")?;
    let mut rendered = source.to_owned();
    rendered.replace_range(start..end, &format!("worker_processes {workers};"));
    Ok(rendered)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_worker_overrides() {
        assert!(validate_workers("auto").is_ok());
        assert!(validate_workers("1").is_ok());
        assert!(validate_workers("64").is_ok());
        assert!(validate_workers("0").is_err());
        assert!(validate_workers("many").is_err());
    }

    #[test]
    fn preserves_only_allowlisted_provider_environment() {
        let directives = nginx_global_directives();
        assert!(directives.starts_with("daemon off;"));
        assert_eq!(
            directives.matches(" env ").count(),
            TRUSTED_WORKER_ENVIRONMENT.len()
        );
        for name in TRUSTED_WORKER_ENVIRONMENT {
            assert!(directives.contains(&format!(" env {name};")));
        }
        assert!(!directives.contains("SECRET"));
        assert!(!directives.contains('='));
    }

    #[test]
    fn rewrites_generated_worker_directive() {
        let source = "worker_processes 4;\npid .hoplite/nginx.pid;\n";
        assert_eq!(
            render_worker_configuration(source, "auto").unwrap(),
            "worker_processes auto;\npid .hoplite/nginx.pid;\n"
        );
    }

    #[test]
    fn rejects_missing_worker_directive() {
        assert!(render_worker_configuration("events {}\n", "auto").is_err());
    }
}
