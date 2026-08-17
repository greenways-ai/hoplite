#!/usr/bin/env python3
from pathlib import Path

root = Path(__file__).resolve().parents[2]
path = root / "core/src/console/protocol.rs"
text = path.read_text()

old = """        Value::Set(values) => Some(values.iter().cloned().collect()),
        _ => None,
"""
new = """        Value::Set(values) => Some(values.iter().cloned().collect()),
        Value::OrderedSet(values) => Some(values.iter().cloned().collect()),
        Value::SortedSet(values) => Some(values.iter().cloned().collect()),
        _ => None,
"""
if text.count(old) != 1:
    raise SystemExit("expected one value_sequence set arm")
text = text.replace(old, new, 1)

marker = """    #[test]
    fn defaults_match_the_public_console_contract() {"""
test = """    #[test]
    fn hal_persistent_sets_are_valid_descriptor_and_grant_collections() {
        let mut runtime = hara_wasm::Runtime::new();
        let descriptors = runtime
            .eval_native_value(
                "[{:command \\\"status\\\" \\
                   :effect :read \\
                   :input {:type :map :required #{} :optional #{}}}]",
            )
            .unwrap();
        let commands = CommandSet::parse(descriptors).unwrap();
        let grant = runtime
            .eval_native_value(
                "{:protocol \\\"hoplite.console-grant/0-alpha\\\" \\
                  :console \\\"console.test\\\" \\
                  :commands #{\\\"status\\\"} \\
                  :write false}",
            )
            .unwrap();
        let grant = ConsoleGrant::parse(&grant).unwrap();
        commands.validate_grant(&grant).unwrap();
        assert!(commands
            .validate_call(&grant, "status", &map_value(vec![]), 1024)
            .is_ok());
    }

"""
if text.count(marker) != 1:
    raise SystemExit("expected one protocol test insertion point")
path.write_text(text.replace(marker, test + marker, 1))
