use hara_wasm::core::{self, Value};
use hara_wasm::Runtime;
use std::collections::BTreeSet;
use std::path::Path;

struct NativeBackend {
    package: &'static str,
    export: &'static str,
    crate_name: &'static str,
    abi: &'static str,
    open: fn(&Path) -> Result<Box<dyn crate::auth::AuthStore>, String>,
    open_native: fn(&Path) -> Result<Box<dyn hoplite_auth_store_abi::Adapter>, String>,
}

const NATIVE_BACKENDS: &[NativeBackend] = &[NativeBackend {
    package: crate::platform::SQLITE_STORE_PACKAGE,
    export: crate::platform::STORE_EXPORT,
    crate_name: "hoplite-store-sqlite",
    abi: hoplite_auth_store_abi::NATIVE_ABI,
    open: open_bundled_sqlite,
    open_native: open_linked_sqlite,
}];

pub fn open(
    path: &Path,
    composition: &crate::platform::AuthComposition,
) -> Result<Box<dyn crate::auth::AuthStore>, String> {
    validate(composition)?;
    let backend = NATIVE_BACKENDS
        .iter()
        .find(|backend| {
            backend.package == composition.store_package
                && backend.export == composition.store_export
                && backend.abi == hoplite_auth_store_abi::NATIVE_ABI
        })
        .ok_or_else(|| format!(
            "authentication store adapter {} :{} is resolved but was not linked into the native registry",
            composition.store_package, composition.store_export
        ))?;
    (backend.open)(path)
}

fn open_bundled_sqlite(path: &Path) -> Result<Box<dyn crate::auth::AuthStore>, String> {
    Ok(Box::new(crate::auth::SqliteStore::open(path)?))
}

fn open_linked_sqlite(path: &Path) -> Result<Box<dyn hoplite_auth_store_abi::Adapter>, String> {
    Ok(Box::new(hoplite_store_sqlite::SqliteAuthStore::open(path)?))
}

pub fn open_native(
    path: &Path,
    composition: &crate::platform::AuthComposition,
) -> Result<Box<dyn hoplite_auth_store_abi::Adapter>, String> {
    validate(composition)?;
    let backend = NATIVE_BACKENDS
        .iter()
        .find(|backend| {
            backend.package == composition.store_package
                && backend.export == composition.store_export
                && backend.abi == hoplite_auth_store_abi::NATIVE_ABI
        })
        .ok_or_else(|| {
            format!(
                "authentication store adapter {} :{} is not linked",
                composition.store_package, composition.store_export
            )
        })?;
    (backend.open_native)(path)
}

pub struct NativeLinkPlan {
    pub manifest_edn: String,
    pub cargo_toml: String,
}

pub fn native_link_plan(
    composition: &crate::platform::AuthComposition,
) -> Result<NativeLinkPlan, String> {
    if !composition.explicit {
        let backend = NATIVE_BACKENDS
            .iter()
            .find(|backend| backend.package == composition.store_package)
            .ok_or("default authentication backend is not linked")?;
        return Ok(NativeLinkPlan {
            manifest_edn: format!(
                "{{:native/format 1 :native/adapters [{{:package/id {:?} :package/version {:?} :adapter/export :{} :native/crate {:?} :native/abi {:?} :native/source :bundled}}]}}\n",
                composition.store_package,
                composition.store_version.to_string(),
                composition.store_export,
                backend.crate_name,
                backend.abi
            ),
            cargo_toml: "# No external native adapters; SQLite is bundled.\n".into(),
        });
    }
    validate(composition)?;
    let root = crate::package::installed_root_locked(
        &composition.store_package,
        &composition.store_version,
        composition.store_archive_sha256.as_deref(),
    )?;
    let crate_name = composition
        .store_package
        .rsplit([':', '/'])
        .next()
        .ok_or("store package coordinate has no crate name")?;
    let crate_root = root.join("crate");
    if !crate_root.join("Cargo.toml").is_file() {
        return Err(format!(
            "native adapter {} has no crate/Cargo.toml",
            composition.store_package
        ));
    }
    let cargo_path = toml_string(&crate_root.to_string_lossy());
    Ok(NativeLinkPlan {
        manifest_edn: format!(
            "{{:native/format 1 :native/adapters [{{:package/id {:?} :package/version {:?} :package/archive-sha256 {:?} :adapter/export :{} :native/crate {:?} :native/abi {:?} :native/source :harp}}]}}\n",
            composition.store_package,
            composition.store_version.to_string(),
            composition.store_archive_sha256,
            composition.store_export,
            crate_name,
            hoplite_auth_store_abi::NATIVE_ABI
        ),
        cargo_toml: format!(
            "[dependencies]\n{crate_name} = {{ path = {cargo_path:?} }}\n\n[patch.\"https://github.com/greenways-ai/hoplite\"]\nhoplite-auth-store-abi = {{ path = {:?} }}\n",
            format!("{}/abi/auth-store", env!("CARGO_MANIFEST_DIR"))
        ),
    })
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn validate(composition: &crate::platform::AuthComposition) -> Result<(), String> {
    if !composition.explicit {
        return Ok(());
    }
    let root = crate::package::installed_root_locked(
        &composition.store_package,
        &composition.store_version,
        composition.store_archive_sha256.as_deref(),
    )?;
    validate_root(composition, &root)
}

fn validate_root(
    composition: &crate::platform::AuthComposition,
    root: &std::path::Path,
) -> Result<(), String> {
    let project = hara_wasm::project::read(&root)?;
    let mut runtime = Runtime::new();
    runtime.register_resource("hoplite.core", crate::app::CORE_SOURCE);
    hara_wasm::project::register_sources(&project, &mut runtime)?;
    let namespace = store_namespace(&composition.store_package, &composition.store_export);
    let value = runtime.eval_native_value(&format!(
        "(ns hoplite.store.activation (:require [{namespace} :as store])) store/adapter"
    ))?;
    expect_keyword(&value, "hoplite/type", "adapter")?;
    expect_keyword(&value, "adapter/export", &composition.store_export)?;
    let contracts = field(&value, "adapter/implements")
        .ok_or("store adapter is missing :adapter/implements")?;
    expect_string(
        &contracts,
        "hoplite/auth-store",
        crate::platform::PRINCIPAL_CONTRACT,
    )?;
    expect_operations(&value)?;
    expect_native_artifact(&value, &composition.store_package)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn installed_adapter_must_export_the_exact_contract() {
        let root = std::env::temp_dir().join(format!(
            "hoplite-store-adapter-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src/hoplite/store")).unwrap();
        fs::write(
            root.join("project.edn"),
            "{:hara/type :project :hara/version \"1.0.0\" :project/id \"gh:greenways-ai/hoplite-store-sqlite\" :project/version \"0.1.0\" :project/source-paths [\"src\"] :project/test-paths [] :project/extension-paths [] :project/capabilities #{}}",
        )
        .unwrap();
        fs::write(
            root.join("src/hoplite/store/sqlite.hal"),
            "(ns hoplite.store.sqlite (:require [hoplite.core :as h])) (def adapter (h/adapter {:adapter/export :hoplite/store :adapter/implements {:hoplite/auth-store \"1.0.0\"} :adapter/operations #{:auth/user-create :auth/user-find :auth/device-put :auth/challenge-put :auth/challenge-consume :auth/session-put :auth/refresh-rotate :auth/session-revoke :auth/audit-append} :adapter/native {:crate \"hoplite-store-sqlite\" :abi \"hoplite-auth-store/1\"}}))",
        )
        .unwrap();
        let composition = crate::platform::AuthComposition {
            policy_package: crate::platform::CORE_PACKAGE.into(),
            policy_version: Version::parse("0.1.0").unwrap(),
            policy_export: crate::platform::CORE_AUTH_EXPORT.into(),
            policy_archive_sha256: None,
            store_package: crate::platform::SQLITE_STORE_PACKAGE.into(),
            store_version: Version::parse("0.1.0").unwrap(),
            store_export: crate::platform::STORE_EXPORT.into(),
            store_archive_sha256: None,
            explicit: true,
        };
        validate_root(&composition, &root).unwrap();

        fs::write(
            root.join("src/hoplite/store/sqlite.hal"),
            "(ns hoplite.store.sqlite (:require [hoplite.core :as h])) (def adapter (h/adapter {:adapter/export :hoplite/store :adapter/implements {:hoplite/auth-store \"2.0.0\"} :adapter/operations #{:auth/user-create :auth/user-find :auth/device-put :auth/challenge-put :auth/challenge-consume :auth/session-put :auth/refresh-rotate :auth/session-revoke :auth/audit-append} :adapter/native {:crate \"hoplite-store-sqlite\" :abi \"hoplite-auth-store/1\"}}))",
        )
        .unwrap();
        assert!(validate_root(&composition, &root)
            .unwrap_err()
            .contains("must be \"1.0.0\""));

        fs::write(
            root.join("src/hoplite/store/sqlite.hal"),
            "(ns hoplite.store.sqlite (:require [hoplite.core :as h])) (def adapter (h/adapter {:adapter/export :hoplite/store :adapter/implements {:hoplite/auth-store \"1.0.0\"} :adapter/operations #{:auth/user-find}}))",
        )
        .unwrap();
        assert!(validate_root(&composition, &root)
            .unwrap_err()
            .contains("operations must exactly implement"));

        fs::write(
            root.join("src/hoplite/store/sqlite.hal"),
            "(ns hoplite.store.sqlite (:require [hoplite.core :as h])) (def adapter (h/adapter {:adapter/export :hoplite/store :adapter/implements {:hoplite/auth-store \"1.0.0\"} :adapter/operations #{:auth/user-create :auth/user-find :auth/device-put :auth/challenge-put :auth/challenge-consume :auth/session-put :auth/refresh-rotate :auth/session-revoke :auth/audit-append} :adapter/native {:crate \"hoplite-store-sqlite\" :abi \"hoplite-auth-store/2\"}}))",
        )
        .unwrap();
        assert!(validate_root(&composition, &root)
            .unwrap_err()
            .contains(":abi must be \"hoplite-auth-store/1\""));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolved_adapters_never_fall_back_to_sqlite() {
        let composition = crate::platform::AuthComposition {
            policy_package: crate::platform::CORE_PACKAGE.into(),
            policy_version: Version::parse("0.1.0").unwrap(),
            policy_export: crate::platform::CORE_AUTH_EXPORT.into(),
            policy_archive_sha256: None,
            store_package: "gh:greenways-ai:hoplite-store-pglite".into(),
            store_version: Version::parse("0.1.0").unwrap(),
            store_export: crate::platform::STORE_EXPORT.into(),
            store_archive_sha256: None,
            explicit: false,
        };
        let error = match open(Path::new(":memory:"), &composition) {
            Ok(_) => panic!("unknown adapter unexpectedly opened"),
            Err(error) => error,
        };
        assert!(error.contains("was not linked into the native registry"));
    }

    #[test]
    fn bundled_backend_has_a_closed_native_link_plan() {
        let composition = crate::platform::Config::default()
            .auth_composition()
            .unwrap();
        let plan = native_link_plan(&composition).unwrap();
        assert!(plan.manifest_edn.contains(":native/source :bundled"));
        assert!(plan.manifest_edn.contains("hoplite-auth-store/1"));
        assert!(plan.cargo_toml.contains("No external native adapters"));
        let mut adapter = open_native(Path::new(":memory:"), &composition).unwrap();
        let request = hoplite_auth_store_abi::NativeRequest {
            id: "registry-test".into(),
            operation: *hoplite_auth_store_abi::operation("auth/user-create").unwrap(),
            payload: std::collections::BTreeMap::from([
                (
                    "user/id".into(),
                    hoplite_auth_store_abi::Value::String("usr_registry".into()),
                ),
                (
                    "user/realm".into(),
                    hoplite_auth_store_abi::Value::String("management".into()),
                ),
                (
                    "user/created-at".into(),
                    hoplite_auth_store_abi::Value::Integer(1),
                ),
            ]),
        };
        assert_eq!(adapter.execute(request).unwrap().id, "registry-test");
    }
}

fn store_namespace(package: &str, export: &str) -> String {
    if package == crate::platform::SQLITE_STORE_PACKAGE && export == crate::platform::STORE_EXPORT {
        "hoplite.store.sqlite".into()
    } else {
        export.replace('/', ".")
    }
}

fn field(value: &Value, name: &str) -> Option<Value> {
    core::map_entries(value)?
        .into_iter()
        .find_map(|(key, value)| {
            matches!(&key, Value::Keyword(keyword) if keyword.as_str() == name).then_some(value)
        })
}

fn expect_keyword(value: &Value, field_name: &str, expected: &str) -> Result<(), String> {
    match field(value, field_name) {
        Some(Value::Keyword(value)) if value.as_str() == expected => Ok(()),
        value => Err(format!(
            "store adapter :{field_name} must be :{expected}, got {value:?}"
        )),
    }
}

fn expect_string(value: &Value, field_name: &str, expected: &str) -> Result<(), String> {
    match field(value, field_name) {
        Some(Value::String(value)) if value == expected => Ok(()),
        value => Err(format!(
            "store adapter :{field_name} must be {expected:?}, got {value:?}"
        )),
    }
}

fn expect_operations(value: &Value) -> Result<(), String> {
    let operations =
        field(value, "adapter/operations").ok_or("store adapter is missing :adapter/operations")?;
    let actual: BTreeSet<String> = match &operations {
        Value::Set(values) => values
            .iter()
            .map(operation_name)
            .collect::<Result<_, _>>()?,
        Value::OrderedSet(values) => values
            .iter()
            .map(operation_name)
            .collect::<Result<_, _>>()?,
        Value::SortedSet(values) => values
            .iter()
            .map(operation_name)
            .collect::<Result<_, _>>()?,
        _ => return Err("store adapter :adapter/operations must be a set of keywords".into()),
    };
    let expected = hoplite_auth_store_abi::OPERATIONS
        .iter()
        .map(|operation| operation.name.to_owned())
        .collect::<BTreeSet<_>>();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "store adapter operations must exactly implement hoplite/auth-store 1.0.0; expected {expected:?}, got {actual:?}"
        ))
    }
}

fn operation_name(value: &Value) -> Result<String, String> {
    match value {
        Value::Keyword(keyword) => Ok(keyword.as_str().to_owned()),
        value => Err(format!(
            "store adapter :adapter/operations contains a non-keyword value: {value:?}"
        )),
    }
}

fn expect_native_artifact(value: &Value, package: &str) -> Result<(), String> {
    let native = field(value, "adapter/native")
        .ok_or("store adapter is missing :adapter/native artifact metadata")?;
    let expected_crate = package
        .rsplit([':', '/'])
        .next()
        .ok_or("store adapter package coordinate has no artifact name")?;
    expect_string(&native, "crate", expected_crate)?;
    expect_string(&native, "abi", hoplite_auth_store_abi::NATIVE_ABI)
}
