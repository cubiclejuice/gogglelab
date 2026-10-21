//! Synthetic browser acceptance fixture. This executable never uses printer transports.
//!
//! Run with one absolute, nonexistent task-owned output directory. It refuses to
//! reuse any directory, binds only 127.0.0.1:3847, and serves dist-browser from this
//! checkout. The printed key is deliberately public fixture data, not a credential.
use std::collections::BTreeSet;
use std::path::{Component, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use library_core::{Printer, PrinterConnectionStatus, PrinterJobStatus};
use library_protocol::Capability;
use library_runtime::printer_network::{
    self, NetworkGuard, NetworkOutcome, PrinterConnectionTestResult, Suggestion,
};
use library_runtime::printer_network_api::{
    PrinterNetworkBackend, PrinterNetworkPolicy, PrinterNetworkState,
};
use library_runtime::{LibraryRuntime, RuntimeConfig};
use library_server::{
    router_with_static, start_listener, AllowedAuthority, AllowedOrigin, BearerSecret, HostMode,
    ProtocolEngineAdapter, SecurityConfig,
};

const ADDRESS: &str = "127.0.0.1:3847";
const ORIGIN: &str = "http://127.0.0.1:3847";
const FIXTURE_KEY: &str = "SYNTHETIC-PRINTER-NETWORK-QA-ONLY-000000000000000";
const MOONRAKER: std::net::Ipv4Addr = std::net::Ipv4Addr::new(10, 250, 0, 10);

#[derive(Default)]
struct SyntheticBackend {
    discovery_calls: AtomicUsize,
    status_calls: AtomicUsize,
}
impl PrinterNetworkBackend for SyntheticBackend {
    fn discover(
        &self,
        scope: &printer_network::scope::ScanScope,
        cancelled: Arc<AtomicBool>,
        _guard: NetworkGuard,
        _include_bambu: bool,
    ) -> Vec<Suggestion> {
        self.discovery_calls.fetch_add(1, Ordering::AcqRel);
        // Exercise real asynchronous jobs/cancellation without a network socket.
        for _ in 0..50 {
            if cancelled.load(Ordering::Acquire) {
                return Vec::new();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if cancelled.load(Ordering::Acquire) || !scope.addresses().contains(&MOONRAKER) {
            return Vec::new();
        }
        vec![Suggestion {
            key: "moonraker:10.250.0.10".into(),
            name: "Synthetic Moonraker".into(),
            model: Some("Browser acceptance fixture".into()),
            bed_mm: Some([256, 256, 256]),
            connection: printer_network::NetworkConnection {
                kind: "network".into(),
                address: MOONRAKER.to_string(),
            },
            source: printer_network::SuggestionSource::Moonraker,
            printer_type: "FDM".into(),
        }]
    }

    fn check(
        &self,
        _printer: &Printer,
        _discover_mainboard_id: bool,
        _guard: NetworkGuard,
    ) -> NetworkOutcome {
        // These values are constants. No MainboardID, request payload, device,
        // socket, URL, credential, or native transport exists in this backend.
        // The real runtime still enforces approval, scope, version and policy CAS.
        self.status_calls.fetch_add(1, Ordering::AcqRel);
        NetworkOutcome {
            result: PrinterConnectionTestResult::Connected,
            connection_status: PrinterConnectionStatus::Online,
            job_status: PrinterJobStatus::Printing,
            error_code: None,
            status_event_observed: true,
        }
    }
}

fn new_output_directory() -> Result<PathBuf, String> {
    let mut arguments = std::env::args_os().skip(1);
    let directory = PathBuf::from(arguments.next().ok_or(
        "Usage: printer_network_fixture /absolute/nonexistent/task-owned-output-directory",
    )?);
    if arguments.next().is_some()
        || !directory.is_absolute()
        || directory
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        || directory.file_name().is_none()
    {
        return Err("Provide exactly one absolute new output directory".into());
    }
    // create_dir is atomic and fails for an existing directory, file or symlink.
    std::fs::create_dir(&directory)
        .map_err(|_| "Fixture output directory must not already exist; its parent must exist")?;
    Ok(directory)
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let output = new_output_directory()?;
    let web = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist-browser");
    let capabilities = BTreeSet::from([
        Capability::CatalogRead,
        Capability::CatalogWrite,
        Capability::MediaRead,
        Capability::MediaWrite,
        Capability::PrinterNetwork,
    ]);
    let mut config =
        RuntimeConfig::desktop(output.join("catalog"), output.join("integrations.json"));
    config.capabilities = capabilities.clone();
    let runtime = LibraryRuntime::open(config).map_err(|error| error.to_string())?;
    // Refuse to proceed if runtime initialization ever begins installing a
    // production network backend implicitly. Only this fake state may be used.
    assert!(runtime.shared_state::<PrinterNetworkState>().is_none());
    let backend = Arc::new(SyntheticBackend::default());
    let synthetic_state = Arc::new(PrinterNetworkState::with_backend(backend.clone()));
    runtime
        .install_shared_state(synthetic_state.clone())
        .map_err(|error| error.to_string())?;
    runtime
        .configure_printer_network(Some(
            PrinterNetworkPolicy::new(
                vec!["10.250.0.10/32".into(), "10.250.0.11/32".into()],
                true,
                true,
            )
            .map_err(|error| error.to_string())?,
        ))
        .map_err(|error| error.to_string())?;
    assert!(Arc::ptr_eq(
        &synthetic_state,
        &runtime.shared_state::<PrinterNetworkState>().unwrap(),
    ));
    let engine = Arc::new(ProtocolEngineAdapter::new(
        runtime.clone(),
        HostMode::Headless,
    ));
    let app = router_with_static(
        engine,
        SecurityConfig {
            bearer_token: BearerSecret::from_string(FIXTURE_KEY.into())?,
            allowed_hosts: BTreeSet::from([AllowedAuthority::parse(ADDRESS)?]),
            allowed_origins: BTreeSet::from([AllowedOrigin::parse(ORIGIN)?]),
            principal_capabilities: capabilities,
            max_in_flight: 16,
            max_mcp_sessions: 4,
            mcp_session_ttl: Duration::from_secs(60),
            trusted_proxy_peers: BTreeSet::new(),
            public_base_url: None,
            content_send_timeout: Duration::from_secs(5),
        },
        &web,
    )?;
    let listener = tokio::net::TcpListener::bind(ADDRESS)
        .await
        .map_err(|_| "Synthetic fixture loopback port is unavailable")?;
    let handle = start_listener(listener, app).await?;
    println!("SYNTHETIC FIXTURE ONLY — no outbound printer transport");
    println!("Browser URL: {ORIGIN}");
    println!("Public fixture key: {FIXTURE_KEY}");
    println!("Private synthetic output: {}", output.display());
    println!("Discovery scope: 10.250.0.10/32; optional manual Carbon profile: 10.250.0.11");
    println!("Press Ctrl+C to revoke the synthetic policy and stop.");
    let signal = tokio::signal::ctrl_c().await;
    let revocation = runtime.configure_printer_network(None);
    let stopped = handle.stop().await;
    println!(
        "Synthetic calls: discovery={}, status={}",
        backend.discovery_calls.load(Ordering::Acquire),
        backend.status_calls.load(Ordering::Acquire),
    );
    signal.map_err(|_| "Could not receive fixture shutdown signal")?;
    revocation.map_err(|error| error.to_string())?;
    stopped
}
