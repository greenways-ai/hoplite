from pathlib import Path

root = Path(__file__).resolve().parents[2]
path = root / "core/src/doctor.rs"
source = path.read_text()

source = source.replace(
    "use std::process::Command;",
    "use std::process::{Command, Output};",
    1,
)

old_nginx = '''            match probe {
                Ok(Some(output)) if output.status.success() => checks.push(
                    Check::pass(
                        "nginx-runtime",
                        format!(
                            "Nginx {} ({}) is executable",
                            crate::NGINX_VERSION,
                            crate::nginx_distribution()
                        ),
                    )
                    .path(Some(&path), show_paths),
                ),
                Ok(Some(_)) => checks.push(
                    Check::fail("nginx-runtime", "Nginx version probing failed")
                        .detail("hoplite/doctor-nginx-probe-failed")
                        .path(Some(&path), show_paths),
                ),
                Ok(None) => checks.push(
                    Check::fail("nginx-runtime", "the Nginx binary is not executable")
                        .detail("hoplite/doctor-nginx-not-executable")
                        .path(Some(&path), show_paths),
                ),
                Err(_) => checks.push(
                    Check::fail("nginx-runtime", "the Nginx binary could not be executed")
                        .detail("hoplite/doctor-nginx-exec-failed")
                        .path(Some(&path), show_paths),
                ),
            }
'''
new_nginx = '''            match probe {
                Ok(Some(output))
                    if output.status.success()
                        && output_contains(
                            &output,
                            &format!("nginx/{}", crate::NGINX_VERSION),
                        ) =>
                {
                    checks.push(
                        Check::pass(
                            "nginx-runtime",
                            format!(
                                "Nginx {} ({}) is executable",
                                crate::NGINX_VERSION,
                                crate::nginx_distribution()
                            ),
                        )
                        .path(Some(&path), show_paths),
                    )
                }
                Ok(Some(output)) if output.status.success() => checks.push(
                    Check::fail(
                        "nginx-runtime",
                        format!(
                            "the selected Nginx does not report required version {}",
                            crate::NGINX_VERSION
                        ),
                    )
                    .detail("hoplite/doctor-nginx-version-incompatible")
                    .path(Some(&path), show_paths),
                ),
                Ok(Some(_)) => checks.push(
                    Check::fail("nginx-runtime", "Nginx version probing failed")
                        .detail("hoplite/doctor-nginx-probe-failed")
                        .path(Some(&path), show_paths),
                ),
                Ok(None) => checks.push(
                    Check::fail("nginx-runtime", "the Nginx binary is not executable")
                        .detail("hoplite/doctor-nginx-not-executable")
                        .path(Some(&path), show_paths),
                ),
                Err(_) => checks.push(
                    Check::fail("nginx-runtime", "the Nginx binary could not be executed")
                        .detail("hoplite/doctor-nginx-exec-failed")
                        .path(Some(&path), show_paths),
                ),
            }
'''
if source.count(old_nginx) != 1:
    raise SystemExit("unexpected Nginx probe block")
source = source.replace(old_nginx, new_nginx, 1)

old_server = '''    match env::current_exe().ok().and_then(|path| {
        path.parent()
            .map(|directory| directory.join("hoplite-server"))
    }) {
        Some(path) if path.is_file() && executable_file(&path) => checks.push(
            Check::pass(
                "hoplite-server",
                "the source-free production server executable is available",
            )
            .path(Some(&path), show_paths),
        ),
        Some(path) => checks.push(
            Check::warn(
                "hoplite-server",
                "the source-free production server executable is not beside hoplite",
            )
            .detail("hoplite/doctor-server-cli-not-found")
            .path(Some(&path), show_paths),
        ),
        None => checks.push(
            Check::warn(
                "hoplite-server",
                "the source-free production server executable could not be located",
            )
            .detail("hoplite/doctor-server-cli-unavailable"),
        ),
    }
'''
new_server = '''    match env::current_exe().ok().and_then(|path| {
        path.parent()
            .map(|directory| directory.join("hoplite-server"))
    }) {
        Some(path) if path.is_file() && executable_file(&path) => {
            match Command::new(&path).arg("version").output() {
                Ok(output)
                    if output.status.success()
                        && output_contains(
                            &output,
                            &format!("Hoplite server {}", env!("CARGO_PKG_VERSION")),
                        )
                        && output_contains(
                            &output,
                            &format!("Nginx {}", crate::NGINX_VERSION),
                        ) =>
                {
                    checks.push(
                        Check::pass(
                            "hoplite-server",
                            "the source-free production server executable is compatible",
                        )
                        .path(Some(&path), show_paths),
                    )
                }
                Ok(output) if output.status.success() => checks.push(
                    Check::fail(
                        "hoplite-server",
                        "the companion production server reports an incompatible identity",
                    )
                    .detail("hoplite/doctor-server-cli-incompatible")
                    .path(Some(&path), show_paths),
                ),
                Ok(_) => checks.push(
                    Check::fail(
                        "hoplite-server",
                        "the companion production server version probe failed",
                    )
                    .detail("hoplite/doctor-server-cli-probe-failed")
                    .path(Some(&path), show_paths),
                ),
                Err(_) => checks.push(
                    Check::fail(
                        "hoplite-server",
                        "the companion production server could not be executed",
                    )
                    .detail("hoplite/doctor-server-cli-exec-failed")
                    .path(Some(&path), show_paths),
                ),
            }
        }
        Some(path) => checks.push(
            Check::warn(
                "hoplite-server",
                "the source-free production server executable is not beside hoplite",
            )
            .detail("hoplite/doctor-server-cli-not-found")
            .path(Some(&path), show_paths),
        ),
        None => checks.push(
            Check::warn(
                "hoplite-server",
                "the source-free production server executable could not be located",
            )
            .detail("hoplite/doctor-server-cli-unavailable"),
        ),
    }
'''
if source.count(old_server) != 1:
    raise SystemExit("unexpected server probe block")
source = source.replace(old_server, new_server, 1)

helper_anchor = '''#[cfg(unix)]
fn executable_file(path: &Path) -> bool {'''
helper = '''fn output_contains(output: &Output, expected: &str) -> bool {
    bytes_contain(&output.stdout, expected.as_bytes())
        || bytes_contain(&output.stderr, expected.as_bytes())
}

fn bytes_contain(bytes: &[u8], expected: &[u8]) -> bool {
    !expected.is_empty() && bytes.windows(expected.len()).any(|window| window == expected)
}

#[cfg(unix)]
fn executable_file(path: &Path) -> bool {'''
if source.count(helper_anchor) != 1:
    raise SystemExit("unexpected executable helper anchor")
source = source.replace(helper_anchor, helper, 1)

unit_anchor = '''    #[test]
    fn project_checks_are_static_and_path_redacted() {'''
unit = '''    #[test]
    fn version_probes_require_exact_reported_identities() {
        assert!(bytes_contain(
            b"nginx version: nginx/1.30.4\\n",
            b"nginx/1.30.4"
        ));
        assert!(!bytes_contain(
            b"nginx version: nginx/1.28.0\\n",
            b"nginx/1.30.4"
        ));
        assert!(bytes_contain(
            b"Hoplite server 0.1.0\\nNginx 1.30.4 (embedded)\\n",
            b"Hoplite server 0.1.0"
        ));
    }

    #[test]
    fn project_checks_are_static_and_path_redacted() {'''
if source.count(unit_anchor) != 1:
    raise SystemExit("unexpected doctor unit-test anchor")
source = source.replace(unit_anchor, unit, 1)

path.write_text(source)
(root / ".github/workflows/harden-doctor-version-probes.yml").unlink()
Path(__file__).unlink()
