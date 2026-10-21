use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use library_protocol::{
    Capability, CatalogEngine, ContentEngine, DownloadEngine, RequestContext, VersionedTarget,
};
use library_server::{
    AllowedAuthority, AllowedOrigin, EngineError, HostMode, HostSlicerAdapter, HostSlicerSummary,
    ProtocolEngineAdapter, SecurityConfig, ServerHandle,
};
use serde::{Deserialize, Serialize};
use tauri::State;
use tokio::sync::Mutex;

use crate::extension_api::{AppError, SlicerApp, SourceFormat};
use crate::library_commands::LibraryState;
use crate::settings::SettingsState;

type SlicerDetector = dyn Fn() -> Vec<SlicerApp> + Send + Sync;
type SlicerLauncher = dyn Fn(&SlicerApp, &Path) -> Result<(), AppError> + Send + Sync;
type SlicerBatchLauncher = dyn Fn(&SlicerApp, &[PathBuf]) -> Result<(), AppError> + Send + Sync;

const MAX_SLICER_BATCH_MODELS: usize = 100;
const MAX_SLICER_BATCH_BYTES: u64 = 256 * 1024 * 1024;

struct DesktopHostSlicerAdapter {
    runtime: Arc<library_runtime::LibraryRuntime>,
    detect: Arc<SlicerDetector>,
    launch: Arc<SlicerLauncher>,
    launch_many: Arc<SlicerBatchLauncher>,
}

impl DesktopHostSlicerAdapter {
    #[cfg(test)]
    fn with_host_ops(
        runtime: Arc<library_runtime::LibraryRuntime>,
        detect: impl Fn() -> Vec<SlicerApp> + Send + Sync + 'static,
        launch: impl Fn(&SlicerApp, &Path) -> Result<(), AppError> + Send + Sync + 'static,
    ) -> Self {
        let launch: Arc<SlicerLauncher> = Arc::new(launch);
        let launch_many_single = Arc::clone(&launch);
        let launch_many: Arc<SlicerBatchLauncher> = Arc::new(move |slicer, paths| {
            for path in paths {
                launch_many_single(slicer, path)?;
            }
            Ok(())
        });
        Self {
            runtime,
            detect: Arc::new(detect),
            launch,
            launch_many,
        }
    }

    fn with_host_batch_ops(
        runtime: Arc<library_runtime::LibraryRuntime>,
        detect: impl Fn() -> Vec<SlicerApp> + Send + Sync + 'static,
        launch: impl Fn(&SlicerApp, &Path) -> Result<(), AppError> + Send + Sync + 'static,
        launch_many: impl Fn(&SlicerApp, &[PathBuf]) -> Result<(), AppError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            runtime,
            detect: Arc::new(detect),
            launch: Arc::new(launch),
            launch_many: Arc::new(launch_many),
        }
    }
}

impl HostSlicerAdapter for DesktopHostSlicerAdapter {
    fn list_slicers(
        &self,
        context: &RequestContext,
    ) -> Result<Vec<HostSlicerSummary>, EngineError> {
        if !context.capabilities.contains(&Capability::CatalogRead)
            || !context.capabilities.contains(&Capability::HostSlicer)
        {
            return Err(host_slicer_error(
                "forbidden",
                "Host slicer access is not allowed",
            ));
        }
        Ok((self.detect)()
            .into_iter()
            .map(|slicer| HostSlicerSummary {
                id: slicer.id,
                display_name: slicer.display_name,
            })
            .collect())
    }

    fn open_in_slicer(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        slicer_id: &str,
    ) -> Result<(), EngineError> {
        if !context.capabilities.contains(&Capability::CatalogRead)
            || !context.capabilities.contains(&Capability::MediaRead)
            || !context.capabilities.contains(&Capability::HostSlicer)
        {
            return Err(host_slicer_error(
                "forbidden",
                "Host slicer access is not allowed",
            ));
        }
        let model = self
            .runtime
            .get_model(context, model_id)
            .map_err(|_| host_slicer_error("notFound", "Model is not available for handoff"))?;
        if model.metadata_version != expected_version {
            return Err(EngineError {
                code: "conflict",
                message: "Model metadata changed; refresh the model before opening it".into(),
                retryable: true,
                current_version: Some(model.metadata_version),
                source_holds: Vec::new(),
            });
        }
        let lease = self
            .runtime
            .issue_model_download(context, model_id, model.content_hash.as_deref())
            .map_err(|_| host_slicer_error("conflict", "Model source is unavailable or changed"))?;
        if lease.bytes > stl_core::limits::MAX_FILE_BYTES {
            return Err(host_slicer_error(
                "invalidInput",
                "Model exceeds the slicer handoff size limit",
            ));
        }
        let capacity = usize::try_from(lease.bytes)
            .map_err(|_| host_slicer_error("invalidInput", "Model size cannot be represented"))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|_| host_slicer_error("io", "Model could not be staged for slicer handoff"))?;
        self.runtime
            .stream_content(context, &lease.id, &mut bytes)
            .map_err(|_| host_slicer_error("io", "Verified model bytes could not be read"))?;
        let format = match lease.mime.as_str() {
            "model/stl" => SourceFormat::Stl,
            "model/3mf" => SourceFormat::ThreeMf,
            _ => {
                return Err(host_slicer_error(
                    "invalidInput",
                    "Model format is not supported by slicers",
                ))
            }
        };
        crate::handoff::checked_bytes_launch(
            crate::handoff::VerifiedHandoffRequest {
                bytes: &bytes,
                file_name: &lease.file_name,
                expected_digest: &lease.sha256,
                format,
                library_root: self.runtime.host_library_root(),
                slicer_id,
            },
            || (self.detect)(),
            |slicer, path| (self.launch)(slicer, path),
        )
        .map(|_| ())
        .map_err(|_| {
            host_slicer_error(
                "io",
                "Verified model could not be opened in the selected slicer",
            )
        })
    }

    fn open_in_slicer_batch(
        &self,
        context: &RequestContext,
        targets: &[VersionedTarget],
        slicer_id: &str,
    ) -> Result<(), EngineError> {
        if !context.capabilities.contains(&Capability::CatalogRead)
            || !context.capabilities.contains(&Capability::MediaRead)
            || !context.capabilities.contains(&Capability::HostSlicer)
        {
            return Err(host_slicer_error(
                "forbidden",
                "Host slicer access is not allowed",
            ));
        }
        if targets.is_empty() || targets.len() > MAX_SLICER_BATCH_MODELS {
            return Err(host_slicer_error(
                "invalidInput",
                "Slicer handoff target count is invalid",
            ));
        }
        if !crate::handoff::is_supported_slicer(slicer_id, &(self.detect)()) {
            return Err(host_slicer_error(
                "invalidInput",
                "Selected slicer is not installed or supported",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut total_bytes = 0_u64;
        let mut prepared: Vec<(library_protocol::ContentLease, SourceFormat)> =
            Vec::with_capacity(targets.len());
        for target in targets {
            if !seen.insert(target.model_id.as_str()) {
                return Err(host_slicer_error(
                    "invalidInput",
                    "Slicer handoff targets must be unique",
                ));
            }
            let model = self
                .runtime
                .get_model(context, &target.model_id)
                .map_err(|_| host_slicer_error("notFound", "Model is not available for handoff"))?;
            if model.metadata_version != target.expected_version {
                return Err(EngineError {
                    code: "conflict",
                    message: "Model metadata changed; refresh the model before opening it".into(),
                    retryable: true,
                    current_version: Some(model.metadata_version),
                    source_holds: Vec::new(),
                });
            }
            let lease = self
                .runtime
                .issue_model_download(context, &target.model_id, model.content_hash.as_deref())
                .map_err(|_| {
                    host_slicer_error("conflict", "Model source is unavailable or changed")
                })?;
            if lease.bytes > stl_core::limits::MAX_FILE_BYTES {
                return Err(host_slicer_error(
                    "invalidInput",
                    "Model exceeds the slicer handoff size limit",
                ));
            }
            total_bytes = total_bytes.checked_add(lease.bytes).ok_or_else(|| {
                host_slicer_error("invalidInput", "Slicer handoff byte total is out of range")
            })?;
            if total_bytes > MAX_SLICER_BATCH_BYTES {
                return Err(host_slicer_error(
                    "invalidInput",
                    "Slicer handoff batch exceeds its byte limit",
                ));
            }
            let format = match lease.mime.as_str() {
                "model/stl" => SourceFormat::Stl,
                "model/3mf" => SourceFormat::ThreeMf,
                _ => {
                    return Err(host_slicer_error(
                        "invalidInput",
                        "Model format is not supported by slicers",
                    ))
                }
            };
            prepared.push((lease, format));
        }
        let requests = prepared
            .iter()
            .map(|(lease, format)| crate::handoff::StreamHandoffRequest {
                content_id: &lease.id,
                file_name: &lease.file_name,
                expected_digest: &lease.sha256,
                expected_bytes: lease.bytes,
                format: *format,
                library_root: self.runtime.host_library_root(),
                slicer_id,
            })
            .collect::<Vec<_>>();
        let launch_many = Arc::clone(&self.launch_many);
        crate::handoff::checked_stream_batch_launch(
            &requests,
            |lease_id, sink| {
                self.runtime
                    .stream_content(context, lease_id, sink)
                    .map_err(|_| AppError::Library {
                        message: "Verified model bytes could not be read".into(),
                    })
            },
            || (self.detect)(),
            move |slicer, paths| launch_many(slicer, paths),
        )
        .map(|_| ())
        .map_err(|_| {
            host_slicer_error(
                "io",
                "Verified models could not be opened in the selected slicer",
            )
        })
    }
}

fn host_slicer_error(code: &'static str, message: &str) -> EngineError {
    EngineError {
        code,
        message: message.into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    }
}

const DEFAULT_DESKTOP_SERVER_PORT: u16 = 3745;

pub struct DesktopServerState {
    token_path: PathBuf,
    web_root: PathBuf,
    slicer_settings: SettingsState,
    running: Mutex<Option<RunningServer>>,
}

struct RunningServer {
    handle: ServerHandle,
    address: SocketAddr,
    allow_external_integrations: bool,
    integration_transport: Arc<library_runtime::integrations::RuntimeIntegrationTransport>,
    printer_network: Option<DesktopPrinterNetworkPolicy>,
    printer_runtime: Arc<library_runtime::LibraryRuntime>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopPrinterNetworkPolicy {
    allowed_scopes: Vec<String>,
    #[serde(default)]
    allow_multicast_discovery: bool,
    #[serde(default)]
    allow_carbon_broadcast: bool,
}

pub(crate) fn prepare_printer_network(
    policy: Option<&DesktopPrinterNetworkPolicy>,
) -> Result<Option<library_runtime::printer_network_api::PrinterNetworkPolicy>, AppError> {
    policy
        .map(|policy| {
            crate::visual_qa::require_printer_network()?;
            library_runtime::printer_network_api::PrinterNetworkPolicy::new(
                policy.allowed_scopes.clone(),
                policy.allow_multicast_discovery,
                policy.allow_carbon_broadcast,
            )
            .map_err(|error| server_error(error.to_string()))
        })
        .transpose()
}

impl DesktopServerState {
    pub fn new(token_path: PathBuf, web_root: PathBuf, slicer_settings: SettingsState) -> Self {
        Self {
            token_path,
            web_root,
            slicer_settings,
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
    pub allow_external_integrations: bool,
    pub printer_network: Option<DesktopPrinterNetworkPolicy>,
}

#[tauri::command]
pub async fn library_server_start(
    library: State<'_, LibraryState>,
    server: State<'_, DesktopServerState>,
    port: Option<u16>,
    allow_external_integrations: Option<bool>,
    printer_network: Option<DesktopPrinterNetworkPolicy>,
) -> Result<DesktopServerStatus, AppError> {
    start_impl(
        &library,
        &server,
        port,
        allow_external_integrations.unwrap_or(false),
        printer_network,
    )
    .await
}

async fn start_impl(
    library: &LibraryState,
    server: &DesktopServerState,
    port: Option<u16>,
    allow_external_integrations: bool,
    printer_network: Option<DesktopPrinterNetworkPolicy>,
) -> Result<DesktopServerStatus, AppError> {
    // This operator setting validates without I/O. Isolated QA cannot install it.
    let prepared_printer_network = prepare_printer_network(printer_network.as_ref())?;
    let mut running = server.running.lock().await;
    if let Some(current) = running.as_mut() {
        current
            .printer_runtime
            .configure_printer_network(prepared_printer_network)
            .map_err(|error| server_error(error.to_string()))?;
        current
            .integration_transport
            .set_allow_network(allow_external_integrations);
        current.allow_external_integrations = allow_external_integrations;
        current.printer_network = printer_network;
        return Ok(status_with_printer_policy(
            Some(current.address),
            current.allow_external_integrations,
            current.printer_network.clone(),
        ));
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
    let engine = Arc::new(
        ProtocolEngineAdapter::new(Arc::clone(library.runtime()), HostMode::Desktop)
            .with_host_slicer(Arc::new(DesktopHostSlicerAdapter::with_host_batch_ops(
                Arc::clone(library.runtime()),
                {
                    let settings = server.slicer_settings.clone();
                    move || {
                        let settings = settings.0.lock().unwrap();
                        crate::slicers::available_slicers(&settings)
                    }
                },
                {
                    let settings = server.slicer_settings.clone();
                    move |slicer, path| {
                        crate::slicers::launch_with_current_profile(&settings, slicer, || {
                            crate::handoff::launch_paths(slicer, &[path.to_path_buf()])
                        })
                    }
                },
                {
                    let settings = server.slicer_settings.clone();
                    move |slicer, paths| {
                        crate::slicers::launch_with_current_profile(&settings, slicer, || {
                            crate::handoff::launch_paths(slicer, paths)
                        })
                    }
                },
            ))),
    );
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
    let integration_transport = library
        .runtime()
        .shared_state::<library_runtime::integrations::RuntimeIntegrationTransport>()
        .ok_or_else(|| server_error("Integration transport state is unavailable"))?;
    integration_transport.set_allow_network(allow_external_integrations);
    let handle = match library_server::start_listener(listener, app).await {
        Ok(handle) => handle,
        Err(error) => {
            integration_transport.set_allow_network(false);
            return Err(server_error(error));
        }
    };
    if let Err(error) = library
        .runtime()
        .configure_printer_network(prepared_printer_network)
    {
        integration_transport.set_allow_network(false);
        let _ = handle.stop().await;
        return Err(server_error(error.to_string()));
    }
    let status = status_with_printer_policy(
        Some(address),
        allow_external_integrations,
        printer_network.clone(),
    );
    *running = Some(RunningServer {
        handle,
        address,
        allow_external_integrations,
        integration_transport,
        printer_network,
        printer_runtime: Arc::clone(library.runtime()),
    });
    Ok(status)
}

#[tauri::command]
pub async fn library_server_stop(
    server: State<'_, DesktopServerState>,
) -> Result<DesktopServerStatus, AppError> {
    stop_impl(&server).await
}

async fn stop_impl(server: &DesktopServerState) -> Result<DesktopServerStatus, AppError> {
    // Serialize stop with a new start, including revocation and listener shutdown.
    let mut running = server.running.lock().await;
    let current = running.take();
    if let Some(current) = current {
        current.integration_transport.set_allow_network(false);
        let revoked = current.printer_runtime.configure_printer_network(None);
        let stopped = current.handle.stop().await;
        revoked.map_err(|error| server_error(error.to_string()))?;
        stopped.map_err(server_error)?;
    }
    Ok(status_for(None, false))
}

#[tauri::command]
pub async fn library_server_status(
    server: State<'_, DesktopServerState>,
) -> Result<DesktopServerStatus, AppError> {
    let running = server.running.lock().await;
    Ok(match running.as_ref() {
        Some(value) => status_with_printer_policy(
            Some(value.address),
            value.allow_external_integrations,
            value.printer_network.clone(),
        ),
        None => status_for(None, false),
    })
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
        Capability::HostPaths,
        Capability::HostSlicer,
        Capability::PrinterNetwork,
    ])
}

fn status_for(
    address: Option<SocketAddr>,
    allow_external_integrations: bool,
) -> DesktopServerStatus {
    status_with_printer_policy(address, allow_external_integrations, None)
}

fn status_with_printer_policy(
    address: Option<SocketAddr>,
    allow_external_integrations: bool,
    printer_network: Option<DesktopPrinterNetworkPolicy>,
) -> DesktopServerStatus {
    let running = address.is_some();
    DesktopServerStatus {
        running,
        base_url: address.map(|address| format!("http://{address}")),
        mcp_url: address.map(|address| format!("http://{address}/mcp")),
        allow_external_integrations: running && allow_external_integrations,
        printer_network: if running { printer_network } else { None },
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
    use std::fs;
    use std::sync::Mutex;

    #[test]
    fn status_urls_are_loopback_and_never_include_credentials() {
        let address: SocketAddr = "127.0.0.1:3745".parse().unwrap();
        let status = status_for(Some(address), false);
        assert_eq!(status.base_url.as_deref(), Some("http://127.0.0.1:3745"));
        assert_eq!(status.mcp_url.as_deref(), Some("http://127.0.0.1:3745/mcp"));
        assert!(!status.base_url.unwrap().contains('@'));
    }

    #[test]
    fn stopped_server_status_never_keeps_external_integration_access_enabled() {
        let address: SocketAddr = "127.0.0.1:3745".parse().unwrap();
        assert!(status_for(Some(address), true).allow_external_integrations);
        assert!(!status_for(None, true).allow_external_integrations);
    }

    #[test]
    fn stopped_server_explicitly_reports_printer_network_access_off() {
        let json = serde_json::to_value(status_for(None, false)).unwrap();
        assert_eq!(json.get("printerNetwork"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn printer_policy_datagram_defaults_are_independent_and_stop_discards_policy() {
        let mut value = serde_json::json!({"allowedScopes":["10.250.0.1/32"]});
        let policy: DesktopPrinterNetworkPolicy = serde_json::from_value(value.clone()).unwrap();
        assert!(!policy.allow_multicast_discovery);
        assert!(!policy.allow_carbon_broadcast);
        assert!(prepare_printer_network(None).unwrap().is_none());
        let address: SocketAddr = "127.0.0.1:3745".parse().unwrap();
        assert!(
            status_with_printer_policy(Some(address), false, Some(policy.clone()))
                .printer_network
                .is_some()
        );
        assert!(status_with_printer_policy(None, true, Some(policy))
            .printer_network
            .is_none());
        value["allowMulticastDiscovery"] = serde_json::json!(true);
        let policy: DesktopPrinterNetworkPolicy = serde_json::from_value(value.clone()).unwrap();
        assert!(policy.allow_multicast_discovery);
        assert!(!policy.allow_carbon_broadcast);
        value["allowBroadcast"] = serde_json::json!(true);
        assert!(serde_json::from_value::<DesktopPrinterNetworkPolicy>(value).is_err());
    }

    #[test]
    fn desktop_server_does_not_grant_remote_physical_source_writes_by_default() {
        let capabilities = desktop_capabilities();
        assert!(capabilities.contains(&Capability::CatalogRead));
        assert!(capabilities.contains(&Capability::CatalogWrite));
        assert!(!capabilities.contains(&Capability::PhysicalSourceWrite));
    }

    #[test]
    fn desktop_host_slicer_uses_a_verified_managed_copy_and_checks_model_version() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        let settings_path = temp.path().join("integrations.json");
        let runtime = library_runtime::LibraryRuntime::open(
            library_runtime::RuntimeConfig::desktop(library_root.clone(), settings_path),
        )
        .unwrap();
        let source = temp.path().join("fixture.stl");
        let source_bytes = b"solid fixture\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid fixture\n";
        fs::write(&source, source_bytes).unwrap();
        let model = runtime
            .mutate_db(|db| {
                db.import_file(&source, None, None)
                    .map(|result| result.model)
            })
            .unwrap();
        let launched = Arc::new(Mutex::new(None));
        let launch_result = Arc::clone(&launched);
        let adapter = DesktopHostSlicerAdapter::with_host_ops(
            Arc::clone(&runtime),
            || {
                vec![SlicerApp {
                    id: "com.bambulab.bambu-studio".into(),
                    bundle_id: "com.bambulab.bambu-studio".into(),
                    display_name: "Bambu Studio".into(),
                    app_path: "/fixture/Bambu Studio.app".into(),
                    custom: false,
                    profile_identity: None,
                }]
            },
            move |slicer, path| {
                assert_eq!(slicer.id, "com.bambulab.bambu-studio");
                *launch_result.lock().unwrap() =
                    Some((path.to_path_buf(), fs::read(path).unwrap()));
                Ok(())
            },
        );
        let context = RequestContext {
            principal_id: "fixture".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::MediaRead,
                Capability::HostSlicer,
            ]),
            request_id: "open-in-slicer".into(),
        };

        adapter
            .open_in_slicer(
                &context,
                &model.id,
                model.metadata_version,
                "com.bambulab.bambu-studio",
            )
            .unwrap();
        let (staged_path, staged_bytes) = launched.lock().unwrap().clone().unwrap();
        assert_eq!(staged_bytes, source_bytes);
        assert_ne!(staged_path, source);
        assert!(staged_path.starts_with(
            crate::handoff::stable_handoff_root(runtime.host_library_root()).unwrap()
        ));
        assert_eq!(fs::read(&source).unwrap(), source_bytes);

        let stale = adapter.open_in_slicer(
            &context,
            &model.id,
            model.metadata_version + 1,
            "com.bambulab.bambu-studio",
        );
        assert!(stale.is_err());
        assert_eq!(launched.lock().unwrap().as_ref().unwrap().0, staged_path);
    }

    #[test]
    fn desktop_host_slicer_preflights_and_launches_a_verified_batch_once() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        let runtime =
            library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
                library_root.clone(),
                temp.path().join("integrations.json"),
            ))
            .unwrap();
        fn triangle_stl(z: f32) -> Vec<u8> {
            let mut bytes = vec![0_u8; 80];
            bytes.extend_from_slice(&1_u32.to_le_bytes());
            for value in [0.0_f32, 0.0, z, 0.0, 0.0, 1.0, 1.0, 0.0, z, 0.0, 1.0, z] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&[0, 0]);
            bytes
        }
        let first_path = temp.path().join("first.stl");
        let second_path = temp.path().join("second.stl");
        let first_bytes = triangle_stl(0.0);
        let second_bytes = triangle_stl(1.0);
        fs::write(&first_path, &first_bytes).unwrap();
        fs::write(&second_path, &second_bytes).unwrap();
        let first = runtime
            .mutate_db(|db| {
                db.import_file(&first_path, None, None)
                    .map(|result| result.model)
            })
            .unwrap();
        let second = runtime
            .mutate_db(|db| {
                db.import_file(&second_path, None, None)
                    .map(|result| result.model)
            })
            .unwrap();
        let launches = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&launches);
        let adapter = DesktopHostSlicerAdapter::with_host_batch_ops(
            Arc::clone(&runtime),
            || {
                vec![SlicerApp {
                    id: "com.bambulab.bambu-studio".into(),
                    bundle_id: "com.bambulab.bambu-studio".into(),
                    display_name: "Bambu Studio".into(),
                    app_path: "/fixture/Bambu Studio.app".into(),
                    custom: false,
                    profile_identity: None,
                }]
            },
            |_, _| panic!("batch handoff should launch one path set"),
            move |slicer, paths| {
                assert_eq!(slicer.id, "com.bambulab.bambu-studio");
                recorded.lock().unwrap().push((
                    paths.to_vec(),
                    paths
                        .iter()
                        .map(|path| fs::read(path).unwrap())
                        .collect::<Vec<_>>(),
                ));
                Ok(())
            },
        );
        let context = RequestContext {
            principal_id: "fixture".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::MediaRead,
                Capability::HostSlicer,
            ]),
            request_id: "open-in-slicer-batch".into(),
        };
        let targets = [
            VersionedTarget {
                model_id: first.id.clone(),
                expected_version: first.metadata_version,
            },
            VersionedTarget {
                model_id: second.id.clone(),
                expected_version: second.metadata_version,
            },
        ];

        adapter
            .open_in_slicer_batch(&context, &targets, "com.bambulab.bambu-studio")
            .unwrap();
        let launched = launches.lock().unwrap().clone();
        assert_eq!(launched.len(), 1);
        assert_eq!(launched[0].0.len(), 2);
        assert_eq!(
            launched[0].1,
            vec![first_bytes.to_vec(), second_bytes.to_vec()]
        );
        let handoff_root =
            crate::handoff::stable_handoff_root(runtime.host_library_root()).unwrap();
        assert!(
            launched[0]
                .0
                .iter()
                .all(|path| path.starts_with(&handoff_root)),
            "staged paths {:?} should be inside {:?}",
            launched[0].0,
            handoff_root
        );
        assert_eq!(fs::read(&first_path).unwrap(), first_bytes);
        assert_eq!(fs::read(&second_path).unwrap(), second_bytes);

        let stale = [
            targets[0].clone(),
            VersionedTarget {
                expected_version: second.metadata_version + 1,
                ..targets[1].clone()
            },
        ];
        assert!(adapter
            .open_in_slicer_batch(&context, &stale, "com.bambulab.bambu-studio")
            .is_err());
        assert_eq!(launches.lock().unwrap().len(), 1);
    }

    #[test]
    fn desktop_host_slicer_rejects_unknown_slicer_before_resolving_targets() {
        let temp = tempfile::tempdir().unwrap();
        let runtime =
            library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
                temp.path().join("library"),
                temp.path().join("integrations.json"),
            ))
            .unwrap();
        let adapter = DesktopHostSlicerAdapter::with_host_batch_ops(
            runtime,
            Vec::new,
            |_, _| panic!("unknown slicer must not launch"),
            |_, _| panic!("unknown slicer must not launch"),
        );
        let context = RequestContext {
            principal_id: "fixture".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::MediaRead,
                Capability::HostSlicer,
            ]),
            request_id: "reject-unknown-slicer".into(),
        };
        let error = adapter
            .open_in_slicer_batch(
                &context,
                &[VersionedTarget {
                    model_id: "not-present".into(),
                    expected_version: 1,
                }],
                "unknown.slicer",
            )
            .unwrap_err();

        assert_eq!(error.code, "invalidInput");
        assert_eq!(
            error.message,
            "Selected slicer is not installed or supported"
        );
    }
}
