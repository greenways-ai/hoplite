#![allow(clippy::missing_safety_doc)]

use hara_wasm::{core, hta, kernel, vm};

use core::{Promise, PromiseRejection, PromiseState, Value};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::rc::Rc;

const ABI_VERSION: u32 = 1;
type HostCall = (u64, Promise, String, String, Vec<Value>);

type WorkId = u64;
type CallId = u64;
type HandlerId = u64;
type AppId = u64;

struct AppRoute {
    method: String,
    path: String,
    handler: HandlerId,
}

struct AppRouter {
    routes: Vec<AppRoute>,
}

struct Work {
    fiber: Option<vm::VmFiber>,
    result: Promise,
    children: Vec<Promise>,
    calls: HashMap<CallId, Promise>,
}

impl Work {
    fn new(result: Promise) -> Self {
        Self {
            fiber: None,
            result,
            children: Vec::new(),
            calls: HashMap::new(),
        }
    }
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
    events: Rc<RefCell<VecDeque<Vec<u8>>>>,
    ready: Rc<RefCell<VecDeque<(u64, PromiseState)>>>,
    call_owners: HashMap<CallId, WorkId>,
    handlers: HashMap<HandlerId, vm::PreparedCall>,
    apps: HashMap<AppId, AppRouter>,
    works: HashMap<WorkId, Work>,
}

impl HopliteRuntime {
    fn new() -> Self {
        let namespaces = hara_wasm::embedding_namespace_registry();

        Self {
            namespaces,
            protocols: core::ProtocolRegistry::core(),
            next_handler: 1,
            next_work: 1,
            next_call: 1,
            events: Rc::new(RefCell::new(VecDeque::new())),
            ready: Rc::new(RefCell::new(VecDeque::new())),
            call_owners: HashMap::new(),
            handlers: HashMap::new(),
            apps: HashMap::new(),
            works: HashMap::new(),
        }
    }

    fn allocate_work(&mut self) -> WorkId {
        let work = self.next_work;
        self.next_work = self.next_work.saturating_add(1);
        work
    }

    fn host_handler(
        &mut self,
        _work: WorkId,
    ) -> (
        Rc<dyn Fn(String, String, Vec<Value>) -> Result<Value, String>>,
        Rc<RefCell<Vec<HostCall>>>,
        Rc<RefCell<u64>>,
    ) {
        let pending = Rc::new(RefCell::new(Vec::new()));
        let queue = pending.clone();
        let next = Rc::new(RefCell::new(self.next_call));
        let ids = next.clone();
        let handler = Rc::new(move |service: String, method: String, args: Vec<Value>| {
            let call = *ids.borrow();
            *ids.borrow_mut() = call.saturating_add(1);
            let promise = Promise::new();
            queue
                .borrow_mut()
                .push((call, promise.clone(), service, method, args));
            Ok(Value::Promise(promise))
        });
        (handler, pending, next)
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
                    self.namespaces.current().intern("__hoplite_request", binding);
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
        let mut apps = HashMap::new();
        for app in map_sequence(&manifest, "apps")? {
            let id = map_number(&app, "id")? as u64;
            let mut routes = Vec::new();
            for route in map_sequence(&app, "routes")? {
                let method = map_string(&route, "method")?;
                let path = map_string(&route, "path")?;
                let function = map_string(&route, "handler")?;
                let handler = self.handler_prepare(&function)?;
                routes.push(AppRoute {
                    method,
                    path,
                    handler,
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
            .filter_map(|route| route_score(&route.path, &path).map(|score| (score, route.handler)))
            .max_by_key(|(score, _)| *score)
            .map(|(_, handler)| handler);
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
            let children = self
                .works
                .get(&work)
                .map(|owner| owner.children.clone())
                .unwrap_or_default();
            if children.is_empty() {
                continue;
            }
            let (handler, pending, next) = self.host_handler(work);
            let namespaces = self.namespaces.clone();
            let protocols = self.protocols.clone();
            core::with_namespace_registry(&namespaces, || {
                core::with_protocols(&protocols, || {
                    core::with_host_calls(handler, || {
                        for child in &children {
                            child.state();
                        }
                    });
                });
            });
            self.collect_calls(work, pending, next);
            if let Some(owner) = self.works.get_mut(&work) {
                owner
                    .children
                    .retain(|child| matches!(child.state(), PromiseState::Pending));
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
        if success {
            promise.resolve(payload);
        } else {
            promise.reject_value(payload);
        }
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
}
