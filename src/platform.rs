use hara_wasm::core::Value;
use hara_wasm::kernel::{parse, Form};
use hara_wasm::project::{self, Project};
use semver::Version;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

pub const PLATFORM_FORMAT: i64 = 2;
pub const PRINCIPAL_CONTRACT: &str = "1.0.0";
pub const PRINCIPAL_FIELDS: &[&str] = &[
    "principal/id",
    "principal/realm",
    "principal/session-id",
    "principal/claims",
];
pub const KEY_PROVIDER: &str = "auth/key";
pub const CORE_PACKAGE: &str = "gh:greenways-ai:hoplite";
pub const CORE_AUTH_EXPORT: &str = "hoplite/auth";
pub const SQLITE_STORE_PACKAGE: &str = "gh:greenways-ai:hoplite-store-sqlite";
pub const STORE_EXPORT: &str = "hoplite/store";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthComposition {
    pub policy_package: String,
    pub policy_version: Version,
    pub policy_export: String,
    pub store_package: String,
    pub store_version: Version,
    pub store_export: String,
    pub explicit: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub modules: Vec<ModuleActivation>,
    pub authentication: Authentication,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleActivation {
    pub id: String,
    pub version: Version,
    pub export: String,
    pub alias: String,
    pub config: Form,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authentication {
    pub realms: BTreeMap<String, Realm>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Realm {
    pub providers: Vec<String>,
    pub required: bool,
    pub session: SessionPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionPolicy {
    pub access_ttl_seconds: u32,
    pub refresh_ttl_seconds: u32,
    pub rotate_refresh_tokens: bool,
    pub reuse_interval_seconds: u32,
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            access_ttl_seconds: 900,
            refresh_ttl_seconds: 2_592_000,
            rotate_refresh_tokens: true,
            reuse_interval_seconds: 10,
        }
    }
}

impl Default for Authentication {
    fn default() -> Self {
        Self {
            realms: BTreeMap::from([
                (
                    "application".into(),
                    Realm {
                        providers: vec![KEY_PROVIDER.into()],
                        required: false,
                        session: SessionPolicy::default(),
                    },
                ),
                (
                    "management".into(),
                    Realm {
                        providers: vec![KEY_PROVIDER.into()],
                        required: true,
                        session: SessionPolicy::default(),
                    },
                ),
            ]),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            modules: Vec::new(),
            authentication: Authentication::default(),
        }
    }
}

impl Config {
    pub fn auth_composition(&self) -> Result<AuthComposition, String> {
        let Some(policy) = self
            .modules
            .iter()
            .find(|module| module.export == CORE_AUTH_EXPORT)
        else {
            return Ok(AuthComposition {
                policy_package: CORE_PACKAGE.into(),
                policy_version: Version::parse("0.1.0").expect("valid bundled version"),
                policy_export: CORE_AUTH_EXPORT.into(),
                store_package: SQLITE_STORE_PACKAGE.into(),
                store_version: Version::parse("0.1.0").expect("valid bundled version"),
                store_export: STORE_EXPORT.into(),
                explicit: false,
            });
        };
        let config = form_map(&policy.config, ":hoplite/auth module config must be a map")?;
        let store_alias = scalar(
            required(config, "auth/store", ":hoplite/auth module config")?,
            ":auth/store",
        )?;
        let store = self
            .modules
            .iter()
            .find(|module| module.alias == store_alias)
            .ok_or_else(|| format!("authentication store alias :{store_alias} is not activated"))?;
        Ok(AuthComposition {
            policy_package: policy.id.clone(),
            policy_version: policy.version.clone(),
            policy_export: policy.export.clone(),
            store_package: store.id.clone(),
            store_version: store.version.clone(),
            store_export: store.export.clone(),
            explicit: true,
        })
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
    parse_profile(&manifest, &selected.name)
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
    let authentication = lookup(hoplite, "hoplite/authentication")
        .map(parse_authentication)
        .transpose()?
        .unwrap_or_default();
    Ok(Config {
        modules,
        authentication,
    })
}

fn reject_unknown_hoplite_keys(entries: &[(Form, Form)]) -> Result<(), String> {
    for (key, _) in entries {
        let Some(key) = identifier(key) else {
            return Err(":extension/hoplite keys must be keywords".into());
        };
        if !matches!(key.as_str(), "hoplite/modules" | "hoplite/authentication") {
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

fn parse_authentication(value: &Form) -> Result<Authentication, String> {
    let authentication = form_map(value, ":hoplite/authentication must be a map")?;
    reject_unknown_keys(authentication, &["auth/realms"], "authentication")?;
    let realms = form_map(
        required(authentication, "auth/realms", "authentication")?,
        ":auth/realms must be a map",
    )?;
    let mut output = BTreeMap::new();
    for (key, realm) in realms {
        let name = identifier(key).ok_or(":auth/realms keys must be keywords")?;
        if name.is_empty() {
            return Err("authentication realm name cannot be empty".into());
        }
        if output
            .insert(name.clone(), parse_realm(&name, realm)?)
            .is_some()
        {
            return Err(format!("duplicate authentication realm :{name}"));
        }
    }
    for required_realm in ["management", "application"] {
        if !output.contains_key(required_realm) {
            return Err(format!(
                ":auth/realms must declare :{required_realm}; Hoplite separates management and application authentication"
            ));
        }
    }
    if !output["management"].required {
        return Err("the Hoplite :management realm must set :auth/required true".into());
    }
    Ok(Authentication { realms: output })
}

fn parse_realm(name: &str, value: &Form) -> Result<Realm, String> {
    let realm = form_map(value, "authentication realm must be a map")?;
    reject_unknown_keys(
        realm,
        &["auth/providers", "auth/required", "auth/session"],
        "authentication realm",
    )?;
    let providers = match lookup(realm, "auth/providers") {
        Some(Form::Vector(values)) => values
            .iter()
            .map(|value| {
                let provider = identifier(value).ok_or_else(|| {
                    format!("authentication realm :{name} providers must be keywords")
                })?;
                if !provider.contains('/') {
                    return Err(format!(
                        "authentication provider :{provider} must be qualified, for example :auth/key"
                    ));
                }
                Ok(provider)
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(format!("authentication realm :{name} :auth/providers must be a vector")),
        None => vec![KEY_PROVIDER.into()],
    };
    if providers.is_empty() {
        return Err(format!(
            "authentication realm :{name} requires at least one provider"
        ));
    }
    let mut unique = BTreeSet::new();
    for provider in &providers {
        if !unique.insert(provider) {
            return Err(format!(
                "authentication realm :{name} contains duplicate provider :{provider}"
            ));
        }
    }
    let required = match lookup(realm, "auth/required") {
        Some(Form::Bool(value)) => *value,
        Some(_) => {
            return Err(format!(
                "authentication realm :{name} :auth/required must be boolean"
            ))
        }
        None => name == "management",
    };
    let session = lookup(realm, "auth/session")
        .map(|value| parse_session(name, value))
        .transpose()?
        .unwrap_or_default();
    Ok(Realm {
        providers,
        required,
        session,
    })
}

fn parse_session(realm: &str, value: &Form) -> Result<SessionPolicy, String> {
    let session = form_map(value, ":auth/session must be a map")?;
    reject_unknown_keys(
        session,
        &[
            "session/access-ttl-seconds",
            "session/refresh-ttl-seconds",
            "session/rotate-refresh-tokens",
            "session/reuse-interval-seconds",
        ],
        "session policy",
    )?;
    let defaults = SessionPolicy::default();
    let access_ttl_seconds = unsigned_field(
        session,
        "session/access-ttl-seconds",
        defaults.access_ttl_seconds,
    )?;
    let refresh_ttl_seconds = unsigned_field(
        session,
        "session/refresh-ttl-seconds",
        defaults.refresh_ttl_seconds,
    )?;
    let rotate_refresh_tokens = bool_field(
        session,
        "session/rotate-refresh-tokens",
        defaults.rotate_refresh_tokens,
    )?;
    let reuse_interval_seconds = unsigned_field(
        session,
        "session/reuse-interval-seconds",
        defaults.reuse_interval_seconds,
    )?;
    if !(60..=3600).contains(&access_ttl_seconds) {
        return Err(format!(
            "authentication realm :{realm} access token TTL must be between 60 and 3600 seconds"
        ));
    }
    if refresh_ttl_seconds < access_ttl_seconds || refresh_ttl_seconds > 31_536_000 {
        return Err(format!(
            "authentication realm :{realm} refresh token TTL must be at least the access TTL and no more than one year"
        ));
    }
    if !rotate_refresh_tokens {
        return Err(format!(
            "authentication realm :{realm} must rotate single-use refresh tokens"
        ));
    }
    if reuse_interval_seconds > 60 {
        return Err(format!(
            "authentication realm :{realm} refresh-token reuse interval cannot exceed 60 seconds"
        ));
    }
    Ok(SessionPolicy {
        access_ttl_seconds,
        refresh_ttl_seconds,
        rotate_refresh_tokens,
        reuse_interval_seconds,
    })
}

pub fn manifest(config: &Config) -> Result<Vec<u8>, String> {
    hara_wasm::hta::encode(&to_value(config)?)
}

pub fn readable_manifest(config: &Config) -> String {
    format!("{}\n", to_form(config))
}

fn to_form(config: &Config) -> Form {
    Form::Map(vec![
        (keyword("hoplite/format"), Form::Number(PLATFORM_FORMAT)),
        (
            keyword("hoplite/modules"),
            Form::Vector(
                config
                    .modules
                    .iter()
                    .map(|module| {
                        Form::Map(vec![
                            (keyword("module/id"), Form::String(module.id.clone())),
                            (
                                keyword("module/version"),
                                Form::String(module.version.to_string()),
                            ),
                            (keyword("module/export"), keyword(&module.export)),
                            (keyword("module/as"), keyword(&module.alias)),
                            (keyword("module/config"), module.config.clone()),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            keyword("hoplite/authentication"),
            Form::Map(vec![
                (
                    keyword("auth/principal-contract"),
                    Form::String(PRINCIPAL_CONTRACT.into()),
                ),
                (
                    keyword("auth/principal-fields"),
                    Form::Vector(
                        PRINCIPAL_FIELDS
                            .iter()
                            .map(|field| keyword(field))
                            .collect(),
                    ),
                ),
                (
                    keyword("auth/realms"),
                    Form::Map(
                        config
                            .authentication
                            .realms
                            .iter()
                            .map(|(name, realm)| {
                                (
                                    keyword(name),
                                    Form::Map(vec![
                                        (
                                            keyword("auth/providers"),
                                            Form::Vector(
                                                realm
                                                    .providers
                                                    .iter()
                                                    .map(|provider| keyword(provider))
                                                    .collect(),
                                            ),
                                        ),
                                        (keyword("auth/required"), Form::Bool(realm.required)),
                                        (
                                            keyword("auth/session"),
                                            Form::Map(vec![
                                                (
                                                    keyword("session/access-ttl-seconds"),
                                                    Form::Number(i64::from(
                                                        realm.session.access_ttl_seconds,
                                                    )),
                                                ),
                                                (
                                                    keyword("session/refresh-ttl-seconds"),
                                                    Form::Number(i64::from(
                                                        realm.session.refresh_ttl_seconds,
                                                    )),
                                                ),
                                                (
                                                    keyword("session/rotate-refresh-tokens"),
                                                    Form::Bool(realm.session.rotate_refresh_tokens),
                                                ),
                                                (
                                                    keyword("session/reuse-interval-seconds"),
                                                    Form::Number(i64::from(
                                                        realm.session.reuse_interval_seconds,
                                                    )),
                                                ),
                                            ]),
                                        ),
                                    ]),
                                )
                            })
                            .collect(),
                    ),
                ),
            ]),
        ),
    ])
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

fn unsigned_field(entries: &[(Form, Form)], key: &str, default: u32) -> Result<u32, String> {
    match lookup(entries, key) {
        Some(Form::Number(value)) => u32::try_from(*value)
            .map_err(|_| format!(":{key} must be a non-negative 32-bit integer")),
        Some(_) => Err(format!(":{key} must be an integer")),
        None => Ok(default),
    }
}

fn bool_field(entries: &[(Form, Form)], key: &str, default: bool) -> Result<bool, String> {
    match lookup(entries, key) {
        Some(Form::Bool(value)) => Ok(*value),
        Some(_) => Err(format!(":{key} must be boolean")),
        None => Ok(default),
    }
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
    fn defaults_to_hoplite_owned_key_authentication() {
        let config = parse_profile(&project_manifest("{}"), "server").unwrap();
        assert!(config.modules.is_empty());
        assert_eq!(
            config.authentication.realms["management"].providers,
            [KEY_PROVIDER]
        );
        assert!(config.authentication.realms["management"].required);
        assert!(!config.authentication.realms["application"].required);
        assert_eq!(
            config.authentication.realms["application"]
                .session
                .access_ttl_seconds,
            900
        );
    }

    #[test]
    fn compiles_modules_and_separate_authentication_realms() {
        let source = r#"
        {:hoplite/modules
         [{:module/id "gh:greenways-ai:hoplite"
           :module/version "1.2.3"
           :module/export :hoplite/events
           :module/as :events
           :module/config {:events/buffer-size 2048}}]
         :hoplite/authentication
         {:auth/realms
          {:management
           {:auth/providers [:auth/key :auth/passkey]
            :auth/required true
            :auth/session {:session/access-ttl-seconds 300}}
           :application
           {:auth/providers [:auth/key]
            :auth/required false
            :auth/session {:session/refresh-ttl-seconds 86400}}}}}
        "#;
        let config = parse_profile(&project_manifest(source), "server").unwrap();
        assert_eq!(config.modules[0].id, "gh:greenways-ai:hoplite");
        assert_eq!(config.modules[0].version, Version::parse("1.2.3").unwrap());
        assert_eq!(config.modules[0].export, "hoplite/events");
        assert_eq!(config.modules[0].alias, "events");
        assert_eq!(
            config.authentication.realms["management"].providers,
            ["auth/key", "auth/passkey"]
        );
        let readable = readable_manifest(&config);
        assert!(readable.contains(":auth/principal-contract \"1.0.0\""));
        assert!(readable.contains(":principal/session-id"));
        assert!(readable.contains(":session/rotate-refresh-tokens true"));
        assert!(super::manifest(&config).unwrap().starts_with(b"HTA1"));
    }

    #[test]
    fn resolves_auth_policy_and_store_adapter_by_alias() {
        let source = r#"
        {:hoplite/modules
         [{:module/id "gh:greenways-ai:hoplite"
           :module/version "0.1.0"
           :module/export :hoplite/auth
           :module/as :auth
           :module/config {:auth/store :auth-store}}
          {:module/id "gh:greenways-ai:hoplite-store-sqlite"
           :module/version "0.1.0"
           :module/export :hoplite/store
           :module/as :auth-store
           :module/config {}}]}
        "#;
        let config = parse_profile(&project_manifest(source), "server").unwrap();
        assert_eq!(
            config.auth_composition().unwrap(),
            AuthComposition {
                policy_package: CORE_PACKAGE.into(),
                policy_version: Version::parse("0.1.0").unwrap(),
                policy_export: CORE_AUTH_EXPORT.into(),
                store_package: SQLITE_STORE_PACKAGE.into(),
                store_version: Version::parse("0.1.0").unwrap(),
                store_export: STORE_EXPORT.into(),
                explicit: true,
            }
        );

        let missing = parse_profile(
            &project_manifest(
                r#"{:hoplite/modules [{:module/id "gh:greenways-ai:hoplite" :module/version "0.1.0" :module/export :hoplite/auth :module/as :auth :module/config {:auth/store :missing}}]}"#,
            ),
            "server",
        )
        .unwrap();
        assert!(missing
            .auth_composition()
            .unwrap_err()
            .contains("store alias :missing"));
    }

    #[test]
    fn rejects_unsafe_session_and_management_configuration() {
        let no_management = project_manifest(
            "{:hoplite/authentication {:auth/realms {:application {:auth/providers [:auth/key]}}}}",
        );
        assert!(parse_profile(&no_management, "server")
            .unwrap_err()
            .contains(":management"));

        let no_rotation = project_manifest(
            "{:hoplite/authentication {:auth/realms {:management {:auth/required true :auth/session {:session/rotate-refresh-tokens false}} :application {}}}}",
        );
        assert!(parse_profile(&no_rotation, "server")
            .unwrap_err()
            .contains("must rotate"));

        let public_management = project_manifest(
            "{:hoplite/authentication {:auth/realms {:management {:auth/required false} :application {}}}}",
        );
        assert!(parse_profile(&public_management, "server")
            .unwrap_err()
            .contains("management realm"));
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
