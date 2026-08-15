use hara_wasm::core::Value;
use hara_wasm::kernel::{parse, Form};
use hara_wasm::project::{self, Project};
use semver::Version;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

pub const PLATFORM_FORMAT: i64 = 2;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub modules: Vec<ModuleActivation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleActivation {
    pub id: String,
    pub version: Version,
    pub export: String,
    pub alias: String,
    pub config: Form,
    pub archive_sha256: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            modules: Vec::new(),
        }
    }
}

pub fn load(project: &Project, requested_profile: Option<&str>) -> Result<Config, String> {
    let selected = project
        .resolve_profile(requested_profile)?
        .ok_or("Hoplite requires :project/profiles with :profile/language :hoplite")?;
    let source = fs::read_to_string(&project.manifest_path)
        .map_err(|error| format!("cannot read {}: {error}", project.manifest_path.display()))?;
    let manifest =
        parse(&source).map_err(|error| format!("{}: {error}", project.manifest_path.display()))?;
    let mut config = parse_profile(&manifest, &selected.name)?;
    bind_lock(project, &mut config)?;
    Ok(config)
}

fn bind_lock(project: &Project, config: &mut Config) -> Result<(), String> {
    if config.modules.is_empty() {
        return Ok(());
    }
    let path = project.root.join("project.lock.edn");
    let source = fs::read_to_string(&path).map_err(|error| {
        format!(
            "explicit Hoplite modules require {}: {error}",
            path.display()
        )
    })?;
    let packages = hara_wasm::package_catalog::catalog_from_lock(&source)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let mut resolved = BTreeMap::new();
    for package in packages {
        let coordinate = normalize_module_coordinate(&package.coordinate)?;
        let version = Version::parse(&package.version)
            .map_err(|error| format!("locked package version is invalid: {error}"))?;
        if resolved
            .insert(coordinate.clone(), (version, package))
            .is_some()
        {
            return Err(format!("duplicate locked package {coordinate}"));
        }
    }
    for module in &mut config.modules {
        let (version, package) = resolved
            .get(&module.id)
            .ok_or_else(|| format!("project.lock.edn does not lock module {}", module.id))?;
        if version != &module.version {
            return Err(format!(
                "module {} requests {} but project.lock.edn pins {}",
                module.id, module.version, version
            ));
        }
        crate::package::ensure_locked(package)?;
        module.archive_sha256 = Some(format!(
            "sha256:{}",
            package
                .archive_sha256
                .strip_prefix("sha256:")
                .unwrap_or(&package.archive_sha256)
        ));
    }
    Ok(())
}

fn parse_profile(manifest: &Form, profile_name: &str) -> Result<Config, String> {
    let project = form_map(manifest, "project.edn must be an EDN map")?;
    let profiles = match lookup(project, "project/profiles") {
        Some(value) => form_map(value, ":project/profiles must be a map")?,
        None => return Ok(Config::default()),
    };
    let profile = profiles
        .iter()
        .find_map(|(key, value)| {
            (identifier(key).as_deref() == Some(profile_name)).then_some(value)
        })
        .ok_or_else(|| format!("project.edn has no profile {profile_name:?}"))?;
    let profile = form_map(profile, "selected Hoplite profile must be a map")?;
    let Some(extensions) = lookup(profile, "profile/extensions") else {
        return Ok(Config::default());
    };
    let extensions = form_map(extensions, ":profile/extensions must be a map")?;
    let Some(hoplite) = lookup(extensions, "extension/hoplite") else {
        return Ok(Config::default());
    };
    let hoplite = form_map(hoplite, ":extension/hoplite must be a map")?;
    reject_unknown_hoplite_keys(hoplite)?;
    let modules = lookup(hoplite, "hoplite/modules")
        .map(parse_modules)
        .transpose()?
        .unwrap_or_default();
    Ok(Config { modules })
}

fn reject_unknown_hoplite_keys(entries: &[(Form, Form)]) -> Result<(), String> {
    for (key, _) in entries {
        let Some(key) = identifier(key) else {
            return Err(":extension/hoplite keys must be keywords".into());
        };
        if key != "hoplite/modules" {
            return Err(format!("unsupported Hoplite profile key :{key}"));
        }
    }
    Ok(())
}

fn parse_modules(value: &Form) -> Result<Vec<ModuleActivation>, String> {
    let Form::Vector(entries) = value else {
        return Err(":hoplite/modules must be a vector".into());
    };
    let mut modules = Vec::with_capacity(entries.len());
    let mut activations = BTreeSet::new();
    let mut aliases = BTreeSet::new();
    for entry in entries {
        let entry = form_map(entry, "each Hoplite module activation must be a map")?;
        reject_unknown_keys(
            entry,
            &[
                "module/id",
                "module/version",
                "module/export",
                "module/as",
                "module/config",
            ],
            "module",
        )?;
        let id = scalar(
            required(entry, "module/id", "module activation")?,
            ":module/id",
        )?;
        let id = normalize_module_coordinate(&id)?;
        let version_text = string(
            required(entry, "module/version", "module activation")?,
            ":module/version",
        )?;
        let version = Version::parse(&version_text)
            .map_err(|error| format!(":module/version must be exact SemVer: {error}"))?;
        let export = scalar(
            required(entry, "module/export", "module activation")?,
            ":module/export",
        )?;
        if !export.contains('/') || export.chars().any(char::is_whitespace) {
            return Err(
                ":module/export must be a qualified identifier such as :hoplite/auth".into(),
            );
        }
        if !activations.insert((id.clone(), export.clone())) {
            return Err(format!("duplicate Hoplite module export {id:?} :{export}"));
        }
        let alias = scalar(
            required(entry, "module/as", "module activation")?,
            ":module/as",
        )?;
        if alias.is_empty() || alias.contains('/') || alias.chars().any(char::is_whitespace) {
            return Err(":module/as must be a non-empty unqualified identifier".into());
        }
        if !aliases.insert(alias.clone()) {
            return Err(format!("duplicate Hoplite module alias :{alias}"));
        }
        let config = lookup(entry, "module/config")
            .cloned()
            .unwrap_or_else(|| Form::Map(Vec::new()));
        form_map(&config, ":module/config must be a map")?;
        validate_inert_form(&config, ":module/config")?;
        modules.push(ModuleActivation {
            id,
            version,
            export,
            alias,
            config,
            archive_sha256: None,
        });
    }
    Ok(modules)
}

fn normalize_module_coordinate(value: &str) -> Result<String, String> {
    if let Some(repository) = value.strip_prefix("gh:") {
        let mut parts = repository.split(':');
        let valid = matches!(
            (parts.next(), parts.next(), parts.next()),
            (Some(owner), Some(repo), None)
                if valid_github_name(owner) && valid_github_name(repo)
        );
        return valid.then(|| value.to_owned()).ok_or_else(|| {
            format!("invalid GitHub module coordinate {value:?}; expected gh:owner:repo")
        });
    }
    if !value.contains(':') {
        return Err(
            ":module/id must be registry-qualified, for example \"gh:greenways-ai:hoplite\"".into(),
        );
    }
    project::normalize_coordinate(value)
        .map_err(|_| format!("invalid Hoplite module coordinate {value:?}"))
}

fn valid_github_name(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

pub fn manifest(config: &Config) -> Result<Vec<u8>, String> {
    hara_wasm::hta::encode(&to_value(config)?)
}

pub fn readable_manifest(config: &Config) -> String {
    format!("{}\n", to_form(config))
}

fn to_form(config: &Config) -> Form {
    let fields = vec![
        (keyword("hoplite/format"), Form::Number(PLATFORM_FORMAT)),
        (
            keyword("hoplite/modules"),
            Form::Vector(
                config
                    .modules
                    .iter()
                    .map(|module| {
                        let mut fields = vec![
                            (keyword("module/id"), Form::String(module.id.clone())),
                            (
                                keyword("module/version"),
                                Form::String(module.version.to_string()),
                            ),
                            (keyword("module/export"), keyword(&module.export)),
                            (keyword("module/as"), keyword(&module.alias)),
                            (keyword("module/config"), module.config.clone()),
                        ];
                        if let Some(digest) = &module.archive_sha256 {
                            fields.push((
                                keyword("module/archive-sha256"),
                                Form::String(digest.clone()),
                            ));
                        }
                        Form::Map(fields)
                    })
                    .collect(),
            ),
        ),
    ];
    Form::Map(fields)
}

fn to_value(config: &Config) -> Result<Value, String> {
    form_to_value(&to_form(config))
}

fn form_to_value(form: &Form) -> Result<Value, String> {
    match form {
        Form::Nil => Ok(Value::Nil),
        Form::Bool(value) => Ok(Value::Bool(*value)),
        Form::Number(value) => Ok(Value::Number(*value)),
        Form::Float(value) => Ok(Value::Float(*value)),
        Form::BigInteger(value) => Ok(Value::BigInteger(value.clone())),
        Form::Decimal(value) => Ok(Value::Decimal(value.clone())),
        Form::Character(value) => Ok(Value::Character(*value)),
        Form::Regex(value) => Ok(Value::Regex(value.clone())),
        Form::String(value) => Ok(Value::String(value.clone())),
        Form::Keyword(value) => Ok(Value::Keyword(value.clone().into())),
        Form::Symbol(value) => Ok(Value::Symbol(value.clone().into())),
        Form::Vector(values) => Ok(Value::Vector(
            values
                .iter()
                .map(form_to_value)
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        )),
        Form::List(values) => Ok(Value::List(
            values
                .iter()
                .map(form_to_value)
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        )),
        Form::Set(values) => Ok(Value::OrderedSet(Box::new(
            values
                .iter()
                .map(form_to_value)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .collect(),
        ))),
        Form::Map(entries) => Ok(Value::OrderedMap(Box::new(
            entries
                .iter()
                .map(|(key, value)| Ok((form_to_value(key)?, form_to_value(value)?)))
                .collect::<Result<Vec<_>, String>>()?
                .into_iter()
                .collect(),
        ))),
        Form::Tagged(_, _) | Form::Metadata(_, _) => {
            Err("Hoplite module configuration must be inert EDN without tags or metadata".into())
        }
    }
}

fn validate_inert_form(form: &Form, label: &str) -> Result<(), String> {
    match form {
        Form::Tagged(_, _) | Form::Metadata(_, _) => {
            Err(format!("{label} cannot contain tags or metadata"))
        }
        Form::Map(entries) => {
            for (key, value) in entries {
                validate_inert_form(key, label)?;
                validate_inert_form(value, label)?;
            }
            Ok(())
        }
        Form::Vector(values) | Form::List(values) | Form::Set(values) => {
            for value in values {
                validate_inert_form(value, label)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn required<'a>(entries: &'a [(Form, Form)], key: &str, owner: &str) -> Result<&'a Form, String> {
    lookup(entries, key).ok_or_else(|| format!("{owner} requires :{key}"))
}

fn reject_unknown_keys(
    entries: &[(Form, Form)],
    allowed: &[&str],
    owner: &str,
) -> Result<(), String> {
    for (key, _) in entries {
        let Some(key) = identifier(key) else {
            return Err(format!("{owner} keys must be keywords"));
        };
        if !allowed.contains(&key.as_str()) {
            return Err(format!("unsupported {owner} key :{key}"));
        }
    }
    Ok(())
}

fn form_map<'a>(form: &'a Form, message: &str) -> Result<&'a [(Form, Form)], String> {
    match form {
        Form::Map(entries) => Ok(entries),
        _ => Err(message.into()),
    }
}

fn lookup<'a>(entries: &'a [(Form, Form)], key: &str) -> Option<&'a Form> {
    entries.iter().find_map(|(candidate, value)| {
        (identifier(candidate).as_deref() == Some(key)).then_some(value)
    })
}

fn identifier(form: &Form) -> Option<String> {
    match form {
        Form::Keyword(value) | Form::Symbol(value) | Form::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn scalar(form: &Form, label: &str) -> Result<String, String> {
    identifier(form).ok_or_else(|| format!("{label} must be a string, symbol, or keyword"))
}

fn string(form: &Form, label: &str) -> Result<String, String> {
    match form {
        Form::String(value) => Ok(value.clone()),
        _ => Err(format!("{label} must be a string")),
    }
}

fn keyword(value: &str) -> Form {
    Form::Keyword(value.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_manifest(profile: &str) -> Form {
        parse(
            &[
                "{:project/profiles {:server {:profile/extensions {:extension/hoplite ",
                profile,
                "}}}}",
            ]
            .concat(),
        )
        .unwrap()
    }

    #[test]
    fn default_platform_has_no_authentication_or_session_boundary() {
        let config = parse_profile(&project_manifest("{}"), "server").unwrap();
        let readable = readable_manifest(&config);
        assert!(!readable.contains("hoplite/authentication"));
        assert!(!readable.contains("auth/realms"));

        let error = parse_profile(
            &project_manifest("{:hoplite/authentication {:auth/realms {}}}"),
            "server",
        )
        .unwrap_err();
        assert!(error.contains("unsupported Hoplite profile key :hoplite/authentication"));
    }

    #[test]
    fn rejects_unpinned_or_duplicate_modules() {
        let unqualified = project_manifest(
            "{:hoplite/modules [{:module/id \"hoplite/events\" :module/version \"1.0.0\" :module/export :hoplite/events :module/as :events}]}",
        );
        assert!(parse_profile(&unqualified, "server")
            .unwrap_err()
            .contains("registry-qualified"));

        let unpinned = project_manifest(
            "{:hoplite/modules [{:module/id \"gh:greenways-ai:hoplite\" :module/version \"^1.0\" :module/export :hoplite/events :module/as :events}]}",
        );
        assert!(parse_profile(&unpinned, "server")
            .unwrap_err()
            .contains("exact SemVer"));

        let duplicate = project_manifest(
            "{:hoplite/modules [{:module/id \"gh:greenways-ai:hoplite\" :module/version \"1.0.0\" :module/export :hoplite/events :module/as :events} {:module/id \"gh:greenways-ai:hoplite\" :module/version \"1.0.1\" :module/export :hoplite/events :module/as :other-events}]}",
        );
        assert!(parse_profile(&duplicate, "server")
            .unwrap_err()
            .contains("duplicate"));

        let duplicate_alias = project_manifest(
            "{:hoplite/modules [{:module/id \"gh:greenways-ai:hoplite\" :module/version \"1.0.0\" :module/export :hoplite/auth :module/as :service} {:module/id \"gh:greenways-ai:hoplite\" :module/version \"1.0.0\" :module/export :hoplite/gateway :module/as :service}]}",
        );
        assert!(parse_profile(&duplicate_alias, "server")
            .unwrap_err()
            .contains("alias"));
    }
}
