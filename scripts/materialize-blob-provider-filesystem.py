from pathlib import Path

SOURCE = Path("core/abi/blob-store-provider-ffi/src/lib.rs")
WORKFLOW = Path(".github/workflows/blob-store-provider-filesystem.yml")
SELF = Path(__file__)

source = SOURCE.read_text()

replacements = {
    "The provider owns one application-neutral in-memory blob store per worker.":
        "The provider owns one application-neutral installed blob store per worker.",
    """use hoplite_blob_store::{
    ByteSource, DigestVerifier, Error as BlobError, InMemoryBlobStore, Limits,
    MemoryResponseSource, ResponseSource,
};
""": """use hoplite_blob_store::{
    AppendReceipt, BlobStore, ByteSource, DigestVerifier, Error as BlobError,
    InMemoryBlobStore, Limits, MemoryResponseSource, ObjectDescriptor, ObjectRange,
    ResponseSource, StagingAppend, StagingCommit, StagingKey, StagingOpen, StagingStatus,
};
use hoplite_blob_store_filesystem::{FilesystemBlobStore, FilesystemResponseSource};
""",
    "use std::panic::{catch_unwind, AssertUnwindSafe};\n":
        "use std::panic::{catch_unwind, AssertUnwindSafe};\nuse std::path::Path;\n",
    "    source: MemoryResponseSource,\n":
        "    source: ProviderResponseSource,\n",
    "    fn register(&mut self, work: u64, source: MemoryResponseSource) -> Result<u64, BlobError> {\n":
        "    fn register(&mut self, work: u64, source: ProviderResponseSource) -> Result<u64, BlobError> {\n",
    "impl ResponseSourceRegistrar<MemoryResponseSource> for HostResponseRegistrar {\n    fn register(&self, source: MemoryResponseSource) -> Result<u64, BlobError> {\n":
        "impl ResponseSourceRegistrar<ProviderResponseSource> for HostResponseRegistrar {\n    fn register(&self, source: ProviderResponseSource) -> Result<u64, BlobError> {\n",
    "// SAFETY: the caller must pass a live provider returned by open_memory_v1.\n":
        "// SAFETY: the caller must pass a live provider returned by an open function.\n",
}
for old, new in replacements.items():
    if source.count(old) != 1:
        raise SystemExit(f"expected one provider source occurrence: {old[:80]!r}")
    source = source.replace(old, new)

old = """type MemoryProvider = CanonicalProvider<
    InMemoryBlobStore<Sha256Verifier>,
    HostRequestResolver,
    HostResponseRegistrar,
>;

pub struct HopliteBlobStoreProvider {
    provider: MemoryProvider,
    call: SharedCall,
    responses: Arc<Mutex<ResponseRegistry>>,
    execution: Mutex<()>,
}
"""
new = """enum ProviderResponseSource {
    Memory(MemoryResponseSource),
    Filesystem(FilesystemResponseSource),
}

impl ResponseSource for ProviderResponseSource {
    fn declared_length(&self) -> u64 {
        match self {
            Self::Memory(source) => source.declared_length(),
            Self::Filesystem(source) => source.declared_length(),
        }
    }

    fn read(&mut self, output: &mut [u8]) -> Result<usize, BlobError> {
        match self {
            Self::Memory(source) => source.read(output),
            Self::Filesystem(source) => source.read(output),
        }
    }

    fn close(&mut self) -> Result<(), BlobError> {
        match self {
            Self::Memory(source) => source.close(),
            Self::Filesystem(source) => source.close(),
        }
    }
}

enum InstalledBlobStore {
    Memory(InMemoryBlobStore<Sha256Verifier>),
    Filesystem(FilesystemBlobStore),
}

impl BlobStore for InstalledBlobStore {
    type Source = ProviderResponseSource;

    fn staging_open(&self, request: StagingOpen) -> Result<StagingStatus, BlobError> {
        match self {
            Self::Memory(store) => store.staging_open(request),
            Self::Filesystem(store) => store.staging_open(request),
        }
    }

    fn staging_append_from_source(
        &self,
        request: StagingAppend,
        source: &mut dyn ByteSource,
    ) -> Result<AppendReceipt, BlobError> {
        match self {
            Self::Memory(store) => store.staging_append_from_source(request, source),
            Self::Filesystem(store) => store.staging_append_from_source(request, source),
        }
    }

    fn staging_abort(&self, staging_key: &StagingKey) -> Result<(), BlobError> {
        match self {
            Self::Memory(store) => store.staging_abort(staging_key),
            Self::Filesystem(store) => store.staging_abort(staging_key),
        }
    }

    fn staging_verify_commit(
        &self,
        request: StagingCommit,
    ) -> Result<ObjectDescriptor, BlobError> {
        match self {
            Self::Memory(store) => store.staging_verify_commit(request),
            Self::Filesystem(store) => store.staging_verify_commit(request),
        }
    }

    fn object_open_source(
        &self,
        request: ObjectRange,
    ) -> Result<Self::Source, BlobError> {
        match self {
            Self::Memory(store) => store
                .object_open_source(request)
                .map(ProviderResponseSource::Memory),
            Self::Filesystem(store) => store
                .object_open_source(request)
                .map(ProviderResponseSource::Filesystem),
        }
    }
}

type InstalledProvider = CanonicalProvider<
    InstalledBlobStore,
    HostRequestResolver,
    HostResponseRegistrar,
>;

pub struct HopliteBlobStoreProvider {
    provider: InstalledProvider,
    call: SharedCall,
    responses: Arc<Mutex<ResponseRegistry>>,
    execution: Mutex<()>,
}
"""
if source.count(old) != 1:
    raise SystemExit("expected the in-memory-only provider alias")
source = source.replace(old, new)

marker = "fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, BlobError> {\n"
helper = """fn build_provider(
    store: InstalledBlobStore,
    limits: Limits,
) -> Result<Box<HopliteBlobStoreProvider>, ()> {
    let call = SharedCall::new();
    let responses = Arc::new(Mutex::new(ResponseRegistry::new()));
    let provider = CanonicalProvider::new(
        store,
        HostRequestResolver { call: call.clone() },
        HostResponseRegistrar {
            call: call.clone(),
            responses: responses.clone(),
        },
        limits,
    )
    .map_err(|_| ())?;
    Ok(Box::new(HopliteBlobStoreProvider {
        provider,
        call,
        responses,
        execution: Mutex::new(()),
    }))
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, BlobError> {
"""
if source.count(marker) != 1:
    raise SystemExit("expected one provider helper insertion point")
source = source.replace(marker, helper)

memory_function = source.index(
    "pub unsafe extern \"C\" fn hoplite_blob_store_provider_open_memory_v1("
)
construction_start = source.index("        let call = SharedCall::new();", memory_function)
construction_end = source.index(
    "        // SAFETY: output is valid and receives exclusive ownership.",
    construction_start,
)
source = (
    source[:construction_start]
    + """        let store = InstalledBlobStore::Memory(
            InMemoryBlobStore::new(Sha256Verifier, limits).map_err(|_| ())?,
        );
        let provider = build_provider(store, limits)?;
"""
    + source[construction_end:]
)

marker = "/// Execute one synchronous canonical `hara.blob` operation.\n"
constructor = """/// Open one worker-owned trusted-root filesystem provider.
///
/// # Safety
///
/// `root` must be readable UTF-8 for `root_len` bytes, `limits` must
/// point to a readable limits value and `output` must point to a writable
/// provider pointer for this call. The root and limits are trusted startup
/// configuration and must not originate in a HAL request.
#[no_mangle]
pub unsafe extern "C" fn hoplite_blob_store_provider_open_filesystem_v1(
    root: *const u8,
    root_len: usize,
    limits: *const HopliteBlobStoreLimitsV1,
    output: *mut *mut HopliteBlobStoreProvider,
) -> i32 {
    if root.is_null() || root_len == 0 || limits.is_null() || output.is_null() {
        return STATUS_INVALID;
    }
    // SAFETY: output was checked non-null and is writable for this call.
    unsafe { *output = ptr::null_mut() };
    // SAFETY: root is non-null and readable for root_len bytes by contract.
    let root = match unsafe { input_bytes(root, root_len) }
        .ok()
        .and_then(|root| str::from_utf8(root).ok())
    {
        Some(root) if !root.is_empty() && !root.as_bytes().contains(&0) => root,
        _ => return STATUS_INVALID,
    };
    // SAFETY: limits was checked non-null and is copied immediately.
    let limits = match unsafe { *limits }.into_limits().validate() {
        Ok(limits) => limits,
        Err(_) => return STATUS_FAILURE,
    };
    catch_unwind(AssertUnwindSafe(|| {
        let store = FilesystemBlobStore::open(Path::new(root), limits)
            .map(InstalledBlobStore::Filesystem)
            .map_err(|_| ())?;
        let provider = build_provider(store, limits)?;
        // SAFETY: output remains valid and receives exclusive ownership.
        unsafe { *output = Box::into_raw(provider) };
        Ok::<(), ()>(())
    }))
    .ok()
    .and_then(Result::ok)
    .map(|_| STATUS_OK)
    .unwrap_or(STATUS_FAILURE)
}

/// Execute one synchronous canonical `hara.blob` operation.
"""
if source.count(marker) != 1:
    raise SystemExit("expected one filesystem constructor insertion point")
source = source.replace(marker, constructor)

marker = "#[cfg(test)]\nmod tests {\n"
replacement = "#[cfg(test)]\nmod filesystem_tests;\n\n#[cfg(test)]\nmod tests {\n"
if source.count(marker) != 1:
    raise SystemExit("expected one filesystem test-module insertion point")
source = source.replace(marker, replacement)

SOURCE.write_text(source)

WORKFLOW.write_text("""name: Filesystem hara.blob provider FFI

on:
  push:
    branches: [main]
    paths:
      - core/abi/blob-store/**
      - core/abi/blob-store-filesystem/**
      - core/abi/blob-store-provider/**
      - core/abi/blob-store-provider-ffi/**
      - .github/workflows/blob-store-provider-filesystem.yml
  pull_request:
    paths:
      - core/abi/blob-store/**
      - core/abi/blob-store-filesystem/**
      - core/abi/blob-store-provider/**
      - core/abi/blob-store-provider-ffi/**
      - .github/workflows/blob-store-provider-filesystem.yml

permissions:
  contents: read

concurrency:
  group: filesystem-hara-blob-provider-${{ github.ref }}
  cancel-in-progress: true

jobs:
  filesystem-provider:
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v5
      - uses: dtolnay/rust-toolchain@1.78.0
        with:
          components: rustfmt,clippy
      - name: Check formatting
        run: |
          cargo fmt \\
            --manifest-path core/abi/blob-store-provider-ffi/Cargo.toml \\
            --all -- --check
      - name: Reject warnings
        run: |
          cargo clippy \\
            --manifest-path core/abi/blob-store-provider-ffi/Cargo.toml \\
            --all-targets --locked -- -D warnings
      - name: Run memory and filesystem provider conformance
        run: |
          cargo test \\
            --manifest-path core/abi/blob-store-provider-ffi/Cargo.toml \\
            --all-targets --locked
""")
SELF.unlink()
