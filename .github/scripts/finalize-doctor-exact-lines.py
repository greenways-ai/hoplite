from pathlib import Path

root = Path(__file__).resolve().parents[2]
path = root / "core/src/doctor.rs"
source = path.read_text()

replacements = [
    (
        '&& output_contains(&output, &format!("nginx/{}", crate::NGINX_VERSION)) =>',
        '''&& output_has_line(
                            &output,
                            &format!(
                                "nginx version: nginx/{}",
                                crate::NGINX_VERSION
                            ),
                        ) =>''',
    ),
    (
        '''&& output_contains(
                            &output,
                            &format!("Hoplite server {}", env!("CARGO_PKG_VERSION")),
                        )
                        && output_contains(&output, &format!("Nginx {}", crate::NGINX_VERSION)) =>''',
        '''&& output_has_line(
                            &output,
                            &format!("Hoplite server {}", env!("CARGO_PKG_VERSION")),
                        )
                        && output_has_line_prefix(
                            &output,
                            &format!("Nginx {} (", crate::NGINX_VERSION),
                        ) =>''',
    ),
    (
        '''fn output_contains(output: &Output, expected: &str) -> bool {
    bytes_contain(&output.stdout, expected.as_bytes())
        || bytes_contain(&output.stderr, expected.as_bytes())
}

fn bytes_contain(bytes: &[u8], expected: &[u8]) -> bool {
    !expected.is_empty()
        && bytes
            .windows(expected.len())
            .any(|window| window == expected)
}
''',
        '''fn output_has_line(output: &Output, expected: &str) -> bool {
    stream_has_line(&output.stdout, expected.as_bytes())
        || stream_has_line(&output.stderr, expected.as_bytes())
}

fn output_has_line_prefix(output: &Output, prefix: &str) -> bool {
    stream_has_line_prefix(&output.stdout, prefix.as_bytes())
        || stream_has_line_prefix(&output.stderr, prefix.as_bytes())
}

fn stream_has_line(bytes: &[u8], expected: &[u8]) -> bool {
    bytes
        .split(|byte| *byte == b'\\n')
        .map(|line| line.strip_suffix(b"\\r").unwrap_or(line))
        .any(|line| line == expected)
}

fn stream_has_line_prefix(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes
        .split(|byte| *byte == b'\\n')
        .map(|line| line.strip_suffix(b"\\r").unwrap_or(line))
        .any(|line| line.starts_with(prefix))
}
''',
    ),
    (
        '''    fn version_probes_require_exact_reported_identities() {
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
''',
        '''    fn version_probes_require_exact_reported_identities() {
        assert!(stream_has_line(
            b"nginx version: nginx/1.30.4\\n",
            b"nginx version: nginx/1.30.4"
        ));
        assert!(!stream_has_line(
            b"nginx version: nginx/1.30.40\\n",
            b"nginx version: nginx/1.30.4"
        ));
        assert!(!stream_has_line(
            b"nginx version: nginx/1.28.0\\n",
            b"nginx version: nginx/1.30.4"
        ));
        assert!(stream_has_line(
            b"Hoplite server 0.1.0\\nNginx 1.30.4 (embedded)\\n",
            b"Hoplite server 0.1.0"
        ));
        assert!(stream_has_line_prefix(
            b"Hoplite server 0.1.0\\nNginx 1.30.4 (embedded)\\n",
            b"Nginx 1.30.4 ("
        ));
    }
''',
    ),
]

for old, new in replacements:
    if source.count(old) != 1:
        raise SystemExit(f"expected one replacement, found {source.count(old)} for {old[:80]!r}")
    source = source.replace(old, new, 1)

path.write_text(source)

for relative in [
    ".github/workflows/make-doctor-version-lines-exact.yml",
    ".github/scripts/make-doctor-version-lines-exact.py",
    ".github/workflows/finalize-doctor-exact-lines.yml",
    ".github/scripts/finalize-doctor-exact-lines.py",
]:
    candidate = root / relative
    if candidate.exists():
        candidate.unlink()
