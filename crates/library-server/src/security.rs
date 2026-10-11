use std::collections::BTreeSet;
use std::net::IpAddr;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{LibraryEngine, RequestContext, ServerState};

#[derive(Clone)]
pub struct SecurityConfig {
    pub bearer_token: crate::BearerSecret,
    pub allowed_hosts: BTreeSet<AllowedAuthority>,
    pub allowed_origins: BTreeSet<AllowedOrigin>,
    pub principal_capabilities: BTreeSet<crate::Capability>,
    pub max_in_flight: usize,
    pub max_mcp_sessions: usize,
    pub mcp_session_ttl: Duration,
    pub trusted_proxy_peers: BTreeSet<IpAddr>,
    pub public_base_url: Option<AllowedOrigin>,
    pub content_send_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AllowedAuthority(String);

impl AllowedAuthority {
    pub fn parse(value: &str) -> Result<Self, String> {
        let authority = value
            .trim()
            .parse::<http::uri::Authority>()
            .map_err(|_| "Allowed Host authority is invalid")?;
        Ok(Self(authority.as_str().to_ascii_lowercase()))
    }

    fn matches(&self, value: &str) -> bool {
        self.0 == value.trim().to_ascii_lowercase()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AllowedOrigin(String);

impl AllowedOrigin {
    pub fn parse(value: &str) -> Result<Self, String> {
        let url = url::Url::parse(value).map_err(|_| "Allowed Origin is invalid")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !matches!(url.path(), "" | "/")
            || url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Allowed Origin must be an HTTP origin".into());
        }
        Ok(Self(url.origin().ascii_serialization()))
    }

    fn matches(&self, value: &str) -> bool {
        url::Url::parse(value).ok().is_some_and(|value| {
            self.0 == value.origin().ascii_serialization()
                && value.username().is_empty()
                && value.password().is_none()
                && matches!(value.path(), "" | "/")
                && value.query().is_none()
                && value.fragment().is_none()
        })
    }
}

pub async fn admit<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    mut request: Request,
    next: Next,
) -> Response {
    let request_id = Uuid::new_v4().simple().to_string();
    if state.security.public_base_url.is_some() {
        let peer = request
            .extensions()
            .get::<axum::extract::connect_info::ConnectInfo<std::net::SocketAddr>>()
            .map(|value| value.0.ip());
        if peer.is_none_or(|peer| !state.security.trusted_proxy_peers.contains(&peer)) {
            return harden(
                rejection(StatusCode::FORBIDDEN, "proxyPeerRejected", &request_id),
                &request_id,
                false,
            );
        }
    }
    let host_values = request.headers().get_all(header::HOST);
    let host = (host_values.iter().count() == 1)
        .then(|| host_values.iter().next())
        .flatten()
        .and_then(|value| value.to_str().ok());
    if host.is_none_or(|host| {
        !state
            .security
            .allowed_hosts
            .iter()
            .any(|allowed| allowed.matches(host))
    }) {
        return harden(
            rejection(StatusCode::FORBIDDEN, "hostRejected", &request_id),
            &request_id,
            false,
        );
    }
    let origins = request.headers().get_all(header::ORIGIN);
    if origins.iter().count() > 1 {
        return harden(
            rejection(StatusCode::FORBIDDEN, "originRejected", &request_id),
            &request_id,
            false,
        );
    }
    if let Some(origin) = origins.iter().next() {
        let Ok(origin) = origin.to_str() else {
            return harden(
                rejection(StatusCode::FORBIDDEN, "originRejected", &request_id),
                &request_id,
                false,
            );
        };
        if !state
            .security
            .allowed_origins
            .iter()
            .any(|allowed| allowed.matches(origin))
        {
            return harden(
                rejection(StatusCode::FORBIDDEN, "originRejected", &request_id),
                &request_id,
                false,
            );
        }
    }
    let path = request.uri().path();
    let health = path == "/api/v1/health";
    let static_asset = !path.starts_with("/api/") && path != "/mcp";
    if !health && !static_asset {
        let authorization = request.headers().get_all(header::AUTHORIZATION);
        let authorized = (authorization.iter().count() == 1)
            .then(|| authorization.iter().next())
            .flatten()
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|candidate| state.security.bearer_token.matches(candidate));
        if !authorized {
            return harden(
                rejection(
                    StatusCode::UNAUTHORIZED,
                    "authenticationRequired",
                    &request_id,
                ),
                &request_id,
                false,
            );
        }
    }
    let Ok(permit) = state.admission.clone().try_acquire_owned() else {
        let mut response = rejection(StatusCode::TOO_MANY_REQUESTS, "busy", &request_id);
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        return harden(response, &request_id, false);
    };
    request.extensions_mut().insert(RequestContext {
        principal_id: if health || static_asset {
            "anonymous"
        } else {
            "operator"
        }
        .into(),
        capabilities: if health || static_asset {
            BTreeSet::new()
        } else {
            state.security.principal_capabilities.clone()
        },
        request_id: request_id.clone(),
    });
    let response = next.run(request).await;
    drop(permit);
    harden(response, &request_id, static_asset)
}

fn harden(mut response: Response, request_id: &str, static_asset: bool) -> Response {
    response.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(request_id).unwrap_or_else(|_| HeaderValue::from_static("invalid")),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
        .headers_mut()
        .insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "content-security-policy",
        if static_asset {
            HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; connect-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'none'")
        } else {
            HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'; base-uri 'none'")
        },
    );
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}

fn rejection(status: StatusCode, code: &str, request_id: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({
            "error": {
                "code": code,
                "message": "Request admission failed",
                "requestId": request_id,
                "retryable": status == StatusCode::TOO_MANY_REQUESTS
            }
        })),
    )
        .into_response()
}

impl crate::BearerSecret {
    pub(crate) fn matches(&self, candidate: &str) -> bool {
        let expected = self.as_bytes();
        let candidate = candidate.as_bytes();
        expected.len() == candidate.len() && expected.ct_eq(candidate).into()
    }
}
