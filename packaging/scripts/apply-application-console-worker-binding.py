#!/usr/bin/env python3
from pathlib import Path

root = Path(__file__).resolve().parents[2]


def replace_once(path: Path, old: str, new: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(
            f"{path}: expected one replacement, found {count}: {old[:80]!r}"
        )
    path.write_text(text.replace(old, new, 1))


app = root / "core/src/app.rs"
replace_once(
    app,
    "    pub routes: Vec<Route>,\n    pub request_body: Option<RequestBodyPolicy>,",
    "    pub routes: Vec<Route>,\n    pub console: Option<String>,\n    pub request_body: Option<RequestBodyPolicy>,",
)
replace_once(
    app,
    '    let name = text_field(&value, "name").unwrap_or_else(|| format!("app-{id}"));\n    let default_adapter = RouteAdapter::parse(',
    '''    let name = text_field(&value, "name").unwrap_or_else(|| format!("app-{id}"));
    let console = field(&value, "console")
        .map(|value| callable_name(&value))
        .transpose()
        .map_err(|_| {
            format!("Hoplite app {name:?} :console must be a Var such as #'app/console")
        })?;
    let default_adapter = RouteAdapter::parse(''',
)
replace_once(
    app,
    "        routes,\n        request_body,",
    "        routes,\n        console,\n        request_body,",
)

text = app.read_text()
start = text.index("pub fn manifest(config: &Config) -> Result<Vec<u8>, String> {")
end = text.index("\npub fn openapi(app: &App) -> String {", start)
manifest = '''pub fn manifest(config: &Config) -> Result<Vec<u8>, String> {
    let format = 2;
    let apps = config
        .apps
        .iter()
        .map(|app| {
            let mut fields = vec![
                (keyword("id"), Value::Number(app.id as i64)),
                (
                    keyword("peers"),
                    Value::Vector(
                        app.peers
                            .iter()
                            .map(|peer| {
                                map_value(vec![
                                    (keyword("name"), Value::String(peer.name.clone())),
                                    (keyword("channel"), Value::String(peer.channel.clone())),
                                    (keyword("handler"), Value::String(peer.handler.clone())),
                                    (keyword("label"), Value::String(peer.label.clone())),
                                    (
                                        keyword("max-message-bytes"),
                                        Value::Number(peer.max_message_bytes as i64),
                                    ),
                                    (
                                        keyword("idle-timeout-seconds"),
                                        Value::Number(peer.idle_timeout_seconds as i64),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
                (
                    keyword("routes"),
                    Value::Vector(
                        app.routes
                            .iter()
                            .map(|route| {
                                map_value(vec![
                                    (keyword("method"), Value::String(route.method.clone())),
                                    (keyword("path"), Value::String(route.path.clone())),
                                    (keyword("handler"), Value::String(route.handler.clone())),
                                    (
                                        keyword("adapter"),
                                        Value::Keyword(route.adapter.keyword().into()),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ];
            if let Some(console) = &app.console {
                fields.push((keyword("console"), Value::String(console.clone())));
            }
            map_value(fields)
        })
        .collect();
    hara_wasm::hta::encode(&map_value(vec![
        (keyword("format"), Value::Number(format)),
        (keyword("apps"), Value::Vector(apps)),
    ]))
}
'''
app.write_text(text[:start] + manifest + text[end:])

marker = '''    #[test]
    fn semantic_route_authorization_belongs_to_the_hal_handler() {'''
test = '''    #[test]
    fn app_console_is_fixed_in_the_manifest() {
        let mut runtime = Runtime::new();
        runtime.register_resource("hoplite.core", CORE_SOURCE);
        let value = runtime
            .eval_native_value(
                "(ns sample.console (:require [hoplite.core :as h])) \\
                 (defn dispatch [request] request) \\
                 (defn root [_request] {:status 200 :body \\\"ok\\\"}) \\
                 (h/app {:name :sample \\
                         :console #'dispatch \\
                         :resources [[\\\"/\\\" {:get {:handler #'root}}]]})",
            )
            .unwrap();
        let app = parse_app(value, 1, 8080, vec![], false).unwrap();
        assert_eq!(app.console.as_deref(), Some("sample.console/dispatch"));

        let encoded = manifest(&Config {
            workers: 1,
            apps: vec![app],
        })
        .unwrap();
        let decoded = hara_wasm::hta::decode(&encoded).unwrap();
        let apps = sequence_field(&decoded, "apps").unwrap();
        assert_eq!(
            text_field(&apps[0], "console").as_deref(),
            Some("sample.console/dispatch")
        );
    }

'''
replace_once(app, marker, test + marker)

runtime = root / "core/runtime/src/lib.rs"
replace_once(
    runtime,
    '''struct AppRouter {
    routes: Vec<AppRoute>,
}''',
    '''struct AppRouter {
    routes: Vec<AppRoute>,
    console: Option<HandlerId>,
}''',
)
replace_once(
    runtime,
    '''                if apps.insert(id, AppRouter { routes }).is_some() {
                    return Err(format!("duplicate app id {id}"));
                }''',
    '''                let console = map_optional_string(&app, "console")
                    .map(|function| self.handler_prepare(&function))
                    .transpose()?;
                if apps.insert(id, AppRouter { routes, console }).is_some() {
                    return Err(format!("duplicate app id {id}"));
                }''',
)
replace_once(
    runtime,
    '''    fn start_value(&mut self, value: Value) -> WorkId {''',
    '''    fn app_console_call(&mut self, app: AppId, input: Value) -> Result<WorkId, ()> {
        let handler = self
            .apps
            .get(&app)
            .and_then(|router| router.console)
            .ok_or(())?;
        self.work_call(handler, input)
    }

    fn start_value(&mut self, value: Value) -> WorkId {''',
)
export_marker = '''#[no_mangle]
pub unsafe extern "C" fn hoplite_app_invoke_v2('''
export = '''#[no_mangle]
pub unsafe extern "C" fn hoplite_app_console_call(
    runtime: *mut HopliteRuntime,
    app: u64,
    input_ptr: *const u8,
    input_len: usize,
) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        let runtime = runtime_mut(runtime)?;
        let input = hta::decode(bytes(input_ptr, input_len)?).map_err(|_| ())?;
        runtime.app_console_call(app, input)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(0)
}

'''
replace_once(runtime, export_marker, export + export_marker)

helper_marker = '''    fn take_event(runtime: &mut HopliteRuntime) -> Value {'''
helper = '''    fn manifest_v2_with_console(
        route_handler: &str,
        adapter: &str,
        console_handler: &str,
    ) -> Value {
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
                                    Value::Keyword("console".into()),
                                    Value::String(console_handler.into()),
                                ),
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
                                                    Value::String(route_handler.into()),
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

'''
replace_once(runtime, helper_marker, helper + helper_marker)

runtime_test_marker = '''    #[test]
    fn host_call_suspends_and_resumes_the_fiber() {'''
runtime_test = '''    #[test]
    fn application_console_uses_only_the_manifest_selected_handler() {
        let mut runtime = HopliteRuntime::new();
        runtime.work_start(
            "(ns console.bridge) \\
             (defn route [_request] {:status 200 :body \\\"route\\\"}) \\
             (defn dispatch [request] request) \\
             nil",
            None,
        );
        let _ = take_event(&mut runtime);
        runtime
            .apps_prepare(manifest_v2_with_console(
                "console.bridge/route",
                "request",
                "console.bridge/dispatch",
            ))
            .unwrap();
        assert_eq!(runtime.handlers.len(), 2);

        let input = Value::Map(
            vec![
                (
                    Value::Keyword("grant".into()),
                    Value::Map(Default::default()),
                ),
                (
                    Value::Keyword("command".into()),
                    Value::String("status".into()),
                ),
                (
                    Value::Keyword("input".into()),
                    Value::Map(Default::default()),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let encoded = hta::encode(&input).unwrap();
        let runtime_ptr = &mut runtime as *mut HopliteRuntime;
        let work = unsafe {
            hoplite_app_console_call(runtime_ptr, 1, encoded.as_ptr(), encoded.len())
        };
        assert_ne!(work, 0);
        let Value::Vector(event) = take_event(&mut runtime) else {
            panic!("console completion event")
        };
        assert!(matches!(event.get(0), Some(Value::Number(0))));
        assert!(matches!(event.get(1), Some(Value::Number(value)) if *value == work as i64));
        assert_eq!(event.get(2), Some(&input));
        assert_eq!(
            unsafe { hoplite_app_console_call(runtime_ptr, 2, encoded.as_ptr(), encoded.len()) },
            0
        );
    }

'''
replace_once(runtime, runtime_test_marker, runtime_test + runtime_test_marker)

header = root / "core/nginx/hoplite_runtime.h"
replace_once(
    header,
    '''uint64_t hoplite_app_call(hoplite_runtime_t *runtime,
                          uint64_t app,
                          const uint8_t *input,
                          size_t input_len);
int hoplite_app_invoke_v2''',
    '''uint64_t hoplite_app_call(hoplite_runtime_t *runtime,
                          uint64_t app,
                          const uint8_t *input,
                          size_t input_len);
/*
 * Invoke the one console handler selected from the immutable application
 * manifest. The caller supplies only app identity and HTA input; no source,
 * symbol, function name, or handler identifier crosses this boundary.
 */
uint64_t hoplite_app_console_call(hoplite_runtime_t *runtime,
                                  uint64_t app,
                                  const uint8_t *input,
                                  size_t input_len);
int hoplite_app_invoke_v2''',
)
