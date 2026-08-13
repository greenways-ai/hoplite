use hara_wasm::project::{self, Project};
use serde_json::{json, Value as JsonValue};
use std::env;
use std::fmt::Write as _;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const FORMAT: &str = "hoplite.doctor/0-alpha";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

impl CheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Check {
    id: &'static str,
    status: CheckStatus,
    summary: String,
    detail: Option<String>,
    path: Option<String>,
}

impl Check {
    fn pass(id: &'static str, summary: impl Into<String>) -> Self {
        Self {
            id,
            status: CheckStatus::Pass,
            summary: summary.into(),
            detail: None,
            path: None,
        }
    }

    fn warn(id: &'static str, summary: impl Into<String>) -> Self {
        Self {
            id,
            status: CheckStatus::Warn,
            summary: summary.into(),
            detail: None,
            path: None,
        }
    }

    fn fail(id: &'static str, summary: impl Into<String>) -> Self {
        Self {
            id,
            status: CheckStatus::Fail,
            summary: summary.into(),
            detail: None,
            path: None,
        }
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn path(mut self, path: Option<&Path>, show_paths: bool) -> Self {
        self.path = show_paths
            .then(|| path.map(|value| value.display().to_string()))
            .flatten();
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Report {
    deep: bool,
    strict: bool,
    operating_system: &'static str,
    architecture: &'static str,
    nginx_distribution: &'static str,
    checks: Vec<Check>,
}

impl Report {
    fn failures(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == CheckStatus::Fail)
            .count()
    }

    fn warnings(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == CheckStatus::Warn)
            .count()
    }

    fn healthy(&self) -> bool {
        self.failures() == 0
    }

    fn complete(&self) -> bool {
        self.healthy() && self.warnings() == 0
    }
}

pub fn run(arguments: &[String]) -> Result<(), String> {
    if matches!(
        arguments.first().map(String::as_str),
        Some("--help" | "-h" | "help")
    ) {
        usage();
        return Ok(());
    }

    let mut target = None;
    let mut json_output = false;
    let mut show_paths = false;
    let mut deep = false;
    let mut strict = false;
    for argument in arguments {
        match argument.as_str() {
            "--json" => json_output = true,
            "--show-paths" => show_paths = true,
            "--deep" => deep = true,
            "--strict" => strict = true,
            value if value.starts_with('-') => {
                return Err(format!(
                    "hoplite/doctor-argument-invalid: unknown option {value:?}"
                ))
            }
            value if target.is_none() => target = Some(PathBuf::from(value)),
            value => {
                return Err(format!(
                    "hoplite/doctor-argument-invalid: unexpected target {value:?}"
                ))
            }
        }
    }

    let target = target.unwrap_or(
        env::current_dir()
            .map_err(|_| "hoplite/doctor-current-directory-unavailable".to_owned())?,
    );
    let report = collect(&target, show_paths, deep, strict);
    if json_output {
        print!("{}", render_json(&report));
    } else {
        print!("{}", render_human(&report));
    }

    let failures = report.failures();
    if failures != 0 {
        return Err(format!(
            "hoplite/doctor-unhealthy: {failures} required check{} failed",
            if failures == 1 { "" } else { "s" }
        ));
    }
    let warnings = report.warnings();
    if strict && warnings != 0 {
        return Err(format!(
            "hoplite/doctor-incomplete: {warnings} warning{} under --strict",
            if warnings == 1 { "" } else { "s" }
        ));
    }
    Ok(())
}

fn usage() {
    println!("Diagnose the local Hoplite runtime environment");
    println!();
    println!("usage: hoplite doctor [--json] [--show-paths] [--deep] [--strict] [PROJECT]");
    println!();
    println!("The default pass is read-only and does not evaluate application source.");
    println!("--deep performs the full Hoplite source compilation and app preflight.");
    println!("Paths are redacted unless --show-paths is supplied.");
}

fn collect(target: &Path, show_paths: bool, deep: bool, strict: bool) -> Report {
    let mut checks = Vec::new();
    collect_platform_checks(show_paths, &mut checks);
    let project = collect_project_checks(target, show_paths, &mut checks);
    if let Some(project) = project.as_ref() {
        collect_generated_application_checks(project, show_paths, &mut checks);
        if deep {
            collect_deep_preflight(project, &mut checks);
        }
    } else if deep {
        checks.push(
            Check::fail(
                "deep-preflight",
                "deep preflight requires a valid Hoplite project",
            )
            .detail("hoplite/doctor-deep-preflight-unavailable"),
        );
    }

    Report {
        deep,
        strict,
        operating_system: env::consts::OS,
        architecture: env::consts::ARCH,
        nginx_distribution: crate::nginx_distribution(),
        checks,
    }
}

fn collect_platform_checks(show_paths: bool, checks: &mut Vec<Check>) {
    if matches!(env::consts::OS, "linux" | "macos") {
        checks.push(Check::pass(
            "operating-system",
            format!("{} is a supported Hoplite host", env::consts::OS),
        ));
    } else {
        checks.push(
            Check::fail(
                "operating-system",
                format!("{} is not a supported Hoplite host", env::consts::OS),
            )
            .detail("hoplite/doctor-operating-system-unsupported"),
        );
    }

    match env::current_exe() {
        Ok(path) if path.is_file() => checks.push(
            Check::pass("hoplite-executable", "the Hoplite executable is readable")
                .path(Some(&path), show_paths),
        ),
        Ok(path) => checks.push(
            Check::fail(
                "hoplite-executable",
                "the current Hoplite executable is not a regular file",
            )
            .detail("hoplite/doctor-executable-not-regular")
            .path(Some(&path), show_paths),
        ),
        Err(_) => checks.push(
            Check::fail(
                "hoplite-executable",
                "the current Hoplite executable cannot be resolved",
            )
            .detail("hoplite/doctor-executable-unavailable"),
        ),
    }

    match crate::nginx_binary() {
        Ok(path) if path.is_file() => {
            let executable = executable_file(&path);
            let probe = executable
                .then(|| Command::new(&path).arg("-v").output())
                .transpose();
            match probe {
                Ok(Some(output))
                    if output.status.success()
                        && output_has_line(
                            &output,
                            &format!("nginx version: nginx/{}", crate::NGINX_VERSION),
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
        }
        Ok(path) => checks.push(
            Check::fail("nginx-runtime", "the configured Nginx binary is missing")
                .detail("hoplite/doctor-nginx-not-found")
                .path(Some(&path), show_paths),
        ),
        Err(_) => checks.push(
            Check::fail("nginx-runtime", "the Nginx runtime could not be resolved")
                .detail("hoplite/doctor-nginx-unavailable"),
        ),
    }

    match env::current_exe().ok().and_then(|path| {
        path.parent()
            .map(|directory| directory.join("hoplite-server"))
    }) {
        Some(path) if path.is_file() && executable_file(&path) => {
            match Command::new(&path).arg("version").output() {
                Ok(output)
                    if output.status.success()
                        && output_has_line(
                            &output,
                            &format!("Hoplite server {}", env!("CARGO_PKG_VERSION")),
                        )
                        && output_has_line_prefix(
                            &output,
                            &format!("Nginx {} (", crate::NGINX_VERSION),
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

    match crate::system_ca_bundle() {
        Ok(path) => checks.push(
            Check::pass(
                "trusted-ca-bundle",
                "a trusted CA bundle is available for secure static upstreams",
            )
            .path(Some(&path), show_paths),
        ),
        Err(_) => checks.push(
            Check::warn(
                "trusted-ca-bundle",
                "no trusted CA bundle is available for secure static upstreams",
            )
            .detail("hoplite/doctor-ca-bundle-unavailable"),
        ),
    }
}

fn output_has_line(output: &Output, expected: &str) -> bool {
    stream_has_line(&output.stdout, expected.as_bytes())
        || stream_has_line(&output.stderr, expected.as_bytes())
}

fn output_has_line_prefix(output: &Output, prefix: &str) -> bool {
    stream_has_line_prefix(&output.stdout, prefix.as_bytes())
        || stream_has_line_prefix(&output.stderr, prefix.as_bytes())
}

fn stream_has_line(bytes: &[u8], expected: &[u8]) -> bool {
    bytes
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .any(|line| line == expected)
}

fn stream_has_line_prefix(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .any(|line| line.starts_with(prefix))
}

#[cfg(unix)]
fn executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn executable_file(path: &Path) -> bool {
    path.is_file()
}

fn collect_project_checks(
    target: &Path,
    show_paths: bool,
    checks: &mut Vec<Check>,
) -> Option<Project> {
    let project = match project::discover(target) {
        Ok(project) => project,
        Err(error) => {
            checks.push(
                Check::fail("project", "a valid project.edn could not be discovered")
                    .detail(classify_project_error(&error))
                    .path(Some(target), show_paths),
            );
            return None;
        }
    };
    checks.push(
        Check::pass(
            "project",
            format!("project {} {} is readable", project.id, project.version),
        )
        .path(Some(&project.manifest_path), show_paths),
    );

    match project.resolve_profile(None) {
        Ok(Some(profile)) if profile.language == "hoplite" && profile.main.contains('/') => {
            checks.push(Check::pass(
                "project-profile",
                format!(
                    "default profile {} selects qualified Hoplite main {}",
                    profile.name, profile.main
                ),
            ));
        }
        Ok(Some(profile)) if profile.language != "hoplite" => checks.push(
            Check::fail(
                "project-profile",
                format!(
                    "default profile {} uses language {} instead of hoplite",
                    profile.name, profile.language
                ),
            )
            .detail("hoplite/doctor-profile-language-invalid"),
        ),
        Ok(Some(profile)) => checks.push(
            Check::fail(
                "project-profile",
                format!(
                    "default profile {} has an unqualified main value",
                    profile.name
                ),
            )
            .detail("hoplite/doctor-profile-main-invalid"),
        ),
        Ok(None) => checks.push(
            Check::fail("project-profile", "the project has no runnable profile")
                .detail("hoplite/doctor-profile-missing"),
        ),
        Err(_) => checks.push(
            Check::fail("project-profile", "the default project profile is invalid")
                .detail("hoplite/doctor-profile-invalid"),
        ),
    }

    match crate::source_files(&project) {
        Ok(files) if files.is_empty() => checks.push(
            Check::fail(
                "application-source",
                "the project contains no HAL source files",
            )
            .detail("hoplite/doctor-source-empty"),
        ),
        Ok(files) => checks.push(Check::pass(
            "application-source",
            format!(
                "{} HAL source file{} are discoverable",
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            ),
        )),
        Err(_) => checks.push(
            Check::fail("application-source", "HAL source paths could not be read")
                .detail("hoplite/doctor-source-unreadable"),
        ),
    }

    if project
        .capabilities
        .iter()
        .any(|value| value == "host/nginx")
    {
        checks.push(Check::pass(
            "project-capability",
            "the project declares the host/nginx capability",
        ));
    } else {
        checks.push(
            Check::warn(
                "project-capability",
                "the project does not declare the host/nginx capability",
            )
            .detail("hoplite/doctor-nginx-capability-missing"),
        );
    }

    match crate::platform::load(&project, None) {
        Ok(platform) => checks.push(Check::pass(
            "platform-contract",
            format!(
                "{} platform module activation{} validated",
                platform.modules.len(),
                if platform.modules.len() == 1 { "" } else { "s" }
            ),
        )),
        Err(_) => checks.push(
            Check::fail(
                "platform-contract",
                "the selected platform and package lock are invalid",
            )
            .detail("hoplite/doctor-platform-invalid"),
        ),
    }

    Some(project)
}

fn classify_project_error(error: &str) -> &'static str {
    if error.contains("no project.edn") {
        "hoplite/doctor-project-not-found"
    } else if error.contains("cannot read") {
        "hoplite/doctor-project-unreadable"
    } else {
        "hoplite/doctor-project-invalid"
    }
}

fn collect_generated_application_checks(
    project: &Project,
    show_paths: bool,
    checks: &mut Vec<Check>,
) {
    let output = project.root.join(".hoplite");
    let bundle = output.join("app.hbx");
    let manifest = output.join("apps.hta");
    match (bundle.is_file(), manifest.is_file()) {
        (false, false) => checks.push(
            Check::warn(
                "generated-application",
                "the project has not been built into source-free serving artifacts",
            )
            .detail("hoplite/doctor-application-not-built")
            .path(Some(&output), show_paths),
        ),
        (true, true) => match crate::diagnostics::health(&project.root) {
            Ok(health) if health.source_inputs == 0 => checks.push(
                Check::pass(
                    "generated-application",
                    format!(
                        "validated {} application{}, {} route{}, and source-free output",
                        health.applications,
                        if health.applications == 1 { "" } else { "s" },
                        health.routes,
                        if health.routes == 1 { "" } else { "s" }
                    ),
                )
                .path(Some(&output), show_paths),
            ),
            Ok(health) => checks.push(
                Check::warn(
                    "generated-application",
                    format!(
                        "generated output contains {} application source input{} and is not production source-free",
                        health.source_inputs,
                        if health.source_inputs == 1 { "" } else { "s" }
                    ),
                )
                .detail("hoplite/doctor-application-not-source-free")
                .path(Some(&output), show_paths),
            ),
            Err(_) => checks.push(
                Check::fail(
                    "generated-application",
                    "the generated HAB0 application or exact manifest is invalid",
                )
                .detail("hoplite/doctor-built-application-invalid")
                .path(Some(&output), show_paths),
            ),
        },
        _ => checks.push(
            Check::fail(
                "generated-application",
                "generated output contains only one of app.hbx and apps.hta",
            )
            .detail("hoplite/doctor-application-incomplete")
            .path(Some(&output), show_paths),
        ),
    }
}

fn collect_deep_preflight(project: &Project, checks: &mut Vec<Check>) {
    match crate::check(&project.root, &crate::BuildSettings::default()) {
        Ok(_) => checks.push(Check::pass(
            "deep-preflight",
            "source compilation, app evaluation, HAB0 construction, and platform preflight passed",
        )),
        Err(_) => checks.push(
            Check::fail(
                "deep-preflight",
                "the full source compilation and application preflight failed",
            )
            .detail("hoplite/doctor-deep-preflight-failed"),
        ),
    }
}

fn check_json(check: &Check) -> JsonValue {
    json!({
        "id": check.id,
        "status": check.status.as_str(),
        "summary": check.summary,
        "detail": check.detail,
        "path": check.path,
    })
}

fn render_json(report: &Report) -> String {
    let document = json!({
        "format": FORMAT,
        "healthy": report.healthy(),
        "complete": report.complete(),
        "deep": report.deep,
        "strict": report.strict,
        "system": {
            "operating_system": report.operating_system,
            "architecture": report.architecture,
        },
        "versions": {
            "hoplite": env!("CARGO_PKG_VERSION"),
            "nginx": crate::NGINX_VERSION,
            "nginx_distribution": report.nginx_distribution,
        },
        "summary": {
            "passed": report.checks.iter().filter(|check| check.status == CheckStatus::Pass).count(),
            "warnings": report.warnings(),
            "failures": report.failures(),
        },
        "checks": report.checks.iter().map(check_json).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&document).expect("doctor JSON serialization") + "\n"
}

fn render_human(report: &Report) -> String {
    let mut output = String::new();
    writeln!(&mut output, "Hoplite doctor").unwrap();
    writeln!(&mut output, "report format: {FORMAT}").unwrap();
    writeln!(
        &mut output,
        "system: {}/{}; Hoplite {}; Nginx {} ({})",
        report.operating_system,
        report.architecture,
        env!("CARGO_PKG_VERSION"),
        crate::NGINX_VERSION,
        report.nginx_distribution
    )
    .unwrap();
    writeln!(
        &mut output,
        "mode: {}{}",
        if report.deep { "deep" } else { "read-only" },
        if report.strict { ", strict" } else { "" }
    )
    .unwrap();
    writeln!(&mut output).unwrap();
    for check in &report.checks {
        writeln!(
            &mut output,
            "[{}] {}: {}",
            check.status.as_str(),
            check.id,
            check.summary
        )
        .unwrap();
        if let Some(detail) = &check.detail {
            writeln!(&mut output, "    class: {detail}").unwrap();
        }
        if let Some(path) = &check.path {
            writeln!(&mut output, "    path: {path}").unwrap();
        }
    }
    writeln!(&mut output).unwrap();
    writeln!(
        &mut output,
        "result: {} ({} warning{}, {} failure{})",
        if report.healthy() {
            "healthy"
        } else {
            "unhealthy"
        },
        report.warnings(),
        if report.warnings() == 1 { "" } else { "s" },
        report.failures(),
        if report.failures() == 1 { "" } else { "s" }
    )
    .unwrap();
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn fixture_root() -> PathBuf {
        env::temp_dir().join(format!(
            "hoplite-doctor-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn write_project(root: &Path) {
        fs::create_dir_all(root).unwrap();
        fs::write(
            root.join("project.edn"),
            r#"{:hara/type :project
 :hara/version "1.0.0"
 :project/id doctor/example
 :project/version "0.1.0"
 :project/source-paths ["."]
 :project/test-paths []
 :project/extension-paths []
 :project/capabilities #{:host/nginx}
 :project/main doctor.app
 :project/default-profile :server
 :project/profiles
 {:server {:profile/language :hoplite
           :profile/main doctor.app/app
           :profile/options {:port 8080}}}}
"#,
        )
        .unwrap();
        fs::write(
            root.join("app.hal"),
            "(ns doctor.app (:require [hoplite.core :as h]))\n(defn hello [_] {:status 200 :body \"ok\"})\n(def app (h/app {:name \"doctor\" :resources [[\"/\" {:get {:handler #'hello}}]]}))\n",
        )
        .unwrap();
    }

    #[test]
    fn version_probes_require_exact_reported_identities() {
        assert!(stream_has_line(
            b"nginx version: nginx/1.30.4\n",
            b"nginx version: nginx/1.30.4"
        ));
        assert!(!stream_has_line(
            b"nginx version: nginx/1.30.40\n",
            b"nginx version: nginx/1.30.4"
        ));
        assert!(!stream_has_line(
            b"nginx version: nginx/1.28.0\n",
            b"nginx version: nginx/1.30.4"
        ));
        assert!(stream_has_line(
            b"Hoplite server 0.2.0\nNginx 1.30.4 (embedded)\n",
            b"Hoplite server 0.2.0"
        ));
        assert!(stream_has_line_prefix(
            b"Hoplite server 0.2.0\nNginx 1.30.4 (embedded)\n",
            b"Nginx 1.30.4 ("
        ));
    }

    #[test]
    fn project_checks_are_static_and_path_redacted() {
        let root = fixture_root();
        write_project(&root);
        let mut checks = Vec::new();

        let project = collect_project_checks(&root, false, &mut checks).unwrap();
        assert_eq!(project.id, "doctor/example");
        assert!(checks.iter().all(|check| check.status != CheckStatus::Fail));
        assert!(checks.iter().all(|check| check.path.is_none()));
        let report = Report {
            deep: false,
            strict: false,
            operating_system: "test-os",
            architecture: "test-arch",
            nginx_distribution: "test",
            checks,
        };
        let json = render_json(&report);
        assert!(!json.contains(root.to_string_lossy().as_ref()));
        assert!(json.contains("\"healthy\": true"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_path_authority_is_local_to_the_requested_report() {
        let root = fixture_root();
        write_project(&root);
        let mut checks = Vec::new();

        collect_project_checks(&root, true, &mut checks).unwrap();
        assert!(checks
            .iter()
            .filter_map(|check| check.path.as_ref())
            .any(|path| path.contains(root.to_string_lossy().as_ref())));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_project_errors_use_stable_redacted_classes() {
        let root = fixture_root();
        fs::create_dir_all(&root).unwrap();
        let mut checks = Vec::new();

        assert!(collect_project_checks(&root, false, &mut checks).is_none());
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Fail);
        assert_eq!(
            checks[0].detail.as_deref(),
            Some("hoplite/doctor-project-not-found")
        );
        assert!(!render_human(&Report {
            deep: false,
            strict: false,
            operating_system: "test-os",
            architecture: "test-arch",
            nginx_distribution: "test",
            checks,
        })
        .contains(root.to_string_lossy().as_ref()));

        fs::remove_dir_all(root).unwrap();
    }
}
