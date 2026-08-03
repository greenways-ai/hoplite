use hara_wasm::core::{self, Value};
use hara_wasm::Runtime;

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
            "(ns hoplite.store.sqlite (:require [hoplite.core :as h])) (def adapter (h/adapter {:adapter/export :hoplite/store :adapter/implements {:hoplite/auth-store \"1.0.0\"}}))",
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
            "(ns hoplite.store.sqlite (:require [hoplite.core :as h])) (def adapter (h/adapter {:adapter/export :hoplite/store :adapter/implements {:hoplite/auth-store \"2.0.0\"}}))",
        )
        .unwrap();
        assert!(validate_root(&composition, &root)
            .unwrap_err()
            .contains("must be \"1.0.0\""));
        fs::remove_dir_all(root).unwrap();
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
