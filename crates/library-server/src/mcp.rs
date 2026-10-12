use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};

use crate::{tool_definitions, LibraryEngine, RequestContext, ServerState, MCP_PROTOCOL_VERSION};

const SESSION_HEADER: &str = "mcp-session-id";
const VERSION_HEADER: &str = "mcp-protocol-version";

pub struct SessionStore {
    sessions: Mutex<HashMap<String, Session>>,
    max_sessions: usize,
    ttl: Duration,
}

struct Session {
    principal_id: String,
    library_epoch: String,
    expires_at: Instant,
    initialized: bool,
}

impl SessionStore {
    pub fn new(max_sessions: usize, ttl: Duration) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            max_sessions,
            ttl,
        }
    }

    fn create(&self, principal_id: &str, library_epoch: &str) -> Result<String, ()> {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, session| session.expires_at > Instant::now());
        if sessions.len() >= self.max_sessions {
            return Err(());
        }
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|_| ())?;
        let id = URL_SAFE_NO_PAD.encode(random);
        sessions.insert(
            id.clone(),
            Session {
                principal_id: principal_id.into(),
                library_epoch: library_epoch.into(),
                expires_at: Instant::now() + self.ttl,
                initialized: false,
            },
        );
        Ok(id)
    }

    fn initialize(&self, id: &str, principal: &str) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(value) = sessions
            .get_mut(id)
            .filter(|value| value.principal_id == principal)
        {
            value.initialized = true;
            true
        } else {
            false
        }
    }
    fn is_initialized(&self, id: &str) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|value| value.initialized)
    }

    fn validate(&self, id: &str, principal_id: &str, library_epoch: &str) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, session| session.expires_at > Instant::now());
        let Some(session) = sessions.get_mut(id) else {
            return false;
        };
        if session.principal_id != principal_id || session.library_epoch != library_epoch {
            sessions.remove(id);
            return false;
        }
        session.expires_at = Instant::now() + self.ttl;
        true
    }

    fn delete(&self, id: &str, principal_id: &str) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions
            .get(id)
            .is_some_and(|session| session.principal_id == principal_id)
        {
            sessions.remove(id);
            true
        } else {
            false
        }
    }
}

pub async fn post<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    Extension(context): Extension<RequestContext>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !accepts_json_and_sse(&headers) {
        return StatusCode::NOT_ACCEPTABLE.into_response();
    }
    if headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_none_or(|value| {
            value
                .split(';')
                .next()
                .is_none_or(|mime| !mime.trim().eq_ignore_ascii_case("application/json"))
        })
    {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return rpc_http(
            StatusCode::BAD_REQUEST,
            rpc_error(Value::Null, -32700, "Parse error"),
            None,
        );
    };
    let Some(object) = message.as_object() else {
        return rpc_http(
            StatusCode::BAD_REQUEST,
            rpc_error(Value::Null, -32600, "A single JSON-RPC object is required"),
            None,
        );
    };
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "jsonrpc" | "id" | "method" | "params" | "result" | "error"
        )
    }) {
        return rpc_http(
            StatusCode::BAD_REQUEST,
            rpc_error(Value::Null, -32600, "Unknown JSON-RPC fields"),
            None,
        );
    }
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return rpc_http(
            StatusCode::BAD_REQUEST,
            rpc_error(Value::Null, -32600, "Invalid Request"),
            None,
        );
    }
    let id = object.get("id").cloned();
    if id
        .as_ref()
        .is_some_and(|value| value.is_array() || value.is_object() || value.is_boolean())
    {
        return rpc_http(
            StatusCode::BAD_REQUEST,
            rpc_error(Value::Null, -32600, "Invalid request id"),
            None,
        );
    }
    let method = object.get("method").and_then(Value::as_str).unwrap_or("");
    if method == "initialize" {
        if id.is_none() {
            return rpc_http(
                StatusCode::BAD_REQUEST,
                rpc_error(Value::Null, -32600, "Initialize must be a request"),
                None,
            );
        }
        if headers.contains_key(SESSION_HEADER) {
            return StatusCode::BAD_REQUEST.into_response();
        }
        let requested = message
            .pointer("/params/protocolVersion")
            .and_then(Value::as_str);
        if requested != Some(MCP_PROTOCOL_VERSION) {
            return rpc_http(
                StatusCode::OK,
                rpc_error(
                    id.unwrap_or(Value::Null),
                    -32602,
                    "Unsupported protocol version",
                ),
                None,
            );
        }
        let Ok(runtime_status) = state.engine.status(&context) else {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };
        let Ok(session_id) = state
            .mcp_sessions
            .create(&context.principal_id, &runtime_status.library_epoch)
        else {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        };
        return rpc_http(
            StatusCode::OK,
            rpc_result(
                id.unwrap_or(Value::Null),
                json!({
                    "protocolVersion":MCP_PROTOCOL_VERSION,
                    "capabilities":{"tools":{"listChanged":false}},
                    "serverInfo":{"name":"gogglelab","version":env!("CARGO_PKG_VERSION")},
                    "instructions":"Use stable IDs and expected versions. Destructive operations require a separate confirmation token."
                }),
            ),
            Some(&session_id),
        );
    }

    let Some(session_id) = session_id(&headers) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(runtime_status) = state.engine.status(&context) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if !state.mcp_sessions.validate(
        session_id,
        &context.principal_id,
        &runtime_status.library_epoch,
    ) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if headers
        .get(VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        != Some(MCP_PROTOCOL_VERSION)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let notification = id.is_none();
    if matches!(
        method,
        "notifications/initialized" | "notifications/cancelled"
    ) {
        if method == "notifications/initialized" {
            state
                .mcp_sessions
                .initialize(session_id, &context.principal_id);
        }
        return StatusCode::ACCEPTED.into_response();
    }
    if method.is_empty()
        && id.is_some()
        && (object.contains_key("result") || object.contains_key("error"))
    {
        return StatusCode::ACCEPTED.into_response();
    }
    if notification {
        return StatusCode::ACCEPTED.into_response();
    }
    if !state.mcp_sessions.is_initialized(session_id) {
        return rpc_http(
            StatusCode::OK,
            rpc_error(
                id.unwrap_or(Value::Null),
                -32002,
                "Session is not initialized",
            ),
            Some(session_id),
        );
    }
    let id = id.unwrap_or(Value::Null);
    let response = match method {
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(
            id,
            json!({"tools":tool_definitions().into_iter().filter(|tool| {
                tool.get("name").and_then(Value::as_str).is_some_and(|name| state.engine.supports_tool(&context,name))
            }).collect::<Vec<_>>() }),
        ),
        "tools/call" => {
            let Some(name) = message.pointer("/params/name").and_then(Value::as_str) else {
                return rpc_http(
                    StatusCode::OK,
                    rpc_error(id, -32602, "Missing tool name"),
                    Some(session_id),
                );
            };
            if !crate::tool_names().contains(&name) || !state.engine.supports_tool(&context, name) {
                rpc_error(id, -32602, "Unknown tool")
            } else {
                let arguments = message
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if !arguments.is_object() {
                    rpc_error(id, -32602, "Tool arguments must be an object")
                } else if destructive_tool(name)
                    && arguments
                        .get("confirmationToken")
                        .and_then(Value::as_str)
                        .is_none_or(|token| token.is_empty() || token.len() > 256)
                {
                    rpc_error(id, -32602, "A bounded confirmationToken is required")
                } else if matches!(name, "set_thumbnail" | "add_thumbnail")
                    && validate_image_upload(&arguments).is_err()
                {
                    rpc_error(id, -32602, "Thumbnail image is invalid or oversized")
                } else {
                    let engine = std::sync::Arc::clone(&state.engine);
                    let call_context = context.clone();
                    let call_name = name.to_owned();
                    match tokio::task::spawn_blocking(move || {
                        engine.call_tool(&call_context, &call_name, arguments)
                    })
                    .await
                    {
                        Ok(Ok(value)) => rpc_result(
                            id,
                            json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value}),
                        ),
                        Ok(Err(error)) => rpc_result(
                            id,
                            json!({"isError":true,"content":[{"type":"text","text":error.message}],"structuredContent":{"error":{"code":error.code,"retryable":error.retryable}}}),
                        ),
                        Err(_) => rpc_result(
                            id,
                            json!({"isError":true,"content":[{"type":"text","text":"Tool execution stopped unexpectedly"}],"structuredContent":{"error":{"code":"internal","retryable":true}}}),
                        ),
                    }
                }
            }
        }
        _ => rpc_error(id, -32601, "Method not found"),
    };
    rpc_http(StatusCode::OK, response, Some(session_id))
}

fn destructive_tool(name: &str) -> bool {
    matches!(
        name,
        "remove_model" | "trash_file" | "undo_trash" | "move_files" | "restore_library"
    )
}

fn validate_image_upload(arguments: &Value) -> Result<(), ()> {
    use base64::Engine as _;
    let image = arguments.get("image").and_then(Value::as_str).ok_or(())?;
    if image.len() > 11_500_000 || image.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(());
    }
    let (mime, encoded) = if let Some(encoded) = image.strip_prefix("data:image/png;base64,") {
        ("image/png", encoded)
    } else if let Some(encoded) = image.strip_prefix("data:image/jpeg;base64,") {
        ("image/jpeg", encoded)
    } else {
        (
            arguments
                .get("mimeType")
                .and_then(Value::as_str)
                .ok_or(())?,
            image,
        )
    };
    if !matches!(mime, "image/png" | "image/jpeg") {
        return Err(());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| ())?;
    if bytes.is_empty() || bytes.len() > crate::MAX_THUMBNAIL_UPLOAD_BYTES {
        return Err(());
    }
    let magic_ok = match mime {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        _ => false,
    };
    magic_ok.then_some(()).ok_or(())
}

pub async fn get() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

pub async fn delete<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    Extension(context): Extension<RequestContext>,
    headers: HeaderMap,
) -> StatusCode {
    let Some(session_id) = session_id(&headers) else {
        return StatusCode::BAD_REQUEST;
    };
    if headers
        .get(VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        != Some(MCP_PROTOCOL_VERSION)
    {
        return StatusCode::BAD_REQUEST;
    }
    if state.mcp_sessions.delete(session_id, &context.principal_id) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

fn session_id(headers: &HeaderMap) -> Option<&str> {
    headers.get(SESSION_HEADER)?.to_str().ok().filter(|value| {
        !value.is_empty()
            && value.len() <= 128
            && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    })
}

fn accepts_json_and_sse(headers: &HeaderMap) -> bool {
    let Some(accept) = headers.get("accept").and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let media = accept
        .split(',')
        .filter_map(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    media.iter().any(|value| value == "application/json")
        && media.iter().any(|value| value == "text/event-stream")
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn rpc_http(status: StatusCode, body: Value, session_id: Option<&str>) -> Response {
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        VERSION_HEADER,
        HeaderValue::from_static(MCP_PROTOCOL_VERSION),
    );
    if let Some(session_id) = session_id.and_then(|value| HeaderValue::from_str(value).ok()) {
        response.headers_mut().insert(SESSION_HEADER, session_id);
    }
    response
}
