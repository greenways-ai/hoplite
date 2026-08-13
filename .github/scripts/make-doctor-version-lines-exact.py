from pathlib import Path

root = Path(__file__).resolve().parents[2]
path = root / "core/src/doctor.rs"
source = path.read_text()

source = source.replace(
    '''                        && output_contains(
                            &output,
                            &format!("nginx/{}", crate::NGINX_VERSION),
                        ) =>''',
    '''                        && output_has_line(
                            &output,
                            &format!(
                                "nginx version: nginx/{}",
                                crate::NGINX_VERSION
                            ),
                        ) =>''',
    1,
)
source = source.replace(
    '''                        && output_contains(
                            &output,
                            &format!("Hoplite server {}", env!("CARGO_PKG_VERSION")),
                        )
                        && output_contains(
                            &output,
                            &format!("Nginx {}", crate::NGINX_VERSION),
                        ) =>''',
    '''                        && output_has_line(
                            &output,
                            &format!("Hoplite server {}", env!("CARGO_PKG_VERSION")),
                        )
                        && output_has_line_prefix(
                            &output,
                            &format!("Nginx {} (", crate::NGINX_VERSION),
                        ) =>''',
    1,
)

old_helpers = '''fn output_contains(output: &Output, expected: &str) -> bool {
    bytes_contain(&output.stdout, expected.as_bytes())
        || bytes_contain(&output.stderr, expected.as_bytes())
}

fn bytes_contain(bytes: &[u8], expected: &[u8]) -> bool {
    !expected.is_empty() && bytes.windows(expected.len()).any(|window| window == expected)
}
'''
new_helpers = '''fn output_has_line(output: &Output, expected: &str) -> bool {
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
'''
if source.count(old_helpers) != 1:
    raise SystemExit("unexpected version-output helpers")
source = source.replace(old_helpers, new_helpers, 1)

old_test = '''    fn version_probes_require_exact_reported_identities() {
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
'''
new_test = '''    fn version_probes_require_exact_reported_identities() {
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
'''
if source.count(old_test) != 1:
    raise SystemExit("unexpected exact-version test")
source = source.replace(old_test, new_test, 1)

path.write_text(source)
(root / ".github/workflows/make-doctor-version-lines-exact.yml").unlink()
Path(__file__).unlink()
