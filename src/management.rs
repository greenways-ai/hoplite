use crate::auth::{Principal, Store};
use crate::platform::SessionPolicy;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;

const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

#[derive(Debug)]
struct Response {
    status: u16,
    body: Value,
}

pub fn serve(
    store_path: &Path,
    listen: &str,
    session_policy: &SessionPolicy,
) -> Result<(), String> {
    if !is_loopback_address(listen) {
        return Err("the management gateway must bind to a loopback address".into());
    }
    let listener = TcpListener::bind(listen)
        .map_err(|error| format!("cannot bind Hoplite management gateway at {listen}: {error}"))?;
    let mut store = Store::open(store_path)?;
    println!("Hoplite management gateway listening on http://{listen}");
    for connection in listener.incoming() {
        match connection {
            Ok(mut stream) => {
                let response = match read_request(&mut stream)
                    .and_then(|request| route(&mut store, request, session_policy))
                {
                    Ok(response) => response,
                    Err(error) => error_response(error),
                };
                if let Err(error) = write_response(&mut stream, response) {
                    eprintln!("hoplite: management response failed: {error}");
                }
            }
            Err(error) => eprintln!("hoplite: management connection failed: {error}"),
        }
    }
    Ok(())
}

fn route(
    store: &mut Store,
    request: Request,
    session_policy: &SessionPolicy,
) -> Result<Response, ApiError> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => Ok(ok(json!({"status": "ok"}))),
        ("POST", "/v1/auth/enroll") => {
            let body = json_body(&request)?;
            let principal = store
                .enroll_management_device(
                    string_field(&body, "bootstrap_token")?,
                    string_field(&body, "public_key")?,
                )
                .map_err(auth_error)?;
            Ok(created(principal_json(&principal)))
        }
        ("POST", "/v1/auth/challenges") => {
            let body = json_body(&request)?;
            let (challenge_id, nonce) = store
                .create_challenge("management", string_field(&body, "public_key")?)
                .map_err(auth_error)?;
            Ok(created(json!({
                "challenge_id": challenge_id,
                "nonce": nonce,
                "algorithm": "Ed25519"
            })))
        }
        ("POST", "/v1/auth/sessions") => {
            let body = json_body(&request)?;
            let tokens = store
                .exchange_challenge(
                    string_field(&body, "challenge_id")?,
                    string_field(&body, "signature")?,
                    session_policy.access_ttl_seconds,
                    session_policy.refresh_ttl_seconds,
                )
                .map_err(auth_error)?;
            Ok(created(json!({
                "access_token": tokens.access_token,
                "refresh_token": tokens.refresh_token,
                "session_id": tokens.session_id,
                "access_expires_at": tokens.access_expires_at,
                "refresh_expires_at": tokens.refresh_expires_at
            })))
        }
        ("POST", "/v1/auth/refresh") => {
            let body = json_body(&request)?;
            let tokens = store
                .rotate_refresh_token(
                    string_field(&body, "refresh_token")?,
                    session_policy.access_ttl_seconds,
                    session_policy.refresh_ttl_seconds,
                    session_policy.reuse_interval_seconds,
                )
                .map_err(auth_error)?;
            Ok(ok(json!({
                "access_token": tokens.access_token,
                "refresh_token": tokens.refresh_token,
                "session_id": tokens.session_id,
                "access_expires_at": tokens.access_expires_at,
                "refresh_expires_at": tokens.refresh_expires_at
            })))
        }
        ("GET", "/v1/auth/me") => {
            let principal = authenticate_management(store, &request)?;
            Ok(ok(principal_json(&principal)))
        }
        ("POST", "/v1/auth/revoke") => {
            authenticate_management(store, &request)?;
            let body = json_body(&request)?;
            let session_id = string_field(&body, "session_id")?;
            if !store.revoke_session(session_id).map_err(internal_error)? {
                return Err(ApiError::not_found("session not found"));
            }
            Ok(ok(json!({"revoked": true, "session_id": session_id})))
        }
        _ => Err(ApiError::not_found("management endpoint not found")),
    }
}

fn authenticate_management(store: &Store, request: &Request) -> Result<Principal, ApiError> {
    let authorization = request
        .headers
        .get("authorization")
        .ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;
    let token = authorization
        .strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::unauthorized("authorization must use Bearer"))?;
    store.authenticate("management", token).map_err(auth_error)
}

fn read_request(stream: &mut TcpStream) -> Result<Request, ApiError> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| ApiError::bad_request(format!("cannot read request: {error}")))?;
        if read == 0 {
            return Err(ApiError::bad_request("incomplete HTTP request"));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > MAX_REQUEST_BYTES {
            return Err(ApiError::payload_too_large());
        }
        if let Some(index) = find_bytes(&buffer, b"\r\n\r\n") {
            break index + 4;
        }
    };
    let header = std::str::from_utf8(&buffer[..header_end - 4])
        .map_err(|_| ApiError::bad_request("HTTP headers must be UTF-8"))?;
    let mut lines = header.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| ApiError::bad_request("missing request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| ApiError::bad_request("missing HTTP method"))?
        .to_owned();
    let path = request_parts
        .next()
        .ok_or_else(|| ApiError::bad_request("missing request path"))?
        .split('?')
        .next()
        .unwrap_or_default()
        .to_owned();
    if request_parts.next() != Some("HTTP/1.1") || request_parts.next().is_some() {
        return Err(ApiError::bad_request("management API requires HTTP/1.1"));
    }
    let mut headers = BTreeMap::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| ApiError::bad_request("malformed HTTP header"))?;
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || headers.insert(name, value.trim().to_owned()).is_some() {
            return Err(ApiError::bad_request("empty or duplicate HTTP header"));
        }
    }
    let content_length = headers
        .get("content-length")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| ApiError::bad_request("invalid Content-Length"))
        })
        .transpose()?
        .unwrap_or(0);
    if header_end + content_length > MAX_REQUEST_BYTES {
        return Err(ApiError::payload_too_large());
    }
    while buffer.len() < header_end + content_length {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| ApiError::bad_request(format!("cannot read body: {error}")))?;
        if read == 0 {
            return Err(ApiError::bad_request("incomplete HTTP body"));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Ok(Request {
        method,
        path,
        headers,
        body: buffer[header_end..header_end + content_length].to_vec(),
    })
}

fn write_response(stream: &mut TcpStream, response: Response) -> Result<(), String> {
    let body = serde_json::to_vec(&response.body)
        .map_err(|error| format!("cannot encode management response: {error}"))?;
    let reason = match response.status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        _ => "Internal Server Error",
    };
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        response.status,
        reason,
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(&body))
        .map_err(|error| format!("cannot write management response: {error}"))
}

fn json_body(request: &Request) -> Result<Value, ApiError> {
    if !matches!(
        request.headers.get("content-type"),
        Some(value) if value.split(';').next() == Some("application/json")
    ) {
        return Err(ApiError::bad_request(
            "Content-Type must be application/json",
        ));
    }
    serde_json::from_slice(&request.body)
        .map_err(|_| ApiError::bad_request("request body must be valid JSON"))
}

fn string_field<'a>(body: &'a Value, field: &str) -> Result<&'a str, ApiError> {
    body.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request(format!("{field} must be a non-empty string")))
}

fn principal_json(principal: &Principal) -> Value {
    json!({
        "principal": {
            "id": principal.id,
            "realm": principal.realm,
            "session_id": principal.session_id,
            "device_id": principal.device_id
        }
    })
}

fn ok(body: Value) -> Response {
    Response { status: 200, body }
}

fn created(body: Value) -> Response {
    Response { status: 201, body }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn is_loopback_address(address: &str) -> bool {
    address
        .parse::<std::net::SocketAddr>()
        .is_ok_and(|address| address.ip().is_loopback())
}

#[derive(Debug)]
struct ApiError {
    status: u16,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: 401,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: 404,
            message: message.into(),
        }
    }

    fn payload_too_large() -> Self {
        Self {
            status: 413,
            message: "request exceeds 64 KiB".into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: 500,
            message: message.into(),
        }
    }
}

fn auth_error(message: String) -> ApiError {
    ApiError::unauthorized(message)
}

fn internal_error(message: String) -> ApiError {
    ApiError::internal(message)
}

fn error_response(error: ApiError) -> Response {
    Response {
        status: error.status,
        body: json!({"error": {"message": error.message}}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, path: &str) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn health_is_public_but_management_identity_is_not() {
        let mut store = Store::open(":memory:").unwrap();
        assert_eq!(
            route(
                &mut store,
                request("GET", "/health"),
                &SessionPolicy::default()
            )
            .unwrap()
            .status,
            200
        );
        assert_eq!(
            route(
                &mut store,
                request("GET", "/v1/auth/me"),
                &SessionPolicy::default()
            )
            .unwrap_err()
            .status,
            401
        );
    }

    #[test]
    fn refuses_non_loopback_management_bindings() {
        assert!(is_loopback_address("127.0.0.1:9090"));
        assert!(is_loopback_address("[::1]:9090"));
        assert!(!is_loopback_address("0.0.0.0:9090"));
    }

    #[test]
    fn error_responses_do_not_leak_html() {
        let response = error_response(ApiError::bad_request("bad input"));
        assert_eq!(response.status, 400);
        assert_eq!(response.body["error"]["message"], "bad input");
    }
}
