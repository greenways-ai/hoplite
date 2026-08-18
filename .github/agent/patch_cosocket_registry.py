#!/usr/bin/env python3
from pathlib import Path

path = Path("core/src/app.rs")
text = path.read_text()


def replace_once(old: str, new: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"expected one app.rs anchor, found {count}:\n{old}")
    text = text.replace(old, new)


replace_once(
    '''pub const HOST_SOURCE: &str = include_str!("../lib/src/hoplite/host.hal");
pub const INTERNAL_SOURCE: &str = include_str!("../lib/src/hoplite/internal.hal");
pub const RAW_SOURCE: &str = include_str!("../lib/src/hoplite/raw.hal");
pub const RTC_SOURCE: &str = include_str!("../lib/src/hoplite/rtc.hal");
pub const RESPONSE_SOURCE: &str = include_str!("../lib/src/hoplite/response_source.hal");
''',
    '''pub const HOST_SOURCE: &str = include_str!("../lib/src/hoplite/host.hal");
pub const INTERNAL_SOURCE: &str = include_str!("../lib/src/hoplite/internal.hal");
pub const NCHAN_SOURCE: &str = include_str!("../lib/src/hoplite/nchan.hal");
pub const RAW_SOURCE: &str = include_str!("../lib/src/hoplite/raw.hal");
pub const RTC_SOURCE: &str = include_str!("../lib/src/hoplite/rtc.hal");
pub const RESPONSE_SOURCE: &str = include_str!("../lib/src/hoplite/response_source.hal");
pub const SOCKET_SOURCE: &str = include_str!("../lib/src/hoplite/socket.hal");

const APPLICATION_RESOURCES: &[(&str, &str)] = &[
    ("hoplite.core", CORE_SOURCE),
    ("hoplite.host", HOST_SOURCE),
    ("hoplite.internal", INTERNAL_SOURCE),
    ("hoplite.nchan", NCHAN_SOURCE),
    ("hoplite.raw", RAW_SOURCE),
    ("hoplite.rtc", RTC_SOURCE),
    ("hoplite.socket", SOCKET_SOURCE),
];
const CONTRACT_RESOURCES: &[(&str, &str)] =
    &[("hoplite.response-source", RESPONSE_SOURCE)];
''',
)

replace_once(
    '''#[cfg(test)]
const RESPONSE_SOURCE_TEST_SOURCE: &str =
    include_str!("../lib/test/hoplite/response_source_test.hal");
''',
    '''#[cfg(test)]
const RESPONSE_SOURCE_TEST_SOURCE: &str =
    include_str!("../lib/test/hoplite/response_source_test.hal");
#[cfg(test)]
const SOCKET_TEST_SOURCE: &str = include_str!("../lib/test/hoplite/socket_test.hal");
''',
)

replace_once(
    '''pub fn register_contract_resources(runtime: &mut Runtime) {
    runtime.register_resource("hoplite.response-source", RESPONSE_SOURCE);
}

pub fn register_resources(runtime: &mut Runtime) {
    runtime.register_resource("hoplite.core", CORE_SOURCE);
    runtime.register_resource("hoplite.host", HOST_SOURCE);
    runtime.register_resource("hoplite.internal", INTERNAL_SOURCE);
    runtime.register_resource("hoplite.raw", RAW_SOURCE);
    runtime.register_resource("hoplite.rtc", RTC_SOURCE);
    register_contract_resources(runtime);
}
''',
    '''pub fn register_contract_resources(runtime: &mut Runtime) {
    for &(namespace, source) in CONTRACT_RESOURCES {
        runtime.register_resource(namespace, source);
    }
}

pub fn register_resources(runtime: &mut Runtime) {
    for &(namespace, source) in APPLICATION_RESOURCES {
        runtime.register_resource(namespace, source);
    }
    register_contract_resources(runtime);
}
''',
)

replace_once(
    '''    #[test]
    fn resources_exclude_the_retired_value_contract() {
''',
    '''    #[test]
    fn production_resources_cover_the_non_development_hal_library() {
        let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib/src/hoplite");
        let expected = fs::read_dir(&library)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", library.display()))
            .map(|entry| entry.expect("HAL library entry must be readable").path())
            .filter(|path| path.extension().and_then(OsStr::to_str) == Some("hal"))
            .filter_map(|path| {
                let stem = path.file_stem()?.to_str()?;
                (stem != "dev").then(|| format!("hoplite.{}", stem.replace('_', "-")))
            })
            .collect::<BTreeSet<_>>();
        let registered = APPLICATION_RESOURCES
            .iter()
            .chain(CONTRACT_RESOURCES.iter())
            .map(|(namespace, _)| (*namespace).to_owned())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            registered, expected,
            "every non-development Hoplite HAL namespace must be available to application builds"
        );

        let mut runtime = Runtime::new();
        register_resources(&mut runtime);
        for (index, namespace) in registered.iter().enumerate() {
            let probe = format!(
                "(ns hoplite.resource-probe.n{index} (:require [{namespace} :as subject])) true"
            );
            assert_eq!(
                runtime.eval_native_value(&probe).unwrap_or_else(|error| {
                    panic!("registered resource {namespace} did not load: {error}")
                }),
                Value::Bool(true),
                "registered resource {namespace} did not evaluate"
            );
        }
    }

    #[test]
    fn socket_hal_contract_evaluates_from_the_production_registry() {
        let mut runtime = Runtime::new();
        register_resources(&mut runtime);
        assert_eq!(
            runtime.eval_native_value(SOCKET_TEST_SOURCE).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn resources_exclude_the_retired_value_contract() {
''',
)

path.write_text(text)
