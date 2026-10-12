use std::collections::BTreeSet;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use library_protocol::{Capability, RootAccess};
use library_runtime::{LibraryRuntime, RuntimeConfig, RuntimeRootPolicy};
use library_server::{
    AllowedAuthority, AllowedOrigin, HostMode, ProtocolEngineAdapter, SecurityConfig,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperatorConfig {
    library: PathBuf,
    web: PathBuf,
    token_file: PathBuf,
    integration_settings: PathBuf,
    #[serde(default)]
    allowed_roots: Vec<ConfiguredRoot>,
    #[serde(default)]
    allowed_hosts: Vec<String>,
    #[serde(default)]
    allowed_origins: Vec<String>,
    #[serde(default)]
    network_exposure: NetworkExposure,
    #[serde(default)]
    trusted_proxy_peers: BTreeSet<IpAddr>,
    public_base_url: Option<String>,
    #[serde(default = "default_capabilities")]
    capabilities: BTreeSet<Capability>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum NetworkExposure {
    #[default]
    Loopback,
    TlsProxy,
    ContainerLoopbackPublish,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfiguredRoot {
    id: String,
    label: String,
    path: PathBuf,
    access: RootAccess,
    #[serde(default)]
    catalog_root_id: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let arguments = Arguments::parse()?;
    let mut config = read_config(&arguments.config)?;
    if let Some(library) = arguments.library {
        config.library = library;
    }
    if let Some(web) = arguments.web {
        config.web = web;
    }
    if let Some(token_file) = arguments.token_file {
        config.token_file = token_file;
    }
    let listen = arguments
        .listen
        .unwrap_or_else(|| "127.0.0.1:3745".parse().expect("constant address"));
    validate_listen(
        listen,
        &config.allowed_hosts,
        &config.allowed_origins,
        config.network_exposure,
    )?;
    let public_base_url = match config.network_exposure {
        NetworkExposure::TlsProxy => Some(AllowedOrigin::parse(
            config
                .public_base_url
                .as_deref()
                .filter(|value| value.starts_with("https://"))
                .ok_or("tlsProxy requires HTTPS publicBaseUrl")?,
        )?),
        _ if config.public_base_url.is_some() => {
            return Err("publicBaseUrl is only valid for tlsProxy".into())
        }
        _ => None,
    };
    if matches!(config.network_exposure, NetworkExposure::TlsProxy)
        && config.trusted_proxy_peers.is_empty()
    {
        return Err("tlsProxy requires trustedProxyPeers".into());
    }
    if [
        &config.library,
        &config.web,
        &config.token_file,
        &config.integration_settings,
    ]
    .into_iter()
    .any(|path| !path.is_absolute())
    {
        return Err("Server paths must be absolute".into());
    }
    if config.token_file.starts_with(&config.library)
        || config.integration_settings.starts_with(&config.library)
    {
        return Err(
            "Server token and integration settings must be outside the Library catalog".into(),
        );
    }
    let web_overlaps_private_path = [
        &config.library,
        &config.token_file,
        &config.integration_settings,
        &arguments.config,
    ]
    .into_iter()
    .chain(config.allowed_roots.iter().map(|root| &root.path))
    .any(|private| paths_overlap(&config.web, private));
    if web_overlaps_private_path {
        return Err("Browser asset directory overlaps private server storage".into());
    }
    let integration_settings_path = config.integration_settings.clone();
    let runtime = LibraryRuntime::open(RuntimeConfig {
        library_root: config.library,
        integration_settings_path: integration_settings_path.clone(),
        local_catalog_root_access: false,
        roots: config
            .allowed_roots
            .into_iter()
            .map(|root| RuntimeRootPolicy {
                id: root.id,
                label: root.label,
                path: root.path,
                access: root.access,
                catalog_root_id: root.catalog_root_id,
            })
            .collect(),
        capabilities: config.capabilities.clone(),
    })
    .map_err(|error| error.to_string())?;
    runtime
        .install_shared_state(Arc::new(
            library_runtime::integration_state::IntegrationState::load(integration_settings_path)?,
        ))
        .map_err(|error| error.to_string())?;
    runtime
        .install_shared_state(Arc::new(
            library_runtime::integrations::RuntimeIntegrationTransport::default(),
        ))
        .map_err(|error| error.to_string())?;
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|_| "Server loopback address is unavailable")?;
    let address = listener
        .local_addr()
        .map_err(|_| "Server loopback address is unavailable")?;
    let authority = address.to_string();
    let hosts = if config.allowed_hosts.is_empty() {
        vec![authority.clone(), format!("localhost:{}", address.port())]
    } else {
        config.allowed_hosts
    };
    let origins = if config.allowed_origins.is_empty() {
        vec![
            format!("http://{authority}"),
            format!("http://localhost:{}", address.port()),
        ]
    } else {
        config.allowed_origins
    };
    if let Some(public) = config.public_base_url.as_deref() {
        let url = url::Url::parse(public).map_err(|_| "publicBaseUrl is invalid")?;
        let authority = url
            .host_str()
            .map(|host| match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_owned(),
            })
            .ok_or("publicBaseUrl has no host")?;
        if !origins.iter().any(|origin| origin == public)
            || !hosts
                .iter()
                .any(|host| host.eq_ignore_ascii_case(&authority))
        {
            return Err("publicBaseUrl must be included in allowedOrigins and its authority in allowedHosts".into());
        }
    }
    let display_url = config
        .public_base_url
        .clone()
        .unwrap_or_else(|| format!("http://{address}"));
    let bearer_token = library_server::load_or_create_bearer_secret(&config.token_file)?;
    let app = library_server::router_with_static(
        Arc::new(ProtocolEngineAdapter::new(
            Arc::clone(&runtime),
            HostMode::Headless,
        )),
        SecurityConfig {
            bearer_token,
            allowed_hosts: hosts
                .iter()
                .map(|host| AllowedAuthority::parse(host))
                .collect::<Result<_, _>>()?,
            allowed_origins: origins
                .iter()
                .map(|origin| AllowedOrigin::parse(origin))
                .collect::<Result<_, _>>()?,
            principal_capabilities: config.capabilities,
            max_in_flight: 64,
            max_mcp_sessions: 64,
            mcp_session_ttl: Duration::from_secs(15 * 60),
            trusted_proxy_peers: config.trusted_proxy_peers,
            public_base_url,
            content_send_timeout: Duration::from_secs(5),
        },
        &config.web,
    )?;
    library_runtime::scanner::start_background_scanner(&runtime);
    let server = library_server::start_listener(listener, app).await?;
    println!("GoggleLab server listening; public URL: {display_url}");
    tokio::signal::ctrl_c()
        .await
        .map_err(|_| "Shutdown signal failed")?;
    server.stop().await
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    if left.starts_with(right) || right.starts_with(left) {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left.starts_with(&right) || right.starts_with(&left),
        _ => false,
    }
}

fn read_config(path: &Path) -> Result<OperatorConfig, String> {
    if !path.is_absolute() {
        return Err("Server config path must be absolute".into());
    }
    let mut ancestors = path.ancestors().skip(1).collect::<Vec<_>>();
    ancestors.reverse();
    for ancestor in ancestors {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err("Server config directory ancestry is unsafe".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("Server config directory could not be inspected".into()),
        }
    }
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| "Server config could not be inspected")?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err("Server config is unsafe or oversized".into());
    }
    let bytes = std::fs::read(path).map_err(|_| "Server config could not be read")?;
    serde_json::from_slice(&bytes).map_err(|_| "Server config is malformed".into())
}

fn default_capabilities() -> BTreeSet<Capability> {
    BTreeSet::from([Capability::CatalogRead, Capability::MediaRead])
}

struct Arguments {
    config: PathBuf,
    library: Option<PathBuf>,
    web: Option<PathBuf>,
    listen: Option<SocketAddr>,
    token_file: Option<PathBuf>,
}

impl Arguments {
    fn parse() -> Result<Self, String> {
        let mut values = std::env::args_os().skip(1);
        let mut result = Self {
            config: PathBuf::new(),
            library: None,
            web: None,
            listen: None,
            token_file: None,
        };
        while let Some(flag) = values.next() {
            let flag = flag.to_string_lossy();
            let value = values
                .next()
                .ok_or_else(|| format!("Missing value for {flag}"))?;
            match flag.as_ref() {
                "--config" => result.config = PathBuf::from(value),
                "--library" => result.library = Some(PathBuf::from(value)),
                "--web" => result.web = Some(PathBuf::from(value)),
                "--listen" => {
                    result.listen = Some(
                        value
                            .to_string_lossy()
                            .parse()
                            .map_err(|_| "Listen address is invalid")?,
                    )
                }
                "--token-file" => result.token_file = Some(PathBuf::from(value)),
                _ => return Err(format!("Unknown argument: {flag}")),
            }
        }
        if result.config.as_os_str().is_empty() {
            return Err("--config is required".into());
        }
        Ok(result)
    }
}

fn validate_listen(
    listen: SocketAddr,
    allowed_hosts: &[String],
    allowed_origins: &[String],
    exposure: NetworkExposure,
) -> Result<(), String> {
    if !listen.ip().is_loopback()
        && (matches!(exposure, NetworkExposure::Loopback)
            || allowed_hosts.is_empty()
            || allowed_origins.is_empty())
    {
        return Err("Non-loopback listen requires explicit allowedHosts and allowedOrigins".into());
    }
    for host in allowed_hosts {
        AllowedAuthority::parse(host)?;
    }
    for origin in allowed_origins {
        AllowedOrigin::parse(origin)?;
        if matches!(exposure, NetworkExposure::TlsProxy) && !origin.starts_with("https://") {
            return Err("TLS proxy exposure requires HTTPS allowedOrigins".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_loopback_requires_explicit_valid_host_and_origin_allowlists() {
        let lan: SocketAddr = "0.0.0.0:3745".parse().unwrap();
        assert!(validate_listen(lan, &[], &[], NetworkExposure::Loopback).is_err());
        assert!(validate_listen(
            lan,
            &["nas.local:3745".into()],
            &[],
            NetworkExposure::TlsProxy
        )
        .is_err());
        let loopback: SocketAddr = "127.0.0.1:3745".parse().unwrap();
        assert!(validate_listen(
            loopback,
            &["nas.local:3745".into()],
            &["http://nas.local:3745".into()],
            NetworkExposure::TlsProxy
        )
        .is_err());
        assert!(validate_listen(
            lan,
            &["nas.local:3745".into()],
            &["https://nas.local:3745".into()],
            NetworkExposure::TlsProxy
        )
        .is_ok());
        assert!(validate_listen(
            lan,
            &["nas.local:3745/path".into()],
            &["https://nas.local:3745".into()],
            NetworkExposure::TlsProxy
        )
        .is_err());
    }

    #[test]
    fn web_assets_must_not_overlap_private_storage() {
        let base = Path::new("/srv/gogglelab");
        assert!(paths_overlap(base, &base.join("server-token")));
        assert!(paths_overlap(&base.join("web"), base));
        assert!(!paths_overlap(&base.join("web"), Path::new("/srv/catalog")));
    }
}
