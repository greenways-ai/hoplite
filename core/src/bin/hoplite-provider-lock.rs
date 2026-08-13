#[path = "../../../migration/provider-products/src/provider_lock.rs"]
mod provider_lock;
#[path = "../../../migration/provider-products/src/provider_manifest.rs"]
mod provider_manifest;

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../migration/provider-products/bin/hoplite-provider-lock.rs"
));
