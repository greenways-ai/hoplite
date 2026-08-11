#![allow(clippy::missing_safety_doc)]

#[path = "../../src/host.rs"]
mod host_intrinsics;

use hara_wasm::{core, hta, kernel, vm};
use hoplite_data_plane_abi::{BodyLimits, ResourceHandle};
use hoplite_data_plane_ffi::HopliteRequestBodyV1;
use hoplite_data_plane_registry::ResourceRegistry;

use core::{Promise, PromiseRejection, PromiseState, Value};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::rc::Rc;
use std::{ffi::c_void, slice, str};

const ABI_VERSION: u32 = 0;
const MAX_CHILD_DRIVE_PASSES: usize = 64;
type HostCall = (u64, Promise, String, String, Vec<Value>);

type WorkId = u64;
type CallId = u64;
type HandlerId = u64;
type AppId = u64;
type RequestId = u64;
type ResponseId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouteAdapter {
    Raw,
    Request,
    RequestHta,
}

impl RouteAdapter {
    fn parse(value: Option<String>, legacy: bool) -> Result<Self, String> {
        match value.as_deref() {
            None if legacy => Ok(Self::RequestHta),
            None | Some("request") => Ok(Self::Request),
            Some("raw") => Ok(Self::Raw),
            Some("request+hta") => Ok(Self::RequestHta),
            Some(value) => Err(format!("unknown route adapter :{value}")),
        }
    }

    fn from_abi(value: u32) -> Result<Self, String> {
        match value {
            0 => Ok(Self::Raw),
            1 => Ok(Self::Request),
            2 => Ok(Self::RequestHta),
            _ => Err(format!("unknown route adapter ABI value {value}")),
        }
    }
}

#[derive(Clone)]
struct AppRoute {
    method: String,
    path: String,
    handler: HandlerId,
    adapter: RouteAdapter,
}

struct AppRouter {
    routes: Vec<AppRoute>,
}

struct Work {
    fiber: Option<vm::VmFiber>,
    result: Promise,
    children: Vec<Promise>,
    calls: HashMap<CallId, Promise>,
    request: Option<RequestId>,
}

impl Work {
    fn new(result: Promise) -> Self {
        Self {
            fiber: None,
            result,
            children: Vec::new(),
            calls: HashMap::new(),
            request: None,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HopliteSlice {
    pub data: *const u8,
    pub len: usize,
}

pub type HopliteHeaderAt = unsafe extern "C" fn(
    context: *mut c_void,
    index: usize,
    name: *mut HopliteSlice,
    value: *mut HopliteSlice,
) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HopliteRequestV2 {
    pub context: *mut c_void,
    pub method: HopliteSlice,
    pub uri: HopliteSlice,
    pub path: HopliteSlice,
    pub query_string: HopliteSlice,
    pub remote_address: HopliteSlice,
    pub header_count: usize,
    pub header_at: Option<HopliteHeaderAt>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HopliteRequestV3 {
    pub request: HopliteRequestV2,
    pub body: *const HopliteRequestBodyV1,
    pub max_body_bytes: u64,
    pub max_chunk_bytes: usize,
    /// `0` means false and `1` means true.
    pub require_declared_length: u32,
}

#[repr(C)]
pub struct HopliteOutcomeV2 {
    /// 0 = error, 1 = complete, 2 = suspended.
    pub kind: u32,
    pub id: u64,
}

#[derive(Clone)]
struct RequestRecord {
    request: HopliteRequestV2,
    body: Option<ResourceHandle>,
    identity: Option<Value>,
}

struct NativeResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

struct RawBuilder {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

enum InvokeState {
    Complete(ResponseId),
    Suspended(WorkId),
}

#[repr(C)]
pub struct HopliteBuffer {
    pub data: *mut u8,
    pub len: usize,
}

pub struct HopliteRuntime {
    namespaces: kernel::NamespaceRegistry<Value>,
    protocols: core::ProtocolRegistry,
    next_handler: HandlerId,
    next_work: WorkId,
    next_call: u64,
    next_request: RequestId,
    next_response: ResponseId,
    events: Rc<RefCell<VecDeque<Vec<u8>>>>,
    ready: Rc<RefCell<VecDeque<(u64, PromiseState)>>>,
    call_owners: HashMap<CallId, WorkId>,
    handlers: HashMap<HandlerId, vm::PreparedCall>,
    apps: HashMap<AppId, AppRouter>,
    works: HashMap<WorkId, Work>,
    requests: Rc<RefCell<HashMap<RequestId, RequestRecord>>>,
    resources: Rc<RefCell<ResourceRegistry>>,
    raw_builders: Rc<RefCell<HashMap<RequestId, RawBuilder>>>,
    responses: HashMap<ResponseId, NativeResponse>,
    host: Rc<dyn Fn(String, String, Vec<Value>) -> Result<Value, String>>,
    host_pending: Rc<RefCell<Vec<HostCall>>>,
    host_next: Rc<RefCell<u64>>,
}

fn extension_request_id(value: &Value) -> Result<RequestId, String> {
    match value {
        Value::Extension(value)
            if value.provider == "hoplite.route"
                && matches!(value.type_name.as_str(), "request" | "headers" | "exchange") =>
        {
            Ok(value.handle)
        }
        _ => Err("hoplite/request-invalid: expected a request-scoped value".into()),
    }
}

fn slice_string(value: HopliteSlice) -> Result<String, String> {
    if value.len == 0 {
        return Ok(String::new());
    }
    if value.data.is_null() {
        return Err("hoplite/request-invalid: null request slice".into());
    }
    let bytes = unsafe { slice::from_raw_parts(value.data, value.len) };
    str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| "hoplite/request-invalid: request text is not UTF-8".into())
}

fn request_record(
    requests: &Rc<RefCell<HashMap<RequestId, RequestRecord>>>,
    request: RequestId,
) -> Result<RequestRecord, String> {
    requests
        .borrow()
        .get(&request)
        .cloned()
        .ok_or_else(|| "hoplite/request-closed: request scope has ended".into())
}

fn request_headers(record: RequestRecord) -> Result<Vec<(String, String)>, String> {
    let Some(header_at) = record.request.header_at else {
        return Ok(Vec::new());
    };
    let mut headers = Vec::with_capacity(record.request.header_count);
    for index in 0..record.request.header_count {
        let mut name = HopliteSlice {
            data: ptr::null(),
            len: 0,
        };
        let mut value = name;
        if unsafe { header_at(record.request.context, index, &mut name, &mut value) } != 0 {
            return Err("hoplite/request-invalid: cannot read request header".into());
        }
        headers.push((slice_string(name)?, slice_string(value)?));
    }
    Ok(headers)
}

fn close_request_body_descriptor(descriptor: HopliteRequestBodyV1) {
    if descriptor.context.is_null() {
        return;
    }
    if let Some(close) = descriptor.close {
        unsafe { close(descriptor.context) };
    }
}

fn request_body_value(record: RequestRecord) -> Result<Option<Value>, String> {
    record
        .body
        .map(|handle| {
            i64::try_from(handle.get()).map(Value::Number).map_err(|_| {
                "hoplite/request-body-invalid: handle exceeds Hara integer range".into()
            })
        })
        .transpose()
}

fn request_entries(
    requests: &Rc<RefCell<HashMap<RequestId, RequestRecord>>>,
    request: RequestId,
) -> Result<Vec<(Value, Value)>, String> {
    let record = request_record(requests, request)?;
    let headers = request_headers(record.clone())?
        .into_iter()
        .map(|(name, value)| (Value::String(name), Value::String(value)))
        .collect();
    let mut entries = vec![
        (
            Value::Keyword("method".into()),
            Value::String(slice_string(record.request.method)?),
        ),
        (
            Value::Keyword("uri".into()),
            Value::String(slice_string(record.request.uri)?),
        ),
        (
            Value::Keyword("path".into()),
            Value::String(slice_string(record.request.path)?),
        ),
        (
            Value::Keyword("query-string".into()),
            Value::String(slice_string(record.request.query_string)?),
        ),
        (
            Value::Keyword("remote-address".into()),
            Value::String(slice_string(record.request.remote_address)?),
        ),
        (Value::Keyword("headers".into()), Value::Map(headers)),
    ];
    if let Some(body) = request_body_value(record.clone())? {
        entries.push((Value::Keyword("body-handle".into()), body));
    }
    if let Some(identity) = record.identity {
        entries.push((Value::Keyword("identity".into()), identity));
    }
    Ok(entries)
}

fn request_value(
    requests: &Rc<RefCell<HashMap<RequestId, RequestRecord>>>,
    request: RequestId,
    type_name: &str,
) -> Result<Value, String> {
    request_record(requests, request)?;
    Ok(Value::Extension(core::ExtensionValue {
        provider: "hoplite.route".into(),
        type_name: type_name.into(),
        handle: request,
    }))
}

fn request_lookup(
    requests: &Rc<RefCell<HashMap<RequestId, RequestRecord>>>,
    receiver: &core::ExtensionValue,
    key: &Value,
) -> Result<Option<Value>, String> {
    let record = request_record(requests, receiver.handle)?;
    if receiver.type_name == "headers" {
        let name = match key {
            Value::String(name) => name,
            Value::Keyword(name) => name.as_str(),
            _ => return Ok(None),
        };
        return Ok(request_headers(record)?
            .into_iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| Value::String(value)));
    }
    let name = match key {
        Value::Keyword(name) => name.as_str(),
        Value::String(name) => name,
        _ => return Ok(None),
    };
    Ok(match name {
        "method" => Some(Value::String(slice_string(record.request.method)?)),
        "uri" => Some(Value::String(slice_string(record.request.uri)?)),
        "path" => Some(Value::String(slice_string(record.request.path)?)),
        "query-string" => Some(Value::String(slice_string(record.request.query_string)?)),
        "remote-address" => Some(Value::String(slice_string(record.request.remote_address)?)),
        "headers" => Some(request_value(requests, receiver.handle, "headers")?),
        "body-handle" => request_body_value(record.clone())?,
        "identity" => record.identity,
        _ => None,
    })
}

fn extension_entries(
    requests: &Rc<RefCell<HashMap<RequestId, RequestRecord>>>,
    receiver: &core::ExtensionValue,
) -> Result<Vec<(Value, Value)>, String> {
    if receiver.type_name == "headers" {
        return Ok(request_headers(request_record(requests, receiver.handle)?)?
            .into_iter()
            .map(|(key, value)| (Value::String(key), Value::String(value)))
            .collect());
    }
    request_entries(requests, receiver.handle)
}

fn response_headers(value: Option<&Value>) -> Result<Vec<(String, String)>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    core::map_entries(value)
        .ok_or_else(|| "Hoplite response headers must be a map".to_string())?
        .into_iter()
        .map(|(key, value)| {
            let key = match key {
                Value::String(value) => value,
                Value::Keyword(value) => value.as_str().to_owned(),
                _ => return Err("Hoplite response header names must be text".into()),
            };
            let value = match value {
                Value::String(value) => value,
                _ => return Err("Hoplite response header values must be strings".into()),
            };
            Ok((key, value))
        })
        .collect()
}

fn response_headers_owned(value: Option<Value>) -> Result<Vec<(String, String)>, String> {
    let entries = match value {
        None => return Ok(Vec::new()),
        Some(Value::Map(entries)) => entries.into_iter().collect(),
        Some(value) => core::map_entries(&value)
            .ok_or_else(|| "Hoplite response headers must be a map".to_string())?,
    };
    entries
        .into_iter()
        .map(|(key, value)| {
            let key = match key {
                Value::String(value) => value,
                Value::Keyword(value) => value.as_str().to_owned(),
                _ => return Err("Hoplite response header names must be text".into()),
            };
            let Value::String(value) = value else {
                return Err("Hoplite response header values must be strings".into());
            };
            Ok((key, value))
        })
        .collect()
}

fn native_response(value: Value) -> Result<NativeResponse, String> {
    let entries = match value {
        Value::Map(entries) => entries.into_iter().collect(),
        value => core::map_entries(&value)
            .ok_or_else(|| "Hoplite handler must return a response map".to_string())?,
    };
    let mut status_value = None;
    let mut headers_value = None;
    let mut body_value = None;
    for (key, value) in entries {
        match key {
            Value::Keyword(keyword) if keyword.as_str() == "status" => status_value = Some(value),
            Value::Keyword(keyword) if keyword.as_str() == "headers" => headers_value = Some(value),
            Value::Keyword(keyword) if keyword.as_str() == "body" => body_value = Some(value),
            _ => {}
        }
    }
    let status = match status_value {
        None => 200,
        Some(Value::Number(value)) => u16::try_from(value)
            .ok()
            .filter(|value| (100..=599).contains(value))
            .ok_or_else(|| "Hoplite response status must be between 100 and 599".to_string())?,
        _ => return Err("Hoplite response status must be a number".into()),
    };
    let body = match body_value {
        None | Some(Value::Nil) => Vec::new(),
        Some(Value::String(value)) => value.into_bytes(),
        Some(Value::Bytes(value)) => value,
        _ => return Err("Hoplite response body must be a string or bytes".into()),
    };
    Ok(NativeResponse {
        status,
        headers: response_headers_owned(headers_value)?,
        body,
    })
}

fn response_value_owned(response: NativeResponse) -> Value {
    Value::Map(
        vec![
            (
                Value::Keyword("status".into()),
                Value::Number(response.status as i64),
            ),
            (
                Value::Keyword("headers".into()),
                Value::Map(
                    response
                        .headers
                        .into_iter()
                        .map(|(key, value)| (Value::String(key), Value::String(value)))
                        .collect(),
                ),
            ),
            (Value::Keyword("body".into()), Value::Bytes(response.body)),
        ]
        .into_iter()
        .collect(),
    )
}

fn install_request_protocols(
    protocols: &mut core::ProtocolRegistry,
    requests: Rc<RefCell<HashMap<RequestId, RequestRecord>>>,
) {
    for type_name in ["request", "headers", "exchange"] {
        protocols.register_extension_category("hoplite.route", type_name, "map");

        let store = requests.clone();
        protocols.register_extension(
            "hoplite.route",
            type_name,
            "std.protocol.ilookup/ILookup",
            "lookup",
            move |arguments| match arguments {
                [Value::Extension(receiver), key, default]
                    if receiver.provider == "hoplite.route" =>
                {
                    Ok(request_lookup(&store, receiver, key)?.unwrap_or_else(|| default.clone()))
                }
                _ => Err("hoplite/request-invalid: lookup receiver".into()),
            },
        );

        let store = requests.clone();
        protocols.register_extension(
            "hoplite.route",
            type_name,
            "std.protocol.icount/ICount",
            "count",
            move |arguments| match arguments {
                [Value::Extension(receiver)] if receiver.provider == "hoplite.route" => {
                    let count = extension_entries(&store, receiver)?.len();
                    Ok(Value::Number(count as i64))
                }
                _ => Err("hoplite/request-invalid: count receiver".into()),
            },
        );

        let store = requests.clone();
        protocols.register_extension(
            "hoplite.route",
            type_name,
            "std.protocol.iiter/IIter",
            "iter",
            move |arguments| match arguments {
                [Value::Extension(receiver)] if receiver.provider == "hoplite.route" => {
                    Ok(core::iterator_from_values(
                        extension_entries(&store, receiver)?
                            .into_iter()
                            .map(|(key, value)| Value::Vector(vec![key, value].into()))
                            .collect(),
                    ))
                }
                _ => Err("hoplite/request-invalid: iter receiver".into()),
            },
        );

        let store = requests.clone();
        protocols.register_extension(
            "hoplite.route",
            type_name,
            "std.protocol.ifind/IFind",
            "find",
            move |arguments| match arguments {
                [Value::Extension(receiver), key] if receiver.provider == "hoplite.route" => {
                    Ok(request_lookup(&store, receiver, key)?
                        .map(|value| Value::Vector(vec![key.clone(), value].into()))
                        .unwrap_or(Value::Nil))
                }
                _ => Err("hoplite/request-invalid: find receiver".into()),
            },
        );

        let store = requests.clone();
        protocols.register_extension(
            "hoplite.route",
            type_name,
            "std.protocol.iassoc/IAssoc",
            "assoc",
            move |arguments| match arguments {
                [Value::Extension(receiver), key, replacement]
                    if receiver.provider == "hoplite.route" =>
                {
                    let mut entries = extension_entries(&store, receiver)?;
                    entries.retain(|(candidate, _)| candidate != key);
                    entries.push((key.clone(), replacement.clone()));
                    Ok(Value::Map(entries.into_iter().collect()))
                }
                _ => Err("hoplite/request-invalid: assoc receiver".into()),
            },
        );

        let store = requests.clone();
        protocols.register_extension(
            "hoplite.route",
            type_name,
            "std.protocol.idissoc/IDissoc",
            "dissoc",
            move |arguments| match arguments {
                [Value::Extension(receiver), key] if receiver.provider == "hoplite.route" => {
                    let mut entries = extension_entries(&store, receiver)?;
                    entries.retain(|(candidate, _)| candidate != key);
                    Ok(Value::Map(entries.into_iter().collect()))
                }
                _ => Err("hoplite/request-invalid: dissoc receiver".into()),
            },
        );

        protocols.register_extension(
            "hoplite.route",
            type_name,
            "std.protocol.iempty/IEmpty",
            "empty",
            |_| Ok(Value::Map(Default::default())),
        );
    }
}

fn install_raw_namespace(
    namespaces: &kernel::NamespaceRegistry<Value>,
    requests: Rc<RefCell<HashMap<RequestId, RequestRecord>>>,
    builders: Rc<RefCell<HashMap<RequestId, RawBuilder>>>,
) {
    let native = namespaces.find_or_create("hoplite.raw.native");

    let respond_requests = requests.clone();
    native.intern(
        "respond",
        core::native_function("hoplite.raw.native/respond", 4, move |arguments| {
            let request = extension_request_id(&arguments[0])?;
            request_record(&respond_requests, request)?;
            let response = Value::Map(
                vec![
                    (Value::Keyword("status".into()), arguments[1].clone()),
                    (Value::Keyword("headers".into()), arguments[2].clone()),
                    (Value::Keyword("body".into()), arguments[3].clone()),
                ]
                .into_iter()
                .collect(),
            );
            Ok(response)
        }),
    );

    let start_requests = requests.clone();
    let start_builders = builders.clone();
    native.intern(
        "start",
        core::native_function("hoplite.raw.native/start", 3, move |arguments| {
            let request = extension_request_id(&arguments[0])?;
            request_record(&start_requests, request)?;
            let status = match arguments[1] {
                Value::Number(value) => u16::try_from(value)
                    .ok()
                    .filter(|value| (100..=599).contains(value))
                    .ok_or_else(|| "raw/start! status must be between 100 and 599".to_string())?,
                _ => return Err("raw/start! status must be a number".into()),
            };
            let headers = response_headers(Some(&arguments[2]))?;
            if start_builders
                .borrow_mut()
                .insert(
                    request,
                    RawBuilder {
                        status,
                        headers,
                        body: Vec::new(),
                    },
                )
                .is_some()
            {
                return Err("raw/response-started: response already started".into());
            }
            Ok(Value::Nil)
        }),
    );

    let write_requests = requests.clone();
    let write_builders = builders.clone();
    native.intern(
        "write",
        core::native_function("hoplite.raw.native/write", 2, move |arguments| {
            let request = extension_request_id(&arguments[0])?;
            request_record(&write_requests, request)?;
            let bytes = match &arguments[1] {
                Value::String(value) => value.as_bytes(),
                Value::Bytes(value) => value.as_slice(),
                _ => return Err("raw/write! expects a string or bytes".into()),
            };
            write_builders
                .borrow_mut()
                .get_mut(&request)
                .ok_or_else(|| "raw/response-not-started: call start! first".to_string())?
                .body
                .extend_from_slice(bytes);
            Ok(Value::Nil)
        }),
    );

    let finish_requests = requests;
    native.intern(
        "finish",
        core::native_function("hoplite.raw.native/finish", 1, move |arguments| {
            let request = extension_request_id(&arguments[0])?;
            request_record(&finish_requests, request)?;
            let response = builders
                .borrow_mut()
                .remove(&request)
                .ok_or_else(|| "raw/response-not-started: call start! first".to_string())?;
            Ok(response_value_owned(NativeResponse {
                status: response.status,
                headers: response.headers,
                body: response.body,
            }))
        }),
    );
}

impl HopliteRuntime {
    fn new() -> Self {
        let namespaces = hara_wasm::embedding_namespace_registry();
        let requests = Rc::new(RefCell::new(HashMap::new()));
        let resources = Rc::new(RefCell::new(ResourceRegistry::new()));
        let raw_builders = Rc::new(RefCell::new(HashMap::new()));
        let mut protocols = core::ProtocolRegistry::core();
        install_request_protocols(&mut protocols, requests.clone());
        install_raw_namespace(&namespaces, requests.clone(), raw_builders.clone());
        let host_pending = Rc::new(RefCell::new(Vec::new()));
        let pending = host_pending.clone();
        let host_next = Rc::new(RefCell::new(1_u64));
        let next = host_next.clone();
        let host = Rc::new(move |service: String, method: String, args: Vec<Value>| {
            if service == "hoplite.host" {
                return host_intrinsics::dispatch(service, method, args);
            }
            let call = *next.borrow();
            *next.borrow_mut() = call.saturating_add(1);
            let promise = Promise::new();
            pending
                .borrow_mut()
                .push((call, promise.clone(), service, method, args));
            Ok(Value::Promise(promise))
        });

        Self {
            namespaces,
            protocols,
            next_handler: 1,
            next_work: 1,
            next_call: 1,
            next_request: 1,
            next_response: 1,
            events: Rc::new(RefCell::new(VecDeque::new())),
            ready: Rc::new(RefCell::new(VecDeque::new())),
            call_owners: HashMap::new(),
            handlers: HashMap::new(),
            apps: HashMap::new(),
            works: HashMap::new(),
            requests,
            resources,
            raw_builders,
            responses: HashMap::new(),
            host,
            host_pending,
            host_next,
        }
    }

    fn allocate_work(&mut self) -> WorkId {
        let work = self.next_work;
        self.next_work = self.next_work.saturating_add(1);
        work
    }

    fn register_request_v3(
        &mut self,
        request: HopliteRequestV3,
    ) -> Result<(HopliteRequestV2, Option<ResourceHandle>), String> {
        if request.body.is_null() {
            if request.max_body_bytes != 0
                || request.max_chunk_bytes != 0
                || request.require_declared_length != 0
            {
                return Err(
                    "hoplite/request-body-invalid: body limits require a body descriptor".into(),
                );
            }
            return Ok((request.request, None));
        }

        let descriptor = unsafe { *request.body };
        let require_declared_length = match request.require_declared_length {
            0 => false,
            1 => true,
            value => {
                close_request_body_descriptor(descriptor);
                return Err(format!(
                    "hoplite/request-body-invalid: require_declared_length must be 0 or 1, received {value}"
                ));
            }
        };
        let limits = BodyLimits {
            max_body_bytes: request.max_body_bytes,
            max_chunk_bytes: request.max_chunk_bytes,
            require_declared_length,
        };
        // SAFETY: the V3 invocation transfers exclusive ownership of
        // this descriptor from its native caller into the worker.
        let registered = unsafe {
            self.resources
                .borrow_mut()
                .insert_request(descriptor, limits)
        };
        match registered {
            Ok(handle) if handle.get() <= i64::MAX as u64 => Ok((request.request, Some(handle))),
            Ok(handle) => {
                let _ = self.resources.borrow_mut().remove(handle);
                Err("hoplite/request-body-invalid: handle exceeds Hara integer range".into())
            }
            Err(error) => Err(format!("hoplite/request-body-invalid: {error}")),
        }
    }

    fn work_owns_request_body(&self, work: WorkId, handle: ResourceHandle) -> bool {
        let Some(request) = self.works.get(&work).and_then(|owner| owner.request) else {
            return false;
        };
        let requests = self.requests.borrow();
        matches!(
            requests.get(&request),
            Some(record) if record.body == Some(handle)
        )
    }

    fn allocate_request(
        &mut self,
        request: HopliteRequestV2,
        body: Option<ResourceHandle>,
    ) -> RequestId {
        let id = self.next_request;
        self.next_request = self.next_request.saturating_add(1);
        self.requests.borrow_mut().insert(
            id,
            RequestRecord {
                request,
                body,
                identity: None,
            },
        );
        id
    }

    fn close_request(&mut self, request: RequestId) {
        if let Some(record) = self.requests.borrow_mut().remove(&request) {
            if let Some(body) = record.body {
                let _ = self.resources.borrow_mut().remove(body);
            }
        }
        self.raw_builders.borrow_mut().remove(&request);
    }

    fn store_response(&mut self, response: NativeResponse) -> ResponseId {
        let id = self.next_response;
        self.next_response = self.next_response.saturating_add(1);
        self.responses.insert(id, response);
        id
    }

    fn insert_work(&mut self, work: WorkId, request: Option<RequestId>) {
        let result = Promise::new();
        let events = self.events.clone();
        result.on_settle(Rc::new(move |state| emit_settlement(&events, work, state)));
        let mut owner = Work::new(result);
        owner.request = request;
        self.works.insert(work, owner);
    }

    fn invoke_direct(
        &mut self,
        handler: HandlerId,
        binding: Value,
        request: RequestId,
    ) -> Result<InvokeState, String> {
        let call = self
            .handlers
            .get(&handler)
            .cloned()
            .ok_or_else(|| "unknown prepared handler".to_string())?;
        let work = self.allocate_work();
        let (host, pending, next) = self.host_handler(work);
        let namespaces = self.namespaces.clone();
        let protocols = self.protocols.clone();
        let value = core::with_namespace_registry(&namespaces, || {
            core::with_protocols(&protocols, || {
                core::with_host_calls(host, || call.invoke(vec![binding]))
            })
        })?;

        match value {
            Value::Promise(promise) => match promise.state() {
                PromiseState::Fulfilled(value) => {
                    self.next_call = *next.borrow();
                    let response = native_response(value)?;
                    self.close_request(request);
                    Ok(InvokeState::Complete(self.store_response(response)))
                }
                PromiseState::Rejected(error) => {
                    self.next_call = *next.borrow();
                    self.close_request(request);
                    Err(promise_rejection_message(error))
                }
                PromiseState::Pending => {
                    self.insert_work(work, Some(request));
                    if let Some(owner) = self.works.get_mut(&work) {
                        owner.result.adopt(&promise);
                        owner.children.push(promise);
                    }
                    self.collect_calls(work, pending, next);
                    Ok(InvokeState::Suspended(work))
                }
            },
            value => {
                if !pending.borrow().is_empty() {
                    self.next_call = *next.borrow();
                    self.close_request(request);
                    return Err("handler completed with unobserved host operations".into());
                }
                self.next_call = *next.borrow();
                let response = native_response(value)?;
                self.close_request(request);
                Ok(InvokeState::Complete(self.store_response(response)))
            }
        }
    }

    fn app_invoke_with_body(
        &mut self,
        app: AppId,
        request: HopliteRequestV2,
        body: Option<ResourceHandle>,
    ) -> Result<InvokeState, String> {
        let has_body = body.is_some();
        let request_id = self.allocate_request(request, body);
        let result = (|| {
            let method = slice_string(request.method)?.to_ascii_uppercase();
            let path = slice_string(request.path)?;
            let router = self
                .apps
                .get(&app)
                .ok_or_else(|| format!("unknown app {app}"))?;
            let route = router
                .routes
                .iter()
                .filter(|route| route.method == "ANY" || route.method == method)
                .filter_map(|route| route_score(&route.path, &path).map(|score| (score, route)))
                .max_by_key(|(score, _)| *score)
                .map(|(_, route)| (route.handler, route.adapter));
            let Some((handler, adapter)) = route else {
                self.close_request(request_id);
                return Ok(InvokeState::Complete(self.store_response(native_response(
                    response_value(404, "Not Found\n"),
                )?)));
            };

            match adapter {
                RouteAdapter::Raw => {
                    let binding = request_value(&self.requests, request_id, "exchange")?;
                    self.invoke_direct(handler, binding, request_id)
                }
                RouteAdapter::Request => {
                    let binding = request_value(&self.requests, request_id, "request")?;
                    self.invoke_direct(handler, binding, request_id)
                }
                RouteAdapter::RequestHta if has_body => Err(
                    "hoplite/request-body-adapter-invalid: body handles require request or raw adapter"
                        .into(),
                ),
                RouteAdapter::RequestHta => {
                    let value = Value::Map(
                        request_entries(&self.requests, request_id)?
                            .into_iter()
                            .collect(),
                    );
                    self.close_request(request_id);
                    let portable = hta::decode(&hta::encode(&value)?)?;
                    self.work_call(handler, portable)
                        .map(InvokeState::Suspended)
                        .map_err(|_| "cannot start HTA route".into())
                }
            }
        })();
        if result.is_err() {
            self.close_request(request_id);
        }
        result
    }

    fn app_invoke(&mut self, app: AppId, request: HopliteRequestV2) -> Result<InvokeState, String> {
        self.app_invoke_with_body(app, request, None)
    }

    fn handler_invoke_with_body(
        &mut self,
        handler: HandlerId,
        adapter: RouteAdapter,
        request: HopliteRequestV2,
        body: Option<ResourceHandle>,
    ) -> Result<InvokeState, String> {
        let has_body = body.is_some();
        let request_id = self.allocate_request(request, body);
        let result = match adapter {
            RouteAdapter::Raw => request_value(&self.requests, request_id, "exchange")
                .and_then(|binding| self.invoke_direct(handler, binding, request_id)),
            RouteAdapter::Request => request_value(&self.requests, request_id, "request")
                .and_then(|binding| self.invoke_direct(handler, binding, request_id)),
            RouteAdapter::RequestHta if has_body => Err(
                "hoplite/request-body-adapter-invalid: body handles require request or raw adapter"
                    .into(),
            ),
            RouteAdapter::RequestHta => {
                let value = Value::Map(
                    request_entries(&self.requests, request_id)?
                        .into_iter()
                        .collect(),
                );
                self.close_request(request_id);
                let portable = hta::decode(&hta::encode(&value)?)?;
                self.work_call(handler, portable)
                    .map(InvokeState::Suspended)
                    .map_err(|_| "cannot start HTA handler".into())
            }
        };
        if result.is_err() {
            self.close_request(request_id);
        }
        result
    }

    fn handler_invoke(
        &mut self,
        handler: HandlerId,
        adapter: RouteAdapter,
        request: HopliteRequestV2,
    ) -> Result<InvokeState, String> {
        self.handler_invoke_with_body(handler, adapter, request, None)
    }

    fn host_handler(
        &mut self,
        _work: WorkId,
    ) -> (
        Rc<dyn Fn(String, String, Vec<Value>) -> Result<Value, String>>,
        Rc<RefCell<Vec<HostCall>>>,
        Rc<RefCell<u64>>,
    ) {
        self.host_pending.borrow_mut().clear();
        *self.host_next.borrow_mut() = self.next_call;
        (
            self.host.clone(),
            self.host_pending.clone(),
            self.host_next.clone(),
        )
    }

    fn collect_calls(
        &mut self,
        work: WorkId,
        pending: Rc<RefCell<Vec<HostCall>>>,
        next: Rc<RefCell<u64>>,
    ) {
        self.next_call = *next.borrow();
        for (call, promise, service, method, args) in pending.borrow_mut().drain(..) {
            let value = Value::Vector(
                vec![
                    Value::Number(2),
                    Value::Number(call as i64),
                    Value::Number(work as i64),
                    Value::String("HOPLITE".into()),
                    Value::Nil,
                    Value::String(service),
                    Value::String(method),
                    Value::Vector(args.into()),
                ]
                .into(),
            );
            match hta::encode(&value) {
                Ok(bytes) => {
                    if let Some(owner) = self.works.get_mut(&work) {
                        owner.calls.insert(call, promise);
                        self.call_owners.insert(call, work);
                    }
                    self.events.borrow_mut().push_back(bytes);
                }
                Err(error) => {
                    promise.reject(format!("hta/value-unsupported: {error}"));
                }
            }
        }
    }

    fn handler_prepare(&mut self, function: &str) -> Result<HandlerId, String> {
        let call = vm::prepare_call(&self.namespaces, function, 1)?;
        let handler = self.next_handler;
        self.next_handler = self.next_handler.saturating_add(1);
        self.handlers.insert(handler, call);
        Ok(handler)
    }

    fn open_work(&mut self) -> WorkId {
        let work = self.allocate_work();
        let result = Promise::new();
        let events = self.events.clone();
        result.on_settle(Rc::new(move |state| emit_settlement(&events, work, state)));
        self.works.insert(work, Work::new(result));
        work
    }

    fn start_program(&mut self, program: Rc<vm::Program>) -> WorkId {
        let work = self.open_work();
        let (handler, pending, next) = self.host_handler(work);
        let namespaces = self.namespaces.clone();
        let protocols = self.protocols.clone();
        let fiber = core::with_namespace_registry(&namespaces, || {
            core::with_protocols(&protocols, || {
                core::with_host_calls(handler, || vm::VmFiber::start(program))
            })
        });
        self.collect_calls(work, pending, next);
        self.drive(work, fiber);
        work
    }

    fn work_start(&mut self, source: &str, binding: Option<Value>) -> WorkId {
        let program = prepare_vm_source(source, &self.namespaces).and_then(|source| {
            vm::compile_source_with(&source, &self.namespaces)
                .map(Rc::new)
                .map_err(|error| error.to_string())
        });
        match program {
            Ok(program) => {
                if let Some(binding) = binding {
                    self.namespaces
                        .current()
                        .intern("__hoplite_request", binding);
                }
                self.start_program(program)
            }
            Err(error) => {
                let work = self.allocate_work();
                let result = Promise::new();
                let events = self.events.clone();
                result.on_settle(Rc::new(move |state| emit_settlement(&events, work, state)));
                self.works.insert(work, Work::new(result));
                self.reject_work(work, error_value("eval/error", error));
                work
            }
        }
    }

    fn bootstrap_modules(&mut self, source: &str) -> Result<(), String> {
        let forms = kernel::parse_forms(source)?;
        let mut modules = Vec::<Vec<kernel::Form>>::new();
        for form in forms {
            if matches!(namespace_form(&form), FormNamespace::Namespace(_)) {
                modules.push(vec![form]);
            } else if let Some(module) = modules.last_mut() {
                module.push(form);
            } else {
                return Err("bootstrap source must begin with an ns form".into());
            }
        }
        for module in modules {
            let declaration = module.first().expect("bootstrap module has ns form");
            let namespace = match namespace_form(declaration) {
                FormNamespace::Namespace(namespace) => namespace.to_owned(),
                FormNamespace::Other => "unknown".to_owned(),
            };
            let mut environment = HashMap::new();
            core::with_namespace_registry(&self.namespaces, || {
                core::with_protocols(&self.protocols, || {
                    core::eval(declaration, &mut environment)
                })
            })
            .map_err(|error| format!("{namespace}: namespace declaration: {error}"))?;
            let body = module
                .iter()
                .skip(1)
                .filter(|form| !application_definition(form))
                .map(render_form)
                .collect::<Vec<_>>()
                .join("\n");
            if body.is_empty() {
                continue;
            }
            let program = vm::compile_source_with(&body, &self.namespaces)
                .map(Rc::new)
                .map_err(|error| format!("{namespace}: {error}"))?;
            core::with_namespace_registry(&self.namespaces, || {
                core::with_protocols(&self.protocols, || {
                    vm::execute_program_with_globals(program, &self.namespaces)
                        .map_err(|error| error.to_string())
                })
            })
            .map_err(|error| format!("{namespace}: execution: {error}"))?;
        }
        Ok(())
    }

    fn bootstrap_bytecode(&mut self, bundle: &[u8]) -> Result<(), String> {
        vm::eval_eager_bytecode_bundle_with_registries(&self.namespaces, &self.protocols, bundle)
    }

    fn work_call(&mut self, handler: HandlerId, binding: Value) -> Result<WorkId, ()> {
        let call = self.handlers.get(&handler).cloned().ok_or(())?;
        let work = self.open_work();
        let (host, pending, next) = self.host_handler(work);
        let namespaces = self.namespaces.clone();
        let protocols = self.protocols.clone();
        let fiber = core::with_namespace_registry(&namespaces, || {
            core::with_protocols(&protocols, || {
                core::with_host_calls(host, || call.start(vec![binding]))
            })
        });
        self.collect_calls(work, pending, next);
        match fiber {
            Ok(fiber) => self.drive(work, fiber),
            Err(error) => self.reject_work(work, error_value("eval/error", error)),
        }
        Ok(work)
    }

    fn apps_prepare(&mut self, manifest: Value) -> Result<(), String> {
        let format = map_optional_number(&manifest, "format").unwrap_or(1);
        if !matches!(format, 1 | 2) {
            return Err("unsupported Hoplite app manifest format".into());
        }
        let legacy = format == 1;
        let mut apps = HashMap::new();
        for app in map_sequence(&manifest, "apps")? {
            let id = map_number(&app, "id")? as u64;
            let mut routes = Vec::new();
            for route in map_sequence(&app, "routes")? {
                let method = map_string(&route, "method")?;
                let path = map_string(&route, "path")?;
                let function = map_string(&route, "handler")?;
                let adapter = RouteAdapter::parse(map_optional_string(&route, "adapter"), legacy)?;
                if map_value(&route, "auth").is_some() {
                    return Err("route auth policy belongs in the HAL handler".into());
                }
                let handler = self.handler_prepare(&function)?;
                routes.push(AppRoute {
                    method,
                    path,
                    handler,
                    adapter,
                });
            }
            if apps.insert(id, AppRouter { routes }).is_some() {
                return Err(format!("duplicate app id {id}"));
            }
        }
        self.apps = apps;
        Ok(())
    }

    fn app_call(&mut self, app: AppId, request: Value) -> Result<WorkId, ()> {
        let method = map_optional_string(&request, "request-method")
            .or_else(|| map_optional_string(&request, "method"))
            .ok_or(())?
            .to_ascii_uppercase();
        let path = map_optional_string(&request, "path")
            .or_else(|| map_optional_string(&request, "uri"))
            .ok_or(())?;
        let router = self.apps.get(&app).ok_or(())?;
        let handler = router
            .routes
            .iter()
            .filter(|route| route.method == "ANY" || route.method == method)
            .filter_map(|route| route_score(&route.path, &path).map(|score| (score, route)))
            .max_by_key(|(score, _)| *score)
            .map(|(_, route)| route.handler);
        match handler {
            Some(handler) => self.work_call(handler, request),
            None => Ok(self.start_value(response_value(404, "Not Found\n"))),
        }
    }

    fn start_value(&mut self, value: Value) -> WorkId {
        let work = self.allocate_work();
        let result = Promise::new();
        let events = self.events.clone();
        result.on_settle(Rc::new(move |state| emit_settlement(&events, work, state)));
        self.works.insert(work, Work::new(result.clone()));
        result.resolve(value);
        work
    }

    fn resume(&mut self, work: WorkId, state: PromiseState) {
        let Some(mut fiber) = self.works.get_mut(&work).and_then(|work| work.fiber.take()) else {
            return;
        };
        let (handler, pending, next) = self.host_handler(work);
        let namespaces = self.namespaces.clone();
        let protocols = self.protocols.clone();
        core::with_namespace_registry(&namespaces, || {
            core::with_protocols(&protocols, || {
                core::with_host_calls(handler, || fiber.resume(state));
            });
        });

        self.collect_calls(work, pending, next);
        self.drive(work, fiber);
    }

    fn drive(&mut self, work: WorkId, fiber: vm::VmFiber) {
        match fiber.state() {
            vm::VmFiberState::Suspended => {
                let promise = fiber.pending().expect("suspended fiber promise");
                let ready = self.ready.clone();
                promise.on_settle(Rc::new(move |state| {
                    ready.borrow_mut().push_back((work, state));
                }));
                if let Some(owner) = self.works.get_mut(&work) {
                    owner.fiber = Some(fiber);
                }
            }
            vm::VmFiberState::Completed(Value::Promise(promise)) => {
                if let Some(owner) = self.works.get_mut(&work) {
                    owner.result.adopt(&promise);
                    owner.children.push(promise);
                }
            }
            vm::VmFiberState::Completed(value) => {
                if let Some(owner) = self.works.get(&work) {
                    owner.result.resolve(value);
                }
            }
            vm::VmFiberState::Failed(error) => {
                self.reject_work(work, error_value("eval/error", error.to_string()));
            }
            vm::VmFiberState::Cancelled => {
                self.reject_work(work, error_value("work/cancelled", "cancelled".into()));
            }
            vm::VmFiberState::Yielded(_) => {
                self.reject_work(
                    work,
                    error_value(
                        "fiber/invalid-state",
                        "request handler yielded outside of a coroutine driver".into(),
                    ),
                );
            }
            vm::VmFiberState::Running => {
                self.reject_work(
                    work,
                    error_value("fiber/invalid-state", "running fiber escaped".into()),
                );
            }
        }
    }

    fn drain_ready(&mut self) {
        self.poll_child_results();
        loop {
            let next = self.ready.borrow_mut().pop_front();
            match next {
                Some((work, state)) => self.resume(work, state),
                None => break,
            }
        }
    }

    fn poll_child_results(&mut self) {
        let work_ids = self.works.keys().copied().collect::<Vec<_>>();
        for work in work_ids {
            for _ in 0..MAX_CHILD_DRIVE_PASSES {
                let Some(owner) = self.works.get(&work) else {
                    break;
                };
                if !owner.calls.is_empty() || owner.children.is_empty() {
                    break;
                }
                let children = owner.children.clone();
                let (handler, pending, next) = self.host_handler(work);
                let namespaces = self.namespaces.clone();
                let protocols = self.protocols.clone();
                let states = core::with_namespace_registry(&namespaces, || {
                    core::with_protocols(&protocols, || {
                        core::with_host_calls(handler, || {
                            children.iter().map(Promise::state).collect::<Vec<_>>()
                        })
                    })
                });
                self.collect_calls(work, pending, next);
                if let Some(owner) = self.works.get_mut(&work) {
                    owner.children = children
                        .into_iter()
                        .zip(states)
                        .filter_map(|(child, state)| {
                            matches!(state, PromiseState::Pending).then_some(child)
                        })
                        .collect();
                }
            }
        }
    }

    fn call_deliver(&mut self, call: CallId, success: bool, payload: Value) -> Result<(), ()> {
        let Some(work) = self.call_owners.remove(&call) else {
            return Err(());
        };
        let Some(promise) = self
            .works
            .get_mut(&work)
            .and_then(|owner| owner.calls.remove(&call))
        else {
            return Err(());
        };
        let (handler, pending, next) = self.host_handler(work);
        let namespaces = self.namespaces.clone();
        let protocols = self.protocols.clone();
        core::with_namespace_registry(&namespaces, || {
            core::with_protocols(&protocols, || {
                core::with_host_calls(handler, || {
                    if success {
                        promise.resolve(payload);
                    } else {
                        promise.reject_value(payload);
                    }
                })
            })
        });
        self.collect_calls(work, pending, next);
        self.drain_ready();
        Ok(())
    }

    fn work_cancel(&mut self, work: WorkId) -> bool {
        let Some(owner) = self.works.get_mut(&work) else {
            return false;
        };
        for (call, promise) in owner.calls.drain() {
            self.call_owners.remove(&call);
            promise.cancel();
        }
        for child in owner.children.drain(..) {
            child.cancel();
        }
        if let Some(mut fiber) = owner.fiber.take() {
            fiber.cancel();
        }
        owner.result.cancel()
    }

    fn work_close(&mut self, work: WorkId) -> bool {
        if !self.works.contains_key(&work) {
            return false;
        }
        self.work_cancel(work);
        if let Some(mut owner) = self.works.remove(&work) {
            for (call, promise) in owner.calls.drain() {
                self.call_owners.remove(&call);
                promise.cancel();
            }
            if let Some(request) = owner.request.take() {
                self.close_request(request);
            }
        }
        true
    }

    fn reject_work(&self, work: WorkId, error: Value) {
        if let Some(owner) = self.works.get(&work) {
            owner.result.reject_value(error);
        }
    }
}

fn map_value(value: &Value, name: &str) -> Option<Value> {
    core::map_entries(value)?
        .into_iter()
        .find_map(|(key, value)| {
            matches!(&key, Value::Keyword(keyword) if keyword.as_str() == name).then_some(value)
        })
}

fn value_sequence(value: Value) -> Result<Vec<Value>, String> {
    match value {
        Value::Vector(values) => Ok(values.iter().cloned().collect()),
        Value::List(values) => Ok(values.iter().cloned().collect()),
        Value::Tuple(values) => Ok(values.iter().cloned().collect()),
        _ => Err("expected sequence".into()),
    }
}

fn map_sequence(value: &Value, name: &str) -> Result<Vec<Value>, String> {
    value_sequence(map_value(value, name).ok_or_else(|| format!("missing :{name}"))?)
}

fn map_optional_string(value: &Value, name: &str) -> Option<String> {
    match map_value(value, name)? {
        Value::String(value) => Some(value),
        Value::Keyword(value) => Some(value.as_str().to_owned()),
        _ => None,
    }
}

fn map_string(value: &Value, name: &str) -> Result<String, String> {
    map_optional_string(value, name).ok_or_else(|| format!("missing or invalid :{name}"))
}

fn map_number(value: &Value, name: &str) -> Result<i64, String> {
    match map_value(value, name) {
        Some(Value::Number(value)) if value > 0 => Ok(value),
        _ => Err(format!("missing or invalid :{name}")),
    }
}

fn map_optional_number(value: &Value, name: &str) -> Option<i64> {
    match map_value(value, name)? {
        Value::Number(value) => Some(value),
        _ => None,
    }
}

fn route_score(pattern: &str, path: &str) -> Option<(usize, usize)> {
    let pattern = pattern
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty());
    let mut path = path
        .split('?')
        .next()
        .unwrap_or(path)
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty());
    let mut literal = 0;
    let mut segments = 0;
    for expected in pattern {
        if expected.starts_with('*') {
            return Some((literal, segments));
        }
        let actual = path.next()?;
        if !expected.starts_with(':') {
            if expected != actual {
                return None;
            }
            literal += 1;
        }
        segments += 1;
    }
    path.next().is_none().then_some((literal, segments))
}

fn response_value(status: i64, body: &str) -> Value {
    Value::Map(
        vec![
            (Value::Keyword("status".into()), Value::Number(status)),
            (
                Value::Keyword("headers".into()),
                Value::Map(
                    vec![(
                        Value::String("content-type".into()),
                        Value::String("text/plain".into()),
                    )]
                    .into_iter()
                    .collect(),
                ),
            ),
            (Value::Keyword("body".into()), Value::String(body.into())),
        ]
        .into_iter()
        .collect(),
    )
}

fn prepare_vm_source(
    source: &str,
    namespaces: &kernel::NamespaceRegistry<Value>,
) -> Result<String, String> {
    let forms = kernel::parse_forms(source)?;
    let mut output = Vec::new();
    for form in forms {
        if let FormNamespace::Namespace(name) = namespace_form(&form) {
            namespaces.set_current(name);
        } else if application_definition(&form) {
            continue;
        } else {
            output.push(render_form(&form));
        }
    }
    Ok(output.join("\n"))
}

fn application_definition(form: &kernel::Form) -> bool {
    let kernel::Form::List(definition) = form else {
        return false;
    };
    if !matches!(definition.first(), Some(kernel::Form::Symbol(operator)) if operator == "def") {
        return false;
    }
    let Some(kernel::Form::List(expression)) = definition.get(2) else {
        return false;
    };
    matches!(expression.first(), Some(kernel::Form::Symbol(operator))
        if matches!(operator.as_str(), "h/app" | "hoplite.core/app" | "internal/config" | "hoplite.internal/config"))
}

enum FormNamespace<'a> {
    Namespace(&'a str),
    Other,
}

fn namespace_form(form: &kernel::Form) -> FormNamespace<'_> {
    match form {
        kernel::Form::List(values) if matches!(values.first(), Some(kernel::Form::Symbol(operator)) if operator == "ns") => {
            match values.get(1) {
                Some(kernel::Form::Symbol(name)) => FormNamespace::Namespace(name),
                _ => FormNamespace::Other,
            }
        }
        _ => FormNamespace::Other,
    }
}

fn render_form(form: &kernel::Form) -> String {
    use kernel::Form;
    match form {
        Form::Metadata(metadata, value) => {
            format!("^{} {}", render_form(metadata), render_form(value))
        }
        Form::Tagged(tag, value) => format!("#{tag}{}", render_form(value)),
        Form::List(values) => render_sequence(values, "(", ")"),
        Form::Vector(values) => render_sequence(values, "[", "]"),
        Form::Set(values) => render_sequence(values, "#{", "}"),
        Form::Map(entries) => {
            let values = entries
                .iter()
                .flat_map(|(key, value)| [render_form(key), render_form(value)])
                .collect::<Vec<_>>();
            format!("{{{}}}", values.join(" "))
        }
        _ => form.to_string(),
    }
}

fn render_sequence(values: &[kernel::Form], prefix: &str, suffix: &str) -> String {
    format!(
        "{prefix}{}{suffix}",
        values.iter().map(render_form).collect::<Vec<_>>().join(" ")
    )
}

fn event(kind: i64, id: u64, value: Value) -> Value {
    Value::Vector(vec![Value::Number(kind), Value::Number(id as i64), value].into())
}

fn error_value(code: &str, message: String) -> Value {
    Value::Map(
        vec![
            (Value::Keyword("code".into()), Value::Keyword(code.into())),
            (Value::Keyword("message".into()), Value::String(message)),
            (
                Value::Keyword("origin".into()),
                Value::Keyword("hoplite".into()),
            ),
            (Value::Keyword("retryable".into()), Value::Bool(false)),
        ]
        .into_iter()
        .collect(),
    )
}

fn emit_settlement(events: &Rc<RefCell<VecDeque<Vec<u8>>>>, work: u64, state: PromiseState) {
    let value = match state {
        PromiseState::Pending => return,
        PromiseState::Fulfilled(value) => event(0, work, value),
        PromiseState::Rejected(PromiseRejection::Value(value)) => event(1, work, value),
        PromiseState::Rejected(PromiseRejection::Message(message)) => {
            event(1, work, error_value("promise/rejected", message))
        }
        PromiseState::Rejected(PromiseRejection::Cancelled(value)) => {
            event(1, work, error_value("work/cancelled", value.display()))
        }
    };
    enqueue_event(events, value);
}

fn promise_rejection_message(error: PromiseRejection) -> String {
    match error {
        PromiseRejection::Value(value) => value.display(),
        PromiseRejection::Message(message) => message,
        PromiseRejection::Cancelled(value) => value.display(),
    }
}

fn enqueue_event(events: &Rc<RefCell<VecDeque<Vec<u8>>>>, value: Value) {
    let encoded = hta::encode(&value)
        .or_else(|error| hta::encode(&event(1, 0, error_value("hta/value-unsupported", error))));
    if let Ok(bytes) = encoded {
        events.borrow_mut().push_back(bytes);
    }
}

fn bytes<'a>(pointer: *const u8, len: usize) -> Result<&'a [u8], ()> {
    if pointer.is_null() {
        if len == 0 {
            return Ok(&[]);
        }
        return Err(());
    }
    Ok(unsafe { std::slice::from_raw_parts(pointer, len) })
}

fn source<'a>(pointer: *const u8, len: usize) -> Result<&'a str, ()> {
    std::str::from_utf8(bytes(pointer, len)?).map_err(|_| ())
}

unsafe fn runtime_mut<'a>(runtime: *mut HopliteRuntime) -> Result<&'a mut HopliteRuntime, ()> {
    runtime.as_mut().ok_or(())
}

#[no_mangle]
pub extern "C" fn hoplite_abi_version() -> u32 {
    ABI_VERSION
}

#[no_mangle]
pub extern "C" fn hoplite_runtime_new() -> *mut HopliteRuntime {
    match catch_unwind(AssertUnwindSafe(HopliteRuntime::new)) {
        Ok(runtime) => Box::into_raw(Box::new(runtime)),
        Err(_) => ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_runtime_free(runtime: *mut HopliteRuntime) {
    if !runtime.is_null() {
        drop(Box::from_raw(runtime));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_bootstrap_modules(
    runtime: *mut HopliteRuntime,
    source_ptr: *const u8,
    source_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let source = source(source_ptr, source_len)?;
        if let Err(error) = runtime.bootstrap_modules(source) {
            eprintln!("hoplite bootstrap: {error}");
            return Err(());
        }
        Ok::<i32, ()>(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_bootstrap_bytecode(
    runtime: *mut HopliteRuntime,
    bundle_ptr: *const u8,
    bundle_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let bundle = bytes(bundle_ptr, bundle_len)?;
        if let Err(error) = runtime.bootstrap_bytecode(bundle) {
            eprintln!("hoplite bytecode bootstrap: {error}");
            return Err(());
        }
        Ok::<i32, ()>(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_work_start(
    runtime: *mut HopliteRuntime,
    source_ptr: *const u8,
    source_len: usize,
    binding_ptr: *const u8,
    binding_len: usize,
) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let source = source(source_ptr, source_len)?;
        let binding = if binding_len == 0 {
            None
        } else {
            Some(hta::decode(bytes(binding_ptr, binding_len)?).map_err(|_| ())?)
        };
        Ok::<u64, ()>(runtime.work_start(source, binding))
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_handler_close(runtime: *mut HopliteRuntime, handler: u64) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        Ok::<i32, ()>(if runtime.handlers.remove(&handler).is_some() {
            0
        } else {
            1
        })
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_handler_prepare(
    runtime: *mut HopliteRuntime,
    function_ptr: *const u8,
    function_len: usize,
) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let function = source(function_ptr, function_len)?;
        runtime.handler_prepare(function).map_err(|_| ())
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_work_call(
    runtime: *mut HopliteRuntime,
    handler: u64,
    input_ptr: *const u8,
    input_len: usize,
) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let input = hta::decode(bytes(input_ptr, input_len)?).map_err(|_| ())?;
        runtime.work_call(handler, input)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_apps_prepare(
    runtime: *mut HopliteRuntime,
    manifest_ptr: *const u8,
    manifest_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let manifest = hta::decode(bytes(manifest_ptr, manifest_len)?).map_err(|_| ())?;
        runtime.apps_prepare(manifest).map_err(|_| ())?;
        Ok::<i32, ()>(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_app_call(
    runtime: *mut HopliteRuntime,
    app: u64,
    input_ptr: *const u8,
    input_len: usize,
) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let input = hta::decode(bytes(input_ptr, input_len)?).map_err(|_| ())?;
        runtime.app_call(app, input)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_app_invoke_v2(
    runtime: *mut HopliteRuntime,
    app: u64,
    request: *const HopliteRequestV2,
    outcome: *mut HopliteOutcomeV2,
) -> i32 {
    if request.is_null() || outcome.is_null() {
        return 1;
    }
    (*outcome).kind = 0;
    (*outcome).id = 0;
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        match runtime.app_invoke(app, *request).map_err(|error| {
            eprintln!("hoplite app invocation failed: {error}");
        })? {
            InvokeState::Complete(response) => {
                (*outcome).kind = 1;
                (*outcome).id = response;
            }
            InvokeState::Suspended(work) => {
                (*outcome).kind = 2;
                (*outcome).id = work;
            }
        }
        Ok::<i32, ()>(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_handler_invoke_v2(
    runtime: *mut HopliteRuntime,
    handler: u64,
    adapter: u32,
    request: *const HopliteRequestV2,
    outcome: *mut HopliteOutcomeV2,
) -> i32 {
    if request.is_null() || outcome.is_null() {
        return 1;
    }
    (*outcome).kind = 0;
    (*outcome).id = 0;
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let adapter = RouteAdapter::from_abi(adapter).map_err(|_| ())?;
        match runtime
            .handler_invoke(handler, adapter, *request)
            .map_err(|_| ())?
        {
            InvokeState::Complete(response) => {
                (*outcome).kind = 1;
                (*outcome).id = response;
            }
            InvokeState::Suspended(work) => {
                (*outcome).kind = 2;
                (*outcome).id = work;
            }
        }
        Ok::<i32, ()>(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_app_invoke_v3(
    runtime: *mut HopliteRuntime,
    app: u64,
    request: *const HopliteRequestV3,
    outcome: *mut HopliteOutcomeV2,
) -> i32 {
    if request.is_null() || outcome.is_null() {
        return 1;
    }
    (*outcome).kind = 0;
    (*outcome).id = 0;
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let (request, body) = runtime.register_request_v3(*request).map_err(|_| ())?;
        match runtime
            .app_invoke_with_body(app, request, body)
            .map_err(|_| ())?
        {
            InvokeState::Complete(response) => {
                (*outcome).kind = 1;
                (*outcome).id = response;
            }
            InvokeState::Suspended(work) => {
                (*outcome).kind = 2;
                (*outcome).id = work;
            }
        }
        Ok::<i32, ()>(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_handler_invoke_v3(
    runtime: *mut HopliteRuntime,
    handler: u64,
    adapter: u32,
    request: *const HopliteRequestV3,
    outcome: *mut HopliteOutcomeV2,
) -> i32 {
    if request.is_null() || outcome.is_null() {
        return 1;
    }
    (*outcome).kind = 0;
    (*outcome).id = 0;
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let (request, body) = runtime.register_request_v3(*request).map_err(|_| ())?;
        let adapter = match RouteAdapter::from_abi(adapter) {
            Ok(adapter) => adapter,
            Err(_) => {
                if let Some(body) = body {
                    let _ = runtime.resources.borrow_mut().remove(body);
                }
                return Err(());
            }
        };
        match runtime
            .handler_invoke_with_body(handler, adapter, request, body)
            .map_err(|_| ())?
        {
            InvokeState::Complete(response) => {
                (*outcome).kind = 1;
                (*outcome).id = response;
            }
            InvokeState::Suspended(work) => {
                (*outcome).kind = 2;
                (*outcome).id = work;
            }
        }
        Ok::<i32, ()>(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_request_body_read_v3(
    runtime: *mut HopliteRuntime,
    work: u64,
    handle: u64,
    output: *mut u8,
    capacity: usize,
    returned: *mut usize,
) -> i32 {
    if returned.is_null() || (output.is_null() && capacity != 0) {
        return 1;
    }
    *returned = 0;
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let handle = ResourceHandle::new(handle).map_err(|_| ())?;
        if !runtime.work_owns_request_body(work, handle) {
            return Err(());
        }
        let output = if capacity == 0 {
            &mut []
        } else {
            slice::from_raw_parts_mut(output, capacity)
        };
        let read = {
            let mut resources = runtime.resources.borrow_mut();
            resources.read_request(handle, output).map_err(|_| ())?
        };
        *returned = read;
        Ok::<i32, ()>(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_request_body_finish_v3(
    runtime: *mut HopliteRuntime,
    work: u64,
    handle: u64,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let handle = ResourceHandle::new(handle).map_err(|_| ())?;
        if !runtime.work_owns_request_body(work, handle) {
            return Err(());
        }
        {
            let resources = runtime.resources.borrow();
            resources.finish_request(handle).map_err(|_| ())?;
        }
        Ok::<i32, ()>(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_response_status_v2(
    runtime: *mut HopliteRuntime,
    response: u64,
    status: *mut u16,
) -> i32 {
    if status.is_null() {
        return 1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        *status = runtime.responses.get(&response).ok_or(())?.status;
        Ok::<i32, ()>(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_response_body_v2(
    runtime: *mut HopliteRuntime,
    response: u64,
    body: *mut HopliteSlice,
) -> i32 {
    if body.is_null() {
        return 1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let value = &runtime.responses.get(&response).ok_or(())?.body;
        (*body).data = value.as_ptr();
        (*body).len = value.len();
        Ok::<i32, ()>(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_response_header_count_v2(
    runtime: *mut HopliteRuntime,
    response: u64,
) -> usize {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        Ok::<usize, ()>(runtime.responses.get(&response).ok_or(())?.headers.len())
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_response_header_at_v2(
    runtime: *mut HopliteRuntime,
    response: u64,
    index: usize,
    name: *mut HopliteSlice,
    value: *mut HopliteSlice,
) -> i32 {
    if name.is_null() || value.is_null() {
        return 1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let (header_name, header_value) = runtime
            .responses
            .get(&response)
            .and_then(|response| response.headers.get(index))
            .ok_or(())?;
        (*name).data = header_name.as_ptr();
        (*name).len = header_name.len();
        (*value).data = header_value.as_ptr();
        (*value).len = header_value.len();
        Ok::<i32, ()>(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_response_close_v2(
    runtime: *mut HopliteRuntime,
    response: u64,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        Ok::<i32, ()>(if runtime.responses.remove(&response).is_some() {
            0
        } else {
            1
        })
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_work_poll(runtime: *mut HopliteRuntime) -> usize {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        runtime.drain_ready();
        Ok::<usize, ()>(runtime.events.borrow().len())
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_work_next_event(
    runtime: *mut HopliteRuntime,
    output: *mut HopliteBuffer,
) -> i32 {
    if output.is_null() {
        return 1;
    }
    (*output).data = ptr::null_mut();
    (*output).len = 0;

    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        runtime.drain_ready();
        let Some(bytes) = runtime.events.borrow_mut().pop_front() else {
            return Ok::<i32, ()>(1);
        };
        let boxed = bytes.into_boxed_slice();
        let len = boxed.len();
        let data = Box::into_raw(boxed) as *mut u8;
        (*output).data = data;
        (*output).len = len;
        Ok(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(2)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_buffer_free(data: *mut u8, len: usize) {
    if data.is_null() {
        return;
    }
    let slice = ptr::slice_from_raw_parts_mut(data, len);
    drop(Box::from_raw(slice));
}

unsafe fn hoplite_call_deliver(
    runtime: *mut HopliteRuntime,
    call: u64,
    success: bool,
    payload_ptr: *const u8,
    payload_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let payload = if payload_len == 0 {
            Value::Nil
        } else {
            hta::decode(bytes(payload_ptr, payload_len)?).map_err(|_| ())?
        };
        runtime
            .call_deliver(call, success, payload)
            .map_err(|_| ())?;
        Ok::<i32, ()>(0)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_call_resolve(
    runtime: *mut HopliteRuntime,
    call: u64,
    payload_ptr: *const u8,
    payload_len: usize,
) -> i32 {
    hoplite_call_deliver(runtime, call, true, payload_ptr, payload_len)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_call_reject(
    runtime: *mut HopliteRuntime,
    call: u64,
    payload_ptr: *const u8,
    payload_len: usize,
) -> i32 {
    hoplite_call_deliver(runtime, call, false, payload_ptr, payload_len)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_work_send(
    runtime: *mut HopliteRuntime,
    work: u64,
    message_ptr: *const u8,
    message_len: usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let _message = hta::decode(bytes(message_ptr, message_len)?).map_err(|_| ())?;
        if runtime.works.contains_key(&work) {
            Ok::<i32, ()>(0)
        } else {
            Err(())
        }
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_work_cancel(runtime: *mut HopliteRuntime, work: u64) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        Ok::<i32, ()>(if runtime.work_cancel(work) { 0 } else { 1 })
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn hoplite_work_close(runtime: *mut HopliteRuntime, work: u64) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        Ok::<i32, ()>(if runtime.work_close(work) { 0 } else { 1 })
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hara_wasm::lang::IPeekFirst;
    use hara_wasm::vm::BytecodeBundleModule;

    fn bytecode_module(
        compiler: &mut hara_wasm::Runtime,
        namespace: &str,
        body: &str,
    ) -> BytecodeBundleModule {
        let declaration = format!("(ns {namespace} (:require [std.foundation :refer :all]))");
        compiler.eval_native(&declaration).unwrap();
        BytecodeBundleModule {
            resource: namespace.into(),
            namespace_form: declaration,
            source_digest: [0; 32],
            dependencies: vec!["std.foundation".into()],
            eager: true,
            artifact: compiler.compile_bytecode_artifact(body).unwrap(),
        }
    }

    #[test]
    fn bytecode_bootstrap_is_hbx_alpha_and_transactional() {
        let mut compiler = hara_wasm::Runtime::new();
        let successful = bytecode_module(&mut compiler, "example.bytecode", "(defn answer [] 42)");
        let bundle = vm::encode_bytecode_bundle(&[successful]).unwrap();
        assert_eq!(&bundle[..4], b"HBX0");
        let mut runtime = HopliteRuntime::new();
        runtime.bootstrap_bytecode(&bundle).unwrap();
        assert!(runtime
            .namespaces
            .find("example.bytecode")
            .unwrap()
            .mappings()
            .iter()
            .any(|(name, _)| name.as_str() == "answer"));

        let good = bytecode_module(&mut compiler, "example.rollback", "(def marker 1)");
        let bad = bytecode_module(&mut compiler, "example.failure", "(throw \"boom\")");
        let failing = vm::encode_bytecode_bundle(&[good, bad]).unwrap();
        assert!(runtime.bootstrap_bytecode(&failing).is_err());
        assert!(runtime.namespaces.find("example.rollback").is_none());
        assert!(runtime.namespaces.find("example.failure").is_none());
    }

    #[test]
    fn bootstrap_compiles_namespaces_independently_in_dependency_order() {
        let mut runtime = HopliteRuntime::new();
        runtime
            .bootstrap_modules(
                "(ns example.dependency (:require [std.foundation :refer :all])) \
                 (defn answer [value] (if (and (map? value) (type value)) 42 0)) \
                 (ns example.application (:require [example.dependency :as dependency])) \
                 (defn answer [value] (dependency/answer value))",
            )
            .unwrap();
        assert!(runtime
            .namespaces
            .find("example.dependency")
            .unwrap()
            .mappings()
            .iter()
            .any(|(name, _)| name.as_str() == "answer"));
        assert!(runtime
            .namespaces
            .find("example.application")
            .unwrap()
            .mappings()
            .iter()
            .any(|(name, _)| name.as_str() == "answer"));
    }

    #[test]
    fn independently_compiled_async_namespaces_drive_inner_host_calls() {
        let mut runtime = HopliteRuntime::new();
        runtime
            .bootstrap_modules(
                "(ns example.provider (:require [std.foundation.coroutine :as coroutine])) \
                 (defn ^:async load [] (coroutine/await (std.native.Host/call \"hoplite.store\" \"load\" [\"state\"])) (coroutine/await (std.native.Host/call \"hoplite.store\" \"initialize\" [\"state\"]))) \
                 (ns example.service (:require [std.foundation.coroutine :as coroutine] [example.provider :as provider])) \
                 (defn ^:async load [] (coroutine/await (provider/load))) \
                 (ns example.handler (:require [std.foundation.coroutine :as coroutine] [example.service :as service])) \
                 (defn ^:async show [request] (coroutine/await (service/load)) {:status 200 :body \"ready\"})",
            )
            .unwrap();
        let handler = runtime.handler_prepare("example.handler/show").unwrap();
        runtime
            .work_call(handler, Value::Map(Default::default()))
            .unwrap();
        let Value::Vector(call) = take_event(&mut runtime) else {
            panic!("cross-namespace host event")
        };
        assert!(matches!(call.get(0), Some(Value::Number(2))));
        assert!(matches!(call.get(5), Some(Value::String(service)) if service == "hoplite.store"));
        let first_call = match call.get(1) {
            Some(Value::Number(value)) => *value as u64,
            _ => panic!("first call id"),
        };
        runtime
            .call_deliver(first_call, true, Value::Nil)
            .expect("first cross-namespace call is delivered");
        let Value::Vector(call) = take_event(&mut runtime) else {
            panic!("second cross-namespace host event")
        };
        assert!(matches!(call.get(6), Some(Value::String(method)) if method == "initialize"));
    }

    struct TestRequest {
        headers: Vec<(&'static str, &'static str)>,
    }

    unsafe extern "C" fn test_header_at(
        context: *mut c_void,
        index: usize,
        name: *mut HopliteSlice,
        value: *mut HopliteSlice,
    ) -> i32 {
        let request = &*(context as *const TestRequest);
        let Some((header_name, header_value)) = request.headers.get(index) else {
            return 1;
        };
        *name = test_slice(header_name);
        *value = test_slice(header_value);
        0
    }

    fn test_slice(value: &'static str) -> HopliteSlice {
        HopliteSlice {
            data: value.as_ptr(),
            len: value.len(),
        }
    }

    fn test_request(context: &mut TestRequest, path: &'static str) -> HopliteRequestV2 {
        HopliteRequestV2 {
            context: context as *mut TestRequest as *mut c_void,
            method: test_slice("GET"),
            uri: test_slice(path),
            path: test_slice(path),
            query_string: test_slice(""),
            remote_address: test_slice("127.0.0.1"),
            header_count: context.headers.len(),
            header_at: Some(test_header_at),
        }
    }

    struct TestBody {
        bytes: Vec<u8>,
        cursor: usize,
        close_count: usize,
    }

    unsafe extern "C" fn test_body_read(
        context: *mut c_void,
        output: *mut u8,
        capacity: usize,
        returned: *mut usize,
    ) -> i32 {
        let body = &mut *(context as *mut TestBody);
        let remaining = body.bytes.len().saturating_sub(body.cursor);
        let count = capacity.min(remaining);
        if count != 0 {
            slice::from_raw_parts_mut(output, capacity)[..count]
                .copy_from_slice(&body.bytes[body.cursor..body.cursor + count]);
            body.cursor += count;
        }
        *returned = count;
        0
    }

    unsafe extern "C" fn test_body_close(context: *mut c_void) {
        let body = &mut *(context as *mut TestBody);
        body.close_count += 1;
    }

    fn test_body_descriptor(body: &mut TestBody) -> HopliteRequestBodyV1 {
        HopliteRequestBodyV1 {
            context: body as *mut TestBody as *mut c_void,
            declared_length: body.bytes.len() as u64,
            has_declared_length: 1,
            read: Some(test_body_read),
            close: Some(test_body_close),
        }
    }

    fn test_request_v3(
        request: HopliteRequestV2,
        descriptor: &HopliteRequestBodyV1,
    ) -> HopliteRequestV3 {
        HopliteRequestV3 {
            request,
            body: descriptor,
            max_body_bytes: 16,
            max_chunk_bytes: 2,
            require_declared_length: 1,
        }
    }

    fn manifest_v2(handler: &str, adapter: &str) -> Value {
        Value::Map(
            vec![
                (Value::Keyword("format".into()), Value::Number(2)),
                (
                    Value::Keyword("apps".into()),
                    Value::Vector(
                        vec![Value::Map(
                            vec![
                                (Value::Keyword("id".into()), Value::Number(1)),
                                (
                                    Value::Keyword("routes".into()),
                                    Value::Vector(
                                        vec![Value::Map(
                                            vec![
                                                (
                                                    Value::Keyword("method".into()),
                                                    Value::String("GET".into()),
                                                ),
                                                (
                                                    Value::Keyword("path".into()),
                                                    Value::String("/*path".into()),
                                                ),
                                                (
                                                    Value::Keyword("handler".into()),
                                                    Value::String(handler.into()),
                                                ),
                                                (
                                                    Value::Keyword("adapter".into()),
                                                    Value::Keyword(adapter.into()),
                                                ),
                                            ]
                                            .into_iter()
                                            .collect(),
                                        )]
                                        .into(),
                                    ),
                                ),
                            ]
                            .into_iter()
                            .collect(),
                        )]
                        .into(),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
        )
    }

    fn take_event(runtime: &mut HopliteRuntime) -> Value {
        runtime.drain_ready();
        let bytes = runtime.events.borrow_mut().pop_front().unwrap();
        hta::decode(&bytes).unwrap()
    }

    #[test]
    fn synchronous_handler_returns_response_map() {
        let mut runtime = HopliteRuntime::new();
        runtime.work_start(
            "{:status 200 :headers {\"content-type\" \"text/plain\"} :body \"hello\"}",
            None,
        );
        let Value::Vector(event) = take_event(&mut runtime) else {
            panic!("event vector")
        };
        assert!(matches!(event.get(0), Some(Value::Number(0))));
    }

    #[test]
    fn example_application_bootstraps() {
        let mut runtime = HopliteRuntime::new();
        let source = format!("{}\nnil", include_str!("../../examples/app.hal"));
        let work = runtime.work_start(&source, None);
        let event = take_event(&mut runtime);
        assert!(
            matches!(&event, Value::Vector(values)
                if matches!(values.get(0), Some(Value::Number(0)))
                    && matches!(values.get(2), Some(Value::Nil))),
            "bootstrap event={event:?}; state={:?}",
            runtime.works.get(&work).map(|work| work.result.state())
        );
    }

    #[test]
    fn prepared_handler_program_is_reused_across_requests() {
        let mut runtime = HopliteRuntime::new();
        let source = format!("{}\nnil", include_str!("../../examples/app.hal"));
        runtime.work_start(&source, None);
        let _ = take_event(&mut runtime);

        let handler = runtime.handler_prepare("hoplite.app/hello").unwrap();
        assert_eq!(runtime.handlers.len(), 1);
        for uri in ["/first", "/second"] {
            let request = Value::Map(
                [(Value::Keyword("uri".into()), Value::String(uri.into()))]
                    .into_iter()
                    .collect(),
            );
            runtime.work_call(handler, request).unwrap();
            let Value::Vector(event) = take_event(&mut runtime) else {
                panic!("event vector")
            };
            assert!(matches!(event.get(0), Some(Value::Number(0))));
            assert_eq!(runtime.handlers.len(), 1);
        }
    }

    #[test]
    fn manifest_routes_requests_through_prepared_handlers() {
        let mut runtime = HopliteRuntime::new();
        runtime.work_start(
            "(ns demo) (defn show [request] {:status 200 :body (:path request)}) nil",
            None,
        );
        let _ = take_event(&mut runtime);
        let manifest = Value::Map(
            vec![(
                Value::Keyword("apps".into()),
                Value::Vector(
                    vec![Value::Map(
                        vec![
                            (Value::Keyword("id".into()), Value::Number(1)),
                            (
                                Value::Keyword("routes".into()),
                                Value::Vector(
                                    vec![Value::Map(
                                        vec![
                                            (
                                                Value::Keyword("method".into()),
                                                Value::String("GET".into()),
                                            ),
                                            (
                                                Value::Keyword("path".into()),
                                                Value::String("/users/:id".into()),
                                            ),
                                            (
                                                Value::Keyword("handler".into()),
                                                Value::String("demo/show".into()),
                                            ),
                                        ]
                                        .into_iter()
                                        .collect(),
                                    )]
                                    .into(),
                                ),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    )]
                    .into(),
                ),
            )]
            .into_iter()
            .collect(),
        );
        runtime.apps_prepare(manifest).unwrap();
        assert_eq!(runtime.handlers.len(), 1);
        let request = Value::Map(
            vec![
                (
                    Value::Keyword("request-method".into()),
                    Value::String("GET".into()),
                ),
                (
                    Value::Keyword("path".into()),
                    Value::String("/users/42".into()),
                ),
            ]
            .into_iter()
            .collect(),
        );
        runtime.app_call(1, request).unwrap();
        let event = take_event(&mut runtime);
        assert!(
            matches!(event, Value::Vector(values) if matches!(values.get(0), Some(Value::Number(0))))
        );
        assert_eq!(runtime.handlers.len(), 1);
    }

    #[test]
    fn host_call_suspends_and_resumes_the_fiber() {
        let mut runtime = HopliteRuntime::new();
        let work = runtime.work_start(
            "(do (defn ^:async delayed [] (std.foundation.coroutine/await (std.native.Host/call \"nginx\" \"sleep\" [1])) {:status 200 :body \"done\"}) (delayed))",
            None,
        );
        assert!(
            !runtime.events.borrow().is_empty(),
            "work produced no event; result={:?}",
            runtime.works.get(&work).map(|work| work.result.state())
        );
        let Value::Vector(call) = take_event(&mut runtime) else {
            panic!("host event")
        };
        assert!(matches!(call.get(0), Some(Value::Number(2))));
        let call_id = match call.get(1) {
            Some(Value::Number(value)) => *value as u64,
            _ => panic!("call id"),
        };
        runtime.call_deliver(call_id, true, Value::Nil).unwrap();
        let Value::Vector(done) = take_event(&mut runtime) else {
            panic!("completion event")
        };
        assert!(matches!(done.get(0), Some(Value::Number(0))));
        assert!(matches!(done.get(1), Some(Value::Number(value)) if *value == work as i64));
    }

    #[test]
    fn trusted_hoplite_host_intrinsics_complete_synchronously() {
        let mut runtime = HopliteRuntime::new();
        let work = runtime.work_start(
            "(do (defn decode [] (std.foundation.string/decode-utf8 (std.native.Host/call \"hoplite.host\" \"base64url-decode\" [\"aGVsbG8\"]))) (decode))",
            None,
        );
        assert!(runtime.host_pending.borrow().is_empty());
        let Value::Vector(done) = take_event(&mut runtime) else {
            panic!("completion event")
        };
        assert!(matches!(done.get(0), Some(Value::Number(0))));
        assert!(matches!(done.get(1), Some(Value::Number(value)) if *value == work as i64));
        assert!(matches!(done.get(2), Some(Value::String(value)) if value == "hello"));
    }

    #[test]
    fn one_work_owns_multiple_sequential_host_calls() {
        let mut runtime = HopliteRuntime::new();
        let work = runtime.work_start(
            "(do (defn ^:async twice [] (std.foundation.coroutine/await (std.native.Host/call \"nginx\" \"sleep\" [1])) (std.foundation.coroutine/await (std.native.Host/call \"nginx\" \"sleep\" [2])) {:status 200 :body \"done\"}) (twice))",
            None,
        );
        let Value::Vector(first) = take_event(&mut runtime) else {
            panic!("first host event")
        };
        let first_call = match first.get(1) {
            Some(Value::Number(value)) => *value as u64,
            _ => panic!("first call id"),
        };
        runtime.call_deliver(first_call, true, Value::Nil).unwrap();
        let Value::Vector(second) = take_event(&mut runtime) else {
            panic!("second host event")
        };
        assert!(matches!(second.get(0), Some(Value::Number(2))));
        let second_call = match second.get(1) {
            Some(Value::Number(value)) => *value as u64,
            _ => panic!("second call id"),
        };
        assert_ne!(first_call, second_call);
        runtime.call_deliver(second_call, true, Value::Nil).unwrap();
        let Value::Vector(done) = take_event(&mut runtime) else {
            panic!("completion event")
        };
        assert!(matches!(done.get(0), Some(Value::Number(0))));
        assert!(matches!(done.get(1), Some(Value::Number(value)) if *value == work as i64));
    }

    #[test]
    fn nested_async_service_exposes_its_first_host_call() {
        let mut runtime = HopliteRuntime::new();
        runtime.work_start(
            "(do (defn ^:async leaf [] (std.foundation.coroutine/await (std.native.Host/call \"hoplite.store\" \"load\" [\"state\"])) (std.foundation.coroutine/await (std.native.Host/call \"hoplite.store\" \"commit\" [\"state\"]))) (defn ^:async child [] (std.foundation.coroutine/await (leaf))) (defn ^:async parent [] (std.foundation.coroutine/await (child))) (parent))",
            None,
        );
        runtime.drain_ready();
        let Value::Vector(call) = take_event(&mut runtime) else {
            panic!("nested host event")
        };
        assert!(matches!(call.get(0), Some(Value::Number(2))));
        assert!(matches!(call.get(5), Some(Value::String(service)) if service == "hoplite.store"));
        assert!(matches!(call.get(6), Some(Value::String(method)) if method == "load"));
        let first_call = match call.get(1) {
            Some(Value::Number(value)) => *value as u64,
            _ => panic!("first call id"),
        };
        runtime
            .call_deliver(first_call, true, Value::Nil)
            .expect("first call is delivered");
        let Value::Vector(call) = take_event(&mut runtime) else {
            panic!("second nested host event")
        };
        assert!(matches!(call.get(0), Some(Value::Number(2))));
        assert!(matches!(call.get(6), Some(Value::String(method)) if method == "commit"));
    }

    #[test]
    fn request_adapter_completes_without_work_or_hta_events() {
        let mut runtime = HopliteRuntime::new();
        runtime.work_start(
            "(ns direct.request) (defn show [request] {:status 200 :headers {\"x-path\" (:path request)} :body (get (:headers request) \"x-test\")}) nil",
            None,
        );
        let bootstrap = take_event(&mut runtime);
        assert!(
            matches!(&bootstrap, Value::Vector(values) if matches!(values.get(0), Some(Value::Number(0)))),
            "bootstrap failed: {bootstrap:?}"
        );
        let works_before = runtime.works.len();
        runtime
            .apps_prepare(manifest_v2("direct.request/show", "request"))
            .unwrap();
        let mut context = TestRequest {
            headers: vec![("x-test", "lazy")],
        };
        let outcome = runtime
            .app_invoke(1, test_request(&mut context, "/hello"))
            .unwrap();
        let InvokeState::Complete(response) = outcome else {
            panic!("request route suspended")
        };
        let response = runtime.responses.get(&response).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"lazy");
        assert_eq!(response.headers, vec![("x-path".into(), "/hello".into())]);
        assert_eq!(runtime.works.len(), works_before);
        assert!(runtime.events.borrow().is_empty());
        assert!(runtime.requests.borrow().is_empty());
    }

    #[test]
    fn raw_adapter_uses_the_exchange_response_api() {
        let mut runtime = HopliteRuntime::new();
        runtime.work_start(
            "(ns direct.raw) (defn show [exchange] (hoplite.raw.native/respond exchange 201 {\"x-mode\" \"raw\"} (:path exchange))) nil",
            None,
        );
        let _ = take_event(&mut runtime);
        let works_before = runtime.works.len();
        runtime
            .apps_prepare(manifest_v2("direct.raw/show", "raw"))
            .unwrap();
        let mut context = TestRequest { headers: vec![] };
        let outcome = runtime
            .app_invoke(1, test_request(&mut context, "/raw"))
            .unwrap();
        let InvokeState::Complete(response) = outcome else {
            panic!("raw route suspended")
        };
        let response = runtime.responses.get(&response).unwrap();
        assert_eq!(response.status, 201);
        assert_eq!(response.body, b"/raw");
        assert_eq!(response.headers, vec![("x-mode".into(), "raw".into())]);
        assert_eq!(runtime.works.len(), works_before);
    }

    #[test]
    fn request_v3_projects_only_an_opaque_body_handle() {
        let mut runtime = HopliteRuntime::new();
        runtime.work_start(
            "(ns body.sync) (defn show [request] {:status 200 :body (if (:body-handle request) \"present\" \"missing\")}) nil",
            None,
        );
        let _ = take_event(&mut runtime);
        let handler = runtime.handler_prepare("body.sync/show").unwrap();
        let mut request_context = TestRequest { headers: vec![] };
        let mut body_context = TestBody {
            bytes: b"body".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let descriptor = test_body_descriptor(&mut body_context);
        let request = test_request_v3(test_request(&mut request_context, "/body"), &descriptor);
        let (request, body) = runtime.register_request_v3(request).unwrap();
        let InvokeState::Complete(response) = runtime
            .handler_invoke_with_body(handler, RouteAdapter::Request, request, body)
            .unwrap()
        else {
            panic!("body route suspended")
        };
        assert_eq!(runtime.responses[&response].body, b"present");
        assert!(runtime.resources.borrow().is_empty());
        assert_eq!(body_context.close_count, 1);
    }

    #[test]
    fn request_v3_body_survives_async_work_and_closes_with_request_scope() {
        let mut runtime = HopliteRuntime::new();
        runtime.work_start(
            "(ns body.async) (defn show [request] (std.foundation.coroutine/await (std.native.Host/call \"nginx\" \"use-body\" [(:body-handle request)])) {:status 200 :body \"done\"}) nil",
            None,
        );
        let _ = take_event(&mut runtime);
        let handler = runtime.handler_prepare("body.async/show").unwrap();
        let mut request_context = TestRequest { headers: vec![] };
        let mut body_context = TestBody {
            bytes: b"abcd".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let descriptor = test_body_descriptor(&mut body_context);
        let request = test_request_v3(test_request(&mut request_context, "/body"), &descriptor);
        let (request, body) = runtime.register_request_v3(request).unwrap();
        let handle = body.expect("body handle");
        let InvokeState::Suspended(work) = runtime
            .handler_invoke_with_body(handler, RouteAdapter::Request, request, Some(handle))
            .unwrap()
        else {
            panic!("body route completed synchronously")
        };
        let Value::Vector(call) = take_event(&mut runtime) else {
            panic!("host event")
        };
        let call_id = match call.get(1) {
            Some(Value::Number(value)) => *value as u64,
            _ => panic!("call id"),
        };
        let event_work = match call.get(2) {
            Some(Value::Number(value)) => *value as u64,
            _ => panic!("work id"),
        };
        assert_eq!(event_work, work);
        let arguments = match call.get(7) {
            Some(Value::Vector(arguments)) => arguments,
            _ => panic!("host argument vector"),
        };
        assert!(
            matches!(arguments.peek_first(), Some(Value::Number(value)) if value == handle.get() as i64)
        );

        let foreign_work = runtime.start_value(Value::Nil);
        let mut output = [0_u8; 8];
        let mut returned = usize::MAX;
        assert_eq!(
            unsafe {
                hoplite_request_body_read_v3(
                    &mut runtime,
                    foreign_work,
                    handle.get(),
                    output.as_mut_ptr(),
                    output.len(),
                    &mut returned,
                )
            },
            1
        );
        assert_eq!(returned, 0);
        assert_eq!(
            unsafe {
                hoplite_request_body_read_v3(
                    &mut runtime,
                    work,
                    handle.get(),
                    output.as_mut_ptr(),
                    output.len(),
                    &mut returned,
                )
            },
            0
        );
        assert_eq!(returned, 2);
        assert_eq!(&output[..2], b"ab");
        assert_eq!(
            unsafe {
                hoplite_request_body_read_v3(
                    &mut runtime,
                    work,
                    handle.get(),
                    output.as_mut_ptr(),
                    output.len(),
                    &mut returned,
                )
            },
            0
        );
        assert_eq!(returned, 2);
        assert_eq!(&output[..2], b"cd");
        assert_eq!(
            unsafe { hoplite_request_body_finish_v3(&mut runtime, foreign_work, handle.get()) },
            1
        );
        assert_eq!(
            unsafe { hoplite_request_body_finish_v3(&mut runtime, work, handle.get()) },
            0
        );
        assert!(runtime.work_close(foreign_work));

        runtime.call_deliver(call_id, true, Value::Nil).unwrap();
        let Value::Vector(done) = take_event(&mut runtime) else {
            panic!("completion event")
        };
        assert!(matches!(done.get(0), Some(Value::Number(0))));
        assert_eq!(body_context.close_count, 0);
        assert!(runtime.work_close(work));
        assert_eq!(body_context.close_count, 1);
        assert!(!runtime.resources.borrow().contains(handle));
        returned = usize::MAX;
        assert_eq!(
            unsafe {
                hoplite_request_body_read_v3(
                    &mut runtime,
                    work,
                    handle.get(),
                    output.as_mut_ptr(),
                    output.len(),
                    &mut returned,
                )
            },
            1
        );
        assert_eq!(returned, 0);
    }

    #[test]
    fn request_v3_invalid_adapter_closes_transferred_body() {
        let mut runtime = HopliteRuntime::new();
        let mut request_context = TestRequest { headers: vec![] };
        let mut body_context = TestBody {
            bytes: b"body".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let descriptor = test_body_descriptor(&mut body_context);
        let request = test_request_v3(test_request(&mut request_context, "/body"), &descriptor);
        let mut outcome = HopliteOutcomeV2 { kind: 7, id: 9 };
        assert_eq!(
            unsafe { hoplite_handler_invoke_v3(&mut runtime, 0, 99, &request, &mut outcome,) },
            1
        );
        assert_eq!(outcome.kind, 0);
        assert_eq!(outcome.id, 0);
        assert_eq!(body_context.close_count, 1);
        assert!(runtime.resources.borrow().is_empty());
    }

    #[test]
    fn request_v3_rejects_portable_adapter_and_closes_body() {
        let mut runtime = HopliteRuntime::new();
        runtime.work_start(
            "(ns body.portable) (defn show [request] {:status 200 :body \"unused\"}) nil",
            None,
        );
        let _ = take_event(&mut runtime);
        let handler = runtime.handler_prepare("body.portable/show").unwrap();
        let mut request_context = TestRequest { headers: vec![] };
        let mut body_context = TestBody {
            bytes: b"body".to_vec(),
            cursor: 0,
            close_count: 0,
        };
        let descriptor = test_body_descriptor(&mut body_context);
        let request = test_request_v3(test_request(&mut request_context, "/body"), &descriptor);
        let (request, body) = runtime.register_request_v3(request).unwrap();
        let error = match runtime.handler_invoke_with_body(
            handler,
            RouteAdapter::RequestHta,
            request,
            body,
        ) {
            Ok(_) => panic!("portable body route unexpectedly accepted"),
            Err(error) => error,
        };
        assert!(error.contains("body handles require request or raw adapter"));
        assert_eq!(body_context.close_count, 1);
        assert!(runtime.resources.borrow().is_empty());
    }

    #[test]
    fn request_v3_rejects_ambiguous_absent_body_limits() {
        let mut runtime = HopliteRuntime::new();
        let mut request_context = TestRequest { headers: vec![] };
        let request = HopliteRequestV3 {
            request: test_request(&mut request_context, "/body"),
            body: ptr::null(),
            max_body_bytes: 16,
            max_chunk_bytes: 2,
            require_declared_length: 1,
        };
        assert!(runtime.register_request_v3(request).is_err());
        assert!(runtime.resources.borrow().is_empty());
    }

    #[test]
    fn request_handler_yields_without_async_metadata() {
        let mut runtime = HopliteRuntime::new();
        runtime.work_start(
            "(ns direct.async) (defn show [request] (std.foundation.coroutine/await (std.native.Host/call \"nginx\" \"sleep\" [1])) {:status 200 :body (:path request)}) nil",
            None,
        );
        let _ = take_event(&mut runtime);
        runtime
            .apps_prepare(manifest_v2("direct.async/show", "request"))
            .unwrap();
        let mut context = TestRequest { headers: vec![] };
        let InvokeState::Suspended(work) = runtime
            .app_invoke(1, test_request(&mut context, "/async"))
            .unwrap()
        else {
            panic!("async route completed synchronously")
        };
        let Value::Vector(call) = take_event(&mut runtime) else {
            panic!("host event")
        };
        let call_id = match call.get(1) {
            Some(Value::Number(value)) => *value as u64,
            _ => panic!("call id"),
        };
        runtime.call_deliver(call_id, true, Value::Nil).unwrap();
        let Value::Vector(done) = take_event(&mut runtime) else {
            panic!("completion event")
        };
        assert!(matches!(done.get(0), Some(Value::Number(0))));
        assert!(runtime.work_close(work));
        assert!(runtime.requests.borrow().is_empty());
    }
}
