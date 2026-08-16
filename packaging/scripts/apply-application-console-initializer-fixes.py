#!/usr/bin/env python3
from pathlib import Path

root = Path(__file__).resolve().parents[2]


def replace_once(path: Path, old: str, new: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(
            f"{path}: expected one replacement, found {count}: {old[:80]!r}"
        )
    path.write_text(text.replace(old, new, 1))


replace_once(
    root / "core/src/diagnostics.rs",
    '''                hostnames: Vec::new(),
                routes: vec![''',
    '''                hostnames: Vec::new(),
                console: None,
                routes: vec![''',
)

replace_once(
    root / "core/src/main.rs",
    '''            hostnames: Vec::new(),
            routes: Vec::new(),
            request_body: None,''',
    '''            hostnames: Vec::new(),
            routes: Vec::new(),
            console: None,
            request_body: None,''',
)
