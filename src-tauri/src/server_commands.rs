use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use library_protocol::Capability;
use library_server::{
    AllowedAuthority, AllowedOrigin, HostMode, ProtocolEngineAdapter, SecurityConfig, ServerHandle,
};
use serde::Serialize;
use tauri::State;
use tokio::sync::Mutex;

use crate::extension_api::AppError;
use crate::library_commands::LibraryState;

const DEFAULT_DESKTOP_SERVER_PORT: u16 = 3745;

pub struct DesktopServerState {
    token_path: PathBuf,
    web_root: PathBuf,
    running: Mutex<Option<RunningServer>>,
}

struct RunningServer {
    handle: ServerHandle,
    address: SocketAddr,
}

impl DesktopServerState {
    pub fn new(token_path: PathBuf, web_root: PathBuf) -> Self {
        Self {
            token_path,
            web_root,
            running: Mutex::new(None),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopServerStatus {
    pub running: bool,
    pub base_url: Option<String>,
    pub mcp_url: Option<String>,
}

#[tauri::command]
pub async fn library_server_start(
    library: State<'_, LibraryState>,
    server: State<'_, DesktopServerState>,
    port: Option<u16>,
) -> Result<DesktopServerStatus, AppError> {
    start_impl(&library, &server, port).await
}

async fn start_impl(
    library: &LibraryState,
    server: &DesktopServerState,
    port: Option<u16>,
) -> Result<DesktopServerStatus, AppError> {
    let mut running = server.running.lock().await;
    if let Some(current) = running.as_ref() {
        return Ok(status_for(Some(current.address)));
    }
    let port = port.unwrap_or(DEFAULT_DESKTOP_SERVER_PORT);
    if port == 0 {
        return Err(server_error("Desktop server port must be nonzero"));
    }
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
            .await
            .map_err(|_| server_error("Desktop server loopback port is unavailable"))?;
    let address = listener
        .local_addr()
        .map_err(|_| server_error("Desktop server address is unavailable"))?;
    let authority = address.to_string();
    let origin = format!("http://{authority}");
    let bearer_token =
        library_server::load_or_create_bearer_secret(&server.token_path).map_err(server_error)?;
    let engine = Arc::new(ProtocolEngineAdapter::new(
        library.runtime().clone(),
        HostMode::Desktop,
    ));
    let app = library_server::router_with_static(
        engine,
        SecurityConfig {
            bearer_token,
            allowed_hosts: BTreeSet::from([
                AllowedAuthority::parse(&authority).map_err(server_error)?,
                AllowedAuthority::parse(&format!("localhost:{}", address.port()))
                    .map_err(server_error)?,
            ]),
            allowed_origins: BTreeSet::from([
                AllowedOrigin::parse(&origin).map_err(server_error)?,
                AllowedOrigin::parse(&format!("http://localhost:{}", address.port()))
                    .map_err(server_error)?,
            ]),
            principal_capabilities: desktop_capabilities(),
            max_in_flight: 64,
            max_mcp_sessions: 64,
            mcp_session_ttl: std::time::Duration::from_secs(15 * 60),
            trusted_proxy_peers: BTreeSet::new(),
            public_base_url: None,
            content_send_timeout: std::time::Duration::from_secs(5),
        },
        &server.web_root,
    )
    .map_err(server_error)?;
    let handle = library_server::start_listener(listener, app)
        .await
        .map_err(server_error)?;
    *running = Some(RunningServer { handle, address });
    Ok(status_for(Some(address)))
}

#[tauri::command]
pub async fn library_server_stop(
    server: State<'_, DesktopServerState>,
) -> Result<DesktopServerStatus, AppError> {
    stop_impl(&server).await
}

async fn stop_impl(server: &DesktopServerState) -> Result<DesktopServerStatus, AppError> {
    let current = server.running.lock().await.take();
    if let Some(current) = current {
        current.handle.stop().await.map_err(server_error)?;
    }
    Ok(status_for(None))
}

#[tauri::command]
pub async fn library_server_status(
    server: State<'_, DesktopServerState>,
) -> Result<DesktopServerStatus, AppError> {
    Ok(status_for(
        server
            .running
            .lock()
            .await
            .as_ref()
            .map(|value| value.address),
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessKeyReveal {
    pub access_key: String,
}
#[tauri::command]
pub fn library_server_reveal_access_key(
    server: State<'_, DesktopServerState>,
) -> Result<AccessKeyReveal, AppError> {
    let secret =
        library_server::load_or_create_bearer_secret(&server.token_path).map_err(server_error)?;
    Ok(AccessKeyReveal {
        access_key: secret.expose_to_operator(),
    })
}

fn desktop_capabilities() -> BTreeSet<Capability> {
    BTreeSet::from([
        Capability::CatalogRead,
        Capability::CatalogWrite,
        Capability::MediaRead,
        Capability::MediaWrite,
        Capability::Scan,
        Capability::Integration,
        Capability::Maintenance,
        Capability::PhysicalSourceWrite,
        Capability::HostPaths,
        Capability::HostSlicer,
    ])
}

fn status_for(address: Option<SocketAddr>) -> DesktopServerStatus {
    DesktopServerStatus {
        running: address.is_some(),
        base_url: address.map(|address| format!("http://{address}")),
        mcp_url: address.map(|address| format!("http://{address}/mcp")),
    }
}

fn server_error(message: impl Into<String>) -> AppError {
    AppError::Library {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_urls_are_loopback_and_never_include_credentials() {
        let address: SocketAddr = "127.0.0.1:3745".parse().unwrap();
        let status = status_for(Some(address));
        assert_eq!(status.base_url.as_deref(), Some("http://127.0.0.1:3745"));
        assert_eq!(status.mcp_url.as_deref(), Some("http://127.0.0.1:3745/mcp"));
        assert!(!status.base_url.unwrap().contains('@'));
    }
}
