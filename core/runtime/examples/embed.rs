use hoplite_runtime::{
    hoplite_abi_version, hoplite_bootstrap_modules, hoplite_handler_close,
    hoplite_handler_invoke_v2, hoplite_handler_prepare, hoplite_response_body_v2,
    hoplite_response_close_v2, hoplite_response_status_v2, hoplite_runtime_free,
    hoplite_runtime_new, HopliteOutcomeV2, HopliteRequestV2, HopliteSlice,
};
use std::ffi::c_void;
use std::ptr;
use std::slice;

fn text(value: &str) -> HopliteSlice {
    HopliteSlice {
        data: value.as_ptr(),
        len: value.len(),
    }
}

fn main() {
    assert!(hoplite_abi_version() >= 5);

    let runtime = hoplite_runtime_new();
    assert!(!runtime.is_null());

    unsafe {
        let source = r#"
(ns embedding.example (:require [std.foundation :refer :all]))
(defn respond [request]
  {:status 200
   :headers {"content-type" "text/plain"}
   :body "embedded"})
"#;
        assert_eq!(
            hoplite_bootstrap_modules(runtime, source.as_ptr(), source.len()),
            0
        );

        let function = "embedding.example/respond";
        let handler = hoplite_handler_prepare(runtime, function.as_ptr(), function.len());
        assert_ne!(handler, 0);

        let request = HopliteRequestV2 {
            context: ptr::null_mut::<c_void>(),
            method: text("GET"),
            uri: text("/embed"),
            path: text("/embed"),
            query_string: text(""),
            remote_address: text("127.0.0.1"),
            header_count: 0,
            header_at: None,
        };
        let mut outcome = HopliteOutcomeV2 { kind: 0, id: 0 };
        assert_eq!(
            hoplite_handler_invoke_v2(runtime, handler, 1, &request, &mut outcome),
            0
        );
        assert_eq!(outcome.kind, 1);
        assert_ne!(outcome.id, 0);

        let mut status = 0_u16;
        assert_eq!(
            hoplite_response_status_v2(runtime, outcome.id, &mut status),
            0
        );
        assert_eq!(status, 200);

        let mut body = HopliteSlice {
            data: ptr::null(),
            len: 0,
        };
        assert_eq!(hoplite_response_body_v2(runtime, outcome.id, &mut body), 0);
        assert!(!body.data.is_null());
        assert_eq!(slice::from_raw_parts(body.data, body.len), b"embedded");

        assert_eq!(hoplite_response_close_v2(runtime, outcome.id), 0);
        assert_eq!(hoplite_handler_close(runtime, handler), 0);
        hoplite_runtime_free(runtime);
    }
}
