use hara_wasm::core::{self, Value};
use hara_wasm::hta;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};

pub const CLIENT_BUNDLE_MAGIC: &[u8; 4] = b"HCB0";
pub const CLIENT_BUNDLE_FORMAT: &str = "hoplite.console-client/0-alpha";
pub const GRANT_PROTOCOL: &str = "hoplite.console-grant/0-alpha";
pub const REQUEST_PROTOCOL: &str = "tahto.console-request/0-alpha";
pub const BROKER_SERVICE: &str = "hoplite.console";
pub const MAX_CLIENT_BUNDLE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsoleLimits {
    pub source_bytes: usize,
    pub result_bytes: usize,
    pub evaluation_millis: u64,
    pub memory_bytes: u64,
}

impl Default for ConsoleLimits {
    fn default() -> Self {
        Self {
            source_bytes: 64 * 1024,
            result_bytes: 1024 * 1024,
            evaluation_millis: 5_000,
            memory_bytes: 128 * 1024 * 1024,
        }
    }
}

impl ConsoleLimits {
    pub fn validate(self) -> Result<Self, String> {
        if self.source_bytes == 0 || self.source_bytes > 16 * 1024 * 1024 {
            return Err("console source limit must be between 1 byte and 16 MiB".into());
        }
        if self.result_bytes == 0 || self.result_bytes > 64 * 1024 * 1024 {
            return Err("console result limit must be between 1 byte and 64 MiB".into());
        }
        if self.evaluation_millis == 0 || self.evaluation_millis > 300_000 {
            return Err("console evaluation limit must be between 1 ms and 300 s".into());
        }
        if self.memory_bytes < 16 * 1024 * 1024 || self.memory_bytes > 16 * 1024 * 1024 * 1024 {
            return Err("console memory limit must be between 16 MiB and 16 GiB".into());
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandEffect {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldType {
    Hta,
    String,
    Timestamp,
    Integer,
    Boolean,
    Map,
    Vector,
    Set,
}

impl FieldType {
    fn parse(value: &Value) -> Result<Self, String> {
        match keyword_text(value).as_deref() {
            Some("hta") => Ok(Self::Hta),
            Some("string") => Ok(Self::String),
            Some("timestamp") => Ok(Self::Timestamp),
            Some("integer") => Ok(Self::Integer),
            Some("boolean") => Ok(Self::Boolean),
            Some("map") => Ok(Self::Map),
            Some("vector") => Ok(Self::Vector),
            Some("set") => Ok(Self::Set),
            _ => Err("console schema contains an unsupported field type".into()),
        }
    }

    fn accepts(self, value: &Value) -> bool {
        match self {
            Self::Hta => hta::encode(value).is_ok(),
            Self::String => matches!(value, Value::String(_)),
            Self::Timestamp => match value {
                Value::String(value) => timestamp(value),
                _ => false,
            },
            Self::Integer => matches!(value, Value::Number(_)),
            Self::Boolean => matches!(value, Value::Bool(_)),
            Self::Map => core::map_entries(value).is_some(),
            Self::Vector => matches!(value, Value::Vector(_)),
            Self::Set => matches!(value, Value::Set(_)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputSchema {
    required: BTreeSet<String>,
    optional: BTreeSet<String>,
    fields: BTreeMap<String, FieldType>,
}

impl InputSchema {
    fn parse(value: &Value) -> Result<Self, String> {
        if keyword_text(
            &map_get(value, "type").ok_or("console command input schema requires :type")?,
        )
        .as_deref()
            != Some("map")
        {
            return Err("console command input schema :type must be :map".into());
        }
        let required = text_set(
            &map_get(value, "required")
                .ok_or("console command input schema requires :required")?,
        )?;
        let optional = text_set(
            &map_get(value, "optional")
                .ok_or("console command input schema requires :optional")?,
        )?;
        if required.iter().any(|field| optional.contains(field)) {
            return Err("console command input schema fields cannot be both required and optional"
                .into());
        }
        let allowed = required
            .iter()
            .chain(optional.iter())
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut fields = BTreeMap::new();
        if let Some(field_types) = map_get(value, "fields") {
            for (key, field_type) in core::map_entries(&field_types)
                .ok_or("console command input :fields must be a map")?
            {
                let name = field_name(&key)
                    .ok_or("console command input :fields keys must be keywords or strings")?;
                if !allowed.contains(&name) {
                    return Err(format!(
                        "console command input :fields contains undeclared field {name:?}"
                    ));
                }
                if fields.insert(name, FieldType::parse(&field_type)?).is_some() {
                    return Err("console command input :fields contains a duplicate".into());
                }
            }
        }
        Ok(Self {
            required,
            optional,
            fields,
        })
    }

    pub fn validate(&self, input: &Value) -> Result<(), String> {
        let entries = core::map_entries(input)
            .ok_or_else(|| "hoplite.console/input-invalid: input must be a map".to_string())?;
        let mut present = BTreeSet::new();
        for (key, value) in entries {
            let name = field_name(&key).ok_or_else(|| {
                "hoplite.console/input-invalid: input keys must be keywords or strings".to_string()
            })?;
            if !self.required.contains(&name) && !self.optional.contains(&name) {
                return Err(format!(
                    "hoplite.console/input-invalid: unsupported input field {name:?}"
                ));
            }
            if !present.insert(name.clone()) {
                return Err(format!(
                    "hoplite.console/input-invalid: duplicate input field {name:?}"
                ));
            }
            if self
                .fields
                .get(&name)
                .is_some_and(|field_type| !field_type.accepts(&value))
            {
                return Err(format!(
                    "hoplite.console/input-invalid: invalid value for field {name:?}"
                ));
            }
        }
        if let Some(missing) = self.required.iter().find(|field| !present.contains(*field)) {
            return Err(format!(
                "hoplite.console/input-invalid: missing input field {missing:?}"
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct CommandDescriptor {
    pub command: String,
    pub effect: CommandEffect,
    pub input: InputSchema,
    raw: Value,
}

impl CommandDescriptor {
    fn parse(value: Value) -> Result<Self, String> {
        let command = string_value(
            &map_get(&value, "command").ok_or("console command descriptor requires :command")?,
        )?;
        if !command_name(&command) {
            return Err("console command name is invalid".into());
        }
        let effect = match keyword_text(
            &map_get(&value, "effect").ok_or("console command descriptor requires :effect")?,
        )
        .as_deref()
        {
            Some("read") => CommandEffect::Read,
            Some("write") => CommandEffect::Write,
            _ => return Err("console command descriptor :effect must be :read or :write".into()),
        };
        let input = InputSchema::parse(
            &map_get(&value, "input").ok_or("console command descriptor requires :input")?,
        )?;
        hta::encode(&value)
            .map_err(|error| format!("console command descriptor is not immutable HTA: {error}"))?;
        Ok(Self {
            command,
            effect,
            input,
            raw: value,
        })
    }
}

#[derive(Clone, Debug)]
pub struct CommandSet {
    commands: BTreeMap<String, CommandDescriptor>,
}

impl CommandSet {
    pub fn parse(value: Value) -> Result<Self, String> {
        let values = value_sequence(&value)
            .ok_or("console descriptor bundle must be a vector or sequence")?;
        if values.is_empty() || values.len() > 256 {
            return Err("console descriptor bundle must contain between 1 and 256 commands".into());
        }
        let mut commands = BTreeMap::new();
        for value in values {
            let descriptor = CommandDescriptor::parse(value)?;
            if commands
                .insert(descriptor.command.clone(), descriptor)
                .is_some()
            {
                return Err("console descriptor bundle contains a duplicate command".into());
            }
        }
        Ok(Self { commands })
    }

    pub fn descriptor(&self, command: &str) -> Option<&CommandDescriptor> {
        self.commands.get(command)
    }

    pub fn granted_value(&self, grant: &ConsoleGrant) -> Value {
        Value::Vector(
            self.commands
                .values()
                .filter(|descriptor| grant.allows_descriptor(descriptor))
                .map(|descriptor| descriptor.raw.clone())
                .collect::<Vec<_>>()
                .into(),
        )
    }

    pub fn validate_grant(&self, grant: &ConsoleGrant) -> Result<(), String> {
        if let Some(command) = grant
            .commands
            .iter()
            .find(|command| !self.commands.contains_key(*command))
        {
            return Err(format!(
                "hoplite.console/grant-invalid: unknown command {command:?}"
            ));
        }
        Ok(())
    }

    pub fn validate_call(
        &self,
        grant: &ConsoleGrant,
        command: &str,
        input: &Value,
        maximum_input_bytes: usize,
    ) -> Result<(), String> {
        self.validate_grant(grant)?;
        let descriptor = self
            .commands
            .get(command)
            .ok_or_else(|| "hoplite.console/command-unlisted".to_string())?;
        if !grant.commands.contains(command) {
            return Err("hoplite.console/command-not-granted".into());
        }
        if descriptor.effect == CommandEffect::Write && !grant.write {
            return Err("hoplite.console/write-not-granted".into());
        }
        descriptor.input.validate(input)?;
        let encoded = hta::encode(input)
            .map_err(|error| format!("hoplite.console/input-not-immutable: {error}"))?;
        if encoded.len() > maximum_input_bytes {
            return Err("hoplite.console/input-too-large".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleGrant {
    pub console: String,
    pub commands: BTreeSet<String>,
    pub write: bool,
}

impl ConsoleGrant {
    pub fn parse(value: &Value) -> Result<Self, String> {
        if string_value(
            &map_get(value, "protocol").ok_or("console grant requires :protocol")?,
        )?
            != GRANT_PROTOCOL
        {
            return Err("console grant protocol is unsupported".into());
        }
        let console = string_value(
            &map_get(value, "console").ok_or("console grant requires :console")?,
        )?;
        if !identifier(&console) {
            return Err("console grant :console is invalid".into());
        }
        let commands = text_set(
            &map_get(value, "commands").ok_or("console grant requires :commands")?,
        )?;
        if commands.iter().any(|command| !command_name(command)) {
            return Err("console grant contains an invalid command name".into());
        }
        let write = match map_get(value, "write") {
            Some(Value::Bool(value)) => value,
            _ => return Err("console grant :write must be boolean".into()),
        };
        let allowed = ["protocol", "console", "commands", "write"]
            .into_iter()
            .collect::<BTreeSet<_>>();
        for (key, _) in core::map_entries(value).ok_or("console grant must be a map")? {
            let key = field_name(&key).ok_or("console grant keys must be text")?;
            if !allowed.contains(key.as_str()) {
                return Err(format!("console grant contains unsupported field {key:?}"));
            }
        }
        Ok(Self {
            console,
            commands,
            write,
        })
    }

    pub fn with_console(&self, console: String) -> Result<Self, String> {
        if !identifier(&console) {
            return Err("generated console identifier is invalid".into());
        }
        Ok(Self {
            console,
            commands: self.commands.clone(),
            write: self.write,
        })
    }

    pub fn to_value(&self) -> Value {
        map_value(vec![
            ("protocol", Value::String(GRANT_PROTOCOL.into())),
            ("console", Value::String(self.console.clone())),
            (
                "commands",
                Value::Set(
                    self.commands
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect::<Vec<_>>()
                        .into(),
                ),
            ),
            ("write", Value::Bool(self.write)),
        ])
    }

    fn allows_descriptor(&self, descriptor: &CommandDescriptor) -> bool {
        self.commands.contains(&descriptor.command)
            && (descriptor.effect == CommandEffect::Read || self.write)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientBundle {
    pub namespace: String,
    pub source: String,
}

impl ClientBundle {
    pub fn new(namespace: String, source: String) -> Result<Self, String> {
        if !namespace_name(&namespace) {
            return Err("console client namespace is invalid".into());
        }
        if source.is_empty() || source.len() > MAX_CLIENT_BUNDLE_BYTES {
            return Err("console client source must be between 1 byte and 1 MiB".into());
        }
        Ok(Self { namespace, source })
    }

    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let namespace = self.namespace.as_bytes();
        let source = self.source.as_bytes();
        let namespace_len = u16::try_from(namespace.len())
            .map_err(|_| "console client namespace is too long")?;
        let source_len = u32::try_from(source.len())
            .map_err(|_| "console client source is too long")?;
        let digest = bundle_digest(namespace, source);
        let mut output = Vec::with_capacity(42 + namespace.len() + source.len());
        output.extend_from_slice(CLIENT_BUNDLE_MAGIC);
        output.extend_from_slice(&namespace_len.to_be_bytes());
        output.extend_from_slice(&source_len.to_be_bytes());
        output.extend_from_slice(&digest);
        output.extend_from_slice(namespace);
        output.extend_from_slice(source);
        Ok(output)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 42 || &bytes[..4] != CLIENT_BUNDLE_MAGIC {
            return Err("console client bundle has invalid HCB0 magic".into());
        }
        let namespace_len = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        let source_len = u32::from_be_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]) as usize;
        if source_len == 0 || source_len > MAX_CLIENT_BUNDLE_BYTES {
            return Err("console client bundle source length is invalid".into());
        }
        let expected = 42usize
            .checked_add(namespace_len)
            .and_then(|value| value.checked_add(source_len))
            .ok_or("console client bundle length overflow")?;
        if bytes.len() != expected {
            return Err("console client bundle length does not match its header".into());
        }
        let namespace_start = 42;
        let source_start = namespace_start + namespace_len;
        let namespace = std::str::from_utf8(&bytes[namespace_start..source_start])
            .map_err(|_| "console client namespace is not UTF-8")?;
        let source = std::str::from_utf8(&bytes[source_start..])
            .map_err(|_| "console client source is not UTF-8")?;
        let digest = bundle_digest(namespace.as_bytes(), source.as_bytes());
        if bytes[10..42] != digest {
            return Err("console client bundle digest mismatch".into());
        }
        Self::new(namespace.into(), source.into())
    }
}

pub fn read_hta_frame<R: Read>(reader: &mut R, maximum: usize) -> Result<Option<Value>, String> {
    let Some(bytes) = read_frame(reader, maximum)? else {
        return Ok(None);
    };
    hta::decode(&bytes).map(Some).map_err(|error| format!("invalid HTA frame: {error}"))
}

pub fn write_hta_frame<W: Write>(
    writer: &mut W,
    value: &Value,
    maximum: usize,
) -> Result<(), String> {
    let bytes = hta::encode(value).map_err(|error| format!("cannot encode HTA frame: {error}"))?;
    write_frame(writer, &bytes, maximum)
}

pub fn read_frame<R: Read>(reader: &mut R, maximum: usize) -> Result<Option<Vec<u8>>, String> {
    let mut header = [0_u8; 4];
    match reader.read(&mut header[..1]) {
        Ok(0) => return Ok(None),
        Ok(1) => {}
        Ok(_) => unreachable!(),
        Err(error) => return Err(format!("cannot read frame: {error}")),
    }
    reader
        .read_exact(&mut header[1..])
        .map_err(|error| format!("cannot read frame header: {error}"))?;
    let length = u32::from_be_bytes(header) as usize;
    if length > maximum {
        return Err(format!("frame exceeds configured limit of {maximum} bytes"));
    }
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .map_err(|error| format!("cannot read frame payload: {error}"))?;
    Ok(Some(payload))
}

pub fn write_frame<W: Write>(writer: &mut W, bytes: &[u8], maximum: usize) -> Result<(), String> {
    if bytes.len() > maximum {
        return Err(format!("frame exceeds configured limit of {maximum} bytes"));
    }
    let length = u32::try_from(bytes.len()).map_err(|_| "frame is too large")?;
    writer
        .write_all(&length.to_be_bytes())
        .and_then(|_| writer.write_all(bytes))
        .and_then(|_| writer.flush())
        .map_err(|error| format!("cannot write frame: {error}"))
}

pub fn success(value: Value) -> Value {
    map_value(vec![("ok", Value::Bool(true)), ("value", value)])
}

pub fn failure(code: &str, message: impl Into<String>) -> Value {
    map_value(vec![
        ("ok", Value::Bool(false)),
        (
            "error",
            map_value(vec![
                ("code", Value::String(code.into())),
                ("message", Value::String(message.into())),
            ]),
        ),
    ])
}

pub(crate) fn map_value(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (Value::Keyword(key.into()), value))
            .collect(),
    )
}

pub(crate) fn map_get(value: &Value, name: &str) -> Option<Value> {
    core::map_entries(value)?.into_iter().find_map(|(key, value)| {
        matches!(&key, Value::Keyword(keyword) if keyword.as_str() == name)
            .then_some(value)
            .or_else(|| matches!(&key, Value::String(text) if text == name).then_some(value))
    })
}

pub(crate) fn string_value(value: &Value) -> Result<String, String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Keyword(value) => Ok(value.as_str().to_owned()),
        _ => Err("expected text value".into()),
    }
}

pub(crate) fn keyword_text(value: &Value) -> Option<String> {
    match value {
        Value::Keyword(value) => Some(value.as_str().to_owned()),
        Value::String(value) => Some(value.clone()),
        _ => None,
    }
}

pub(crate) fn value_sequence(value: &Value) -> Option<Vec<Value>> {
    match value {
        Value::Vector(values) => Some(values.iter().cloned().collect()),
        Value::List(values) => Some(values.iter().cloned().collect()),
        Value::Tuple(values) => Some(values.iter().cloned().collect()),
        Value::Set(values) => Some(values.iter().cloned().collect()),
        _ => None,
    }
}

fn text_set(value: &Value) -> Result<BTreeSet<String>, String> {
    value_sequence(value)
        .ok_or_else(|| "expected a collection of text values".to_string())?
        .into_iter()
        .map(|value| string_value(&value))
        .collect()
}

fn field_name(value: &Value) -> Option<String> {
    match value {
        Value::Keyword(value) => Some(value.as_str().to_owned()),
        Value::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn bundle_digest(namespace: &[u8], source: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(CLIENT_BUNDLE_FORMAT.as_bytes());
    digest.update([0]);
    digest.update(namespace);
    digest.update([0]);
    digest.update(source);
    digest.finalize().into()
}

fn command_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'/')
        })
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':' | b'/')
        })
}

fn namespace_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
            }))
}

fn timestamp(value: &str) -> bool {
    (20..=35).contains(&value.len())
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
        && value.as_bytes().get(10) == Some(&b'T')
        && value.ends_with('Z')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptors() -> Value {
        Value::Vector(
            vec![
                map_value(vec![
                    ("command", Value::String("status".into())),
                    ("effect", Value::Keyword("read".into())),
                    (
                        "input",
                        map_value(vec![
                            ("type", Value::Keyword("map".into())),
                            ("required", Value::Set(Default::default())),
                            ("optional", Value::Set(Default::default())),
                        ]),
                    ),
                ]),
                map_value(vec![
                    (
                        "command",
                        Value::String("pairing.invitation.issue".into()),
                    ),
                    ("effect", Value::Keyword("write".into())),
                    (
                        "input",
                        map_value(vec![
                            ("type", Value::Keyword("map".into())),
                            (
                                "required",
                                Value::Set(vec![Value::Keyword("id".into())].into()),
                            ),
                            ("optional", Value::Set(Default::default())),
                        ]),
                    ),
                ]),
            ]
            .into(),
        )
    }

    fn grant(write: bool) -> ConsoleGrant {
        ConsoleGrant {
            console: "console.test".into(),
            commands: ["status".into(), "pairing.invitation.issue".into()]
                .into_iter()
                .collect(),
            write,
        }
    }

    #[test]
    fn defaults_match_the_public_console_contract() {
        assert_eq!(
            ConsoleLimits::default(),
            ConsoleLimits {
                source_bytes: 65_536,
                result_bytes: 1_048_576,
                evaluation_millis: 5_000,
                memory_bytes: 134_217_728,
            }
        );
    }

    #[test]
    fn client_bundle_detects_mutation() {
        let bundle = ClientBundle::new(
            "tahto.console".into(),
            "(ns tahto.console (:config {}))".into(),
        )
        .unwrap();
        let mut encoded = bundle.encode().unwrap();
        assert_eq!(ClientBundle::decode(&encoded).unwrap(), bundle);
        *encoded.last_mut().unwrap() ^= 1;
        assert!(ClientBundle::decode(&encoded)
            .unwrap_err()
            .contains("digest mismatch"));
    }

    #[test]
    fn write_descriptors_and_calls_require_a_write_grant() {
        let commands = CommandSet::parse(descriptors()).unwrap();
        let read_only = grant(false);
        let granted = commands.granted_value(&read_only);
        assert_eq!(value_sequence(&granted).unwrap().len(), 1);
        assert_eq!(
            commands
                .validate_call(
                    &read_only,
                    "pairing.invitation.issue",
                    &map_value(vec![("id", Value::String("invite.one".into()))]),
                    1024,
                )
                .unwrap_err(),
            "hoplite.console/write-not-granted"
        );
        assert!(commands
            .validate_call(
                &grant(true),
                "pairing.invitation.issue",
                &map_value(vec![("id", Value::String("invite.one".into()))]),
                1024,
            )
            .is_ok());
    }

    #[test]
    fn input_maps_are_exact_and_unlisted_commands_are_rejected() {
        let commands = CommandSet::parse(descriptors()).unwrap();
        assert_eq!(
            commands
                .validate_call(&grant(true), "runtime.eval", &map_value(vec![]), 1024)
                .unwrap_err(),
            "hoplite.console/command-unlisted"
        );
        assert!(commands
            .validate_call(
                &grant(true),
                "status",
                &map_value(vec![("extra", Value::Bool(true))]),
                1024,
            )
            .unwrap_err()
            .contains("unsupported input field"));
    }
}
