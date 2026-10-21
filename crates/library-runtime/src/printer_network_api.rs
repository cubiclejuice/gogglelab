//! Host-owned printer network authority. Remote requests never install policy.
use std::collections::{BTreeSet, HashMap};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use crate::printer_network::{self as network, NetworkGuard};
use crate::{internal, invalid, LibraryRuntime};
use library_core::models::{Printer as CorePrinter, PrinterInput as CorePrinterInput};
use library_protocol::{
    Capability, Printer, PrinterConnectionResponse, PrinterConnectionResult, PrinterDiscoveryJob,
    PrinterDiscoveryJobStatus, PrinterDiscoveryPlan, PrinterNetworkConnection,
    PrinterNetworkEngine, PrinterNetworkPolicySummary, PrinterNetworkSuggestion,
    PrinterSuggestionSource, RequestContext, RuntimeError, RuntimeErrorCode,
};

const MAX_PLANS: usize = 32;
const MAX_JOBS: usize = 16;
const MAX_CHECKS: usize = 4;
const PLAN_TTL: Duration = Duration::from_secs(60);
const JOB_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct PrinterNetworkPolicy {
    addresses: BTreeSet<Ipv4Addr>,
    pub allow_multicast_discovery: bool,
    pub allow_carbon_broadcast: bool,
}
impl PrinterNetworkPolicy {
    pub fn new(
        admitted_ranges: Vec<String>,
        allow_multicast_discovery: bool,
        allow_carbon_broadcast: bool,
    ) -> Result<Self, RuntimeError> {
        if admitted_ranges.is_empty() || admitted_ranges.len() > 32 {
            return Err(invalid(
                "Configure between one and 32 private printer scopes",
            ));
        }
        let mut addresses = BTreeSet::new();
        for range in admitted_ranges {
            let scope = network::parse_scan_scope(&range)
                .map_err(|_| invalid("Invalid private printer scope"))?;
            addresses.extend(scope.addresses().iter().copied());
        }
        Ok(Self {
            addresses,
            allow_multicast_discovery,
            allow_carbon_broadcast,
        })
    }
}

/// Injected implementations must honor the supplied live guard; runtime also
/// protects publication and persistence against a transport which finishes late.
pub trait PrinterNetworkBackend: Send + Sync {
    fn discover(
        &self,
        scope: &network::scope::ScanScope,
        cancelled: Arc<AtomicBool>,
        guard: NetworkGuard,
        include_bambu: bool,
    ) -> Vec<network::Suggestion>;
    fn check(
        &self,
        printer: &CorePrinter,
        discover_mainboard_id: bool,
        guard: NetworkGuard,
    ) -> network::NetworkOutcome;
}
struct NativeBackend;
impl PrinterNetworkBackend for NativeBackend {
    fn discover(
        &self,
        scope: &network::scope::ScanScope,
        cancelled: Arc<AtomicBool>,
        guard: NetworkGuard,
        include_bambu: bool,
    ) -> Vec<network::Suggestion> {
        network::discover_with_options(scope, cancelled, guard, include_bambu)
    }
    fn check(
        &self,
        printer: &CorePrinter,
        discover_mainboard_id: bool,
        guard: NetworkGuard,
    ) -> network::NetworkOutcome {
        network::check_connection(printer, None, discover_mainboard_id, guard)
    }
}
struct Plan {
    owner: String,
    generation: u64,
    expires: Instant,
    scope: network::scope::ScanScope,
    include_bambu: bool,
}
struct Job {
    owner: String,
    generation: u64,
    expires: Instant,
    cancelled: Arc<AtomicBool>,
    value: PrinterDiscoveryJob,
}
struct Inner {
    policy: Option<PrinterNetworkPolicy>,
    generation: u64,
    plans: HashMap<String, Plan>,
    jobs: HashMap<String, Job>,
    active_discovery: Option<String>,
    checks: HashMap<String, Arc<AtomicBool>>,
}
pub struct PrinterNetworkState {
    inner: Mutex<Inner>,
    backend: Arc<dyn PrinterNetworkBackend>,
}
impl PrinterNetworkState {
    fn disabled(backend: Arc<dyn PrinterNetworkBackend>) -> Self {
        Self {
            inner: Mutex::new(Inner {
                policy: None,
                generation: 0,
                plans: HashMap::new(),
                jobs: HashMap::new(),
                active_discovery: None,
                checks: HashMap::new(),
            }),
            backend,
        }
    }
    pub fn with_backend(backend: Arc<dyn PrinterNetworkBackend>) -> Self {
        Self::disabled(backend)
    }
    fn set_policy(&self, policy: Option<PrinterNetworkPolicy>) -> Result<(), RuntimeError> {
        let mut inner = self.inner.lock().unwrap();
        let next = inner.generation.checked_add(1);
        if let Some(next) = next {
            inner.generation = next;
        }
        // Exhaustion cannot preserve or restore authority: revoke without wrapping.
        inner.policy = if next.is_some() { policy } else { None };
        inner.plans.clear();
        for job in inner.jobs.values_mut() {
            job.cancelled.store(true, Ordering::Release);
            job.value.status = PrinterDiscoveryJobStatus::Cancelled;
            job.value.suggestions.clear();
            job.value.error_code = Some("policy_changed".into());
        }
        for cancelled in inner.checks.values() {
            cancelled.store(true, Ordering::Release);
        }
        if next.is_none() {
            return Err(internal("Printer network policy generation exhausted"));
        }
        Ok(())
    }
}
fn denied() -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Forbidden,
        message: "Printer network access is not authorized by the host".into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    }
}
fn missing() -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::NotFound,
        message: "Printer network review or job was not found".into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    }
}
fn busy() -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Busy,
        message: "Printer network operation limit reached".into(),
        retryable: true,
        current_version: None,
        source_holds: Vec::new(),
    }
}
fn conflict(actual: u64) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Conflict,
        message: "Stored printer version changed".into(),
        retryable: true,
        current_version: Some(actual),
        source_holds: Vec::new(),
    }
}
fn cancelled_error() -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Conflict,
        message: "Printer network authority changed during this operation".into(),
        retryable: true,
        current_version: None,
        source_holds: Vec::new(),
    }
}
fn prune(inner: &mut Inner) {
    let now = Instant::now();
    inner.plans.retain(|_, plan| plan.expires > now);
    for job in inner.jobs.values_mut() {
        if job.expires <= now {
            job.cancelled.store(true, Ordering::Release);
            job.value.status = PrinterDiscoveryJobStatus::Cancelled;
            job.value.suggestions.clear();
            job.value.error_code = Some("expired".into());
        }
    }
    let active = inner.active_discovery.clone();
    inner
        .jobs
        .retain(|id, job| job.expires > now || active.as_ref() == Some(id));
}
fn live(inner: &Inner, generation: u64, cancelled: &AtomicBool) -> bool {
    inner.policy.is_some() && inner.generation == generation && !cancelled.load(Ordering::Acquire)
}
fn guard(
    state: &Arc<PrinterNetworkState>,
    generation: u64,
    cancelled: Arc<AtomicBool>,
) -> NetworkGuard {
    let state = Arc::downgrade(state);
    NetworkGuard::new(move || {
        state
            .upgrade()
            .is_some_and(|state| live(&state.inner.lock().unwrap(), generation, &cancelled))
    })
}
fn wire_suggestion(value: network::Suggestion) -> PrinterNetworkSuggestion {
    PrinterNetworkSuggestion {
        key: value.key,
        name: value.name,
        model: value.model,
        bed_mm: value.bed_mm,
        connection: PrinterNetworkConnection {
            kind: value.connection.kind,
            address: value.connection.address,
        },
        source: match value.source {
            network::SuggestionSource::Moonraker => PrinterSuggestionSource::Moonraker,
            network::SuggestionSource::Prusalink => PrinterSuggestionSource::Prusalink,
            network::SuggestionSource::BambuMqttTls => PrinterSuggestionSource::BambuMqttTls,
        },
        printer_type: value.printer_type,
    }
}
fn wire_result(value: network::PrinterConnectionTestResult) -> PrinterConnectionResult {
    match value {
        network::PrinterConnectionTestResult::Connected => PrinterConnectionResult::Connected,
        network::PrinterConnectionTestResult::Unreachable => PrinterConnectionResult::Unreachable,
        network::PrinterConnectionTestResult::Unsupported => PrinterConnectionResult::Unsupported,
        network::PrinterConnectionTestResult::AuthRequired => PrinterConnectionResult::AuthRequired,
        network::PrinterConnectionTestResult::Failed => PrinterConnectionResult::Failed,
        network::PrinterConnectionTestResult::Cancelled => PrinterConnectionResult::Cancelled,
    }
}
fn admitted(policy: &PrinterNetworkPolicy, printer: &CorePrinter) -> Result<(), RuntimeError> {
    let address = network::printer_address(printer)
        .ok_or_else(|| invalid("Printer checks require an admitted private IPv4 endpoint"))?;
    if !policy.addresses.contains(&address) {
        return Err(denied());
    }
    Ok(())
}
fn core_input(printer: &CorePrinter) -> CorePrinterInput {
    CorePrinterInput {
        nickname: printer.nickname.clone(),
        make: printer.make.clone(),
        model: printer.model.clone(),
        printer_type: printer.printer_type.clone(),
        firmware: printer.firmware.clone(),
        host: printer.host.clone(),
        web_url: printer.web_url.clone(),
        bed_mm: printer.bed_mm,
        notes: printer.notes.clone(),
    }
}
struct CheckLease {
    state: Arc<PrinterNetworkState>,
    id: String,
}
impl Drop for CheckLease {
    fn drop(&mut self) {
        self.state.inner.lock().unwrap().checks.remove(&self.id);
    }
}

impl LibraryRuntime {
    /// Trusted host configuration only. Browser/MCP requests have no equivalent.
    pub fn configure_printer_network(
        &self,
        policy: Option<PrinterNetworkPolicy>,
    ) -> Result<(), RuntimeError> {
        let state = {
            let mut states = self.shared_states.lock().unwrap();
            let value = states
                .entry(std::any::TypeId::of::<PrinterNetworkState>())
                .or_insert_with(|| {
                    Arc::new(PrinterNetworkState::disabled(Arc::new(NativeBackend)))
                });
            Arc::clone(value)
                .downcast::<PrinterNetworkState>()
                .map_err(|_| internal("Printer network state type mismatch"))?
        };
        state.set_policy(policy)
    }
    pub fn printer_network_enabled(&self) -> bool {
        self.capabilities.contains(&Capability::PrinterNetwork)
            && self
                .shared_state::<PrinterNetworkState>()
                .is_some_and(|state| state.inner.lock().unwrap().policy.is_some())
    }
    fn printer_network_state(
        &self,
        context: &RequestContext,
        write: bool,
    ) -> Result<Arc<PrinterNetworkState>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.require(context, Capability::PrinterNetwork)?;
        if write {
            self.require(context, Capability::CatalogWrite)?;
        }
        let state = self
            .shared_state::<PrinterNetworkState>()
            .ok_or_else(denied)?;
        if state.inner.lock().unwrap().policy.is_none() {
            return Err(denied());
        }
        Ok(state)
    }
}
impl PrinterNetworkEngine for LibraryRuntime {
    fn get_printer_network_policy(
        &self,
        context: &RequestContext,
    ) -> Result<PrinterNetworkPolicySummary, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        let policy = self
            .shared_state::<PrinterNetworkState>()
            .and_then(|state| state.inner.lock().unwrap().policy.clone());
        let enabled = context.capabilities.contains(&Capability::PrinterNetwork)
            && self.capabilities.contains(&Capability::PrinterNetwork)
            && policy.is_some();
        Ok(PrinterNetworkPolicySummary {
            enabled,
            discovery_available: enabled,
            status_checks_available: enabled
                && context.capabilities.contains(&Capability::CatalogWrite)
                && self.capabilities.contains(&Capability::CatalogWrite),
            multicast_discovery_allowed: enabled
                && policy.as_ref().is_some_and(|p| p.allow_multicast_discovery),
            carbon_broadcast_allowed: enabled
                && policy.as_ref().is_some_and(|p| p.allow_carbon_broadcast),
            max_discovery_addresses: 1022,
            max_suggestions: 33,
            maximum_duration_ms: 30000,
        })
    }
    fn plan_printer_discovery(
        &self,
        context: &RequestContext,
        range: &str,
        include_bambu_discovery: bool,
    ) -> Result<PrinterDiscoveryPlan, RuntimeError> {
        let state = self.printer_network_state(context, false)?;
        let scope = network::parse_scan_scope(range)
            .map_err(|_| invalid("Discovery requires a bounded private IPv4 scope"))?;
        let mut inner = state.inner.lock().unwrap();
        prune(&mut inner);
        let policy = inner.policy.as_ref().ok_or_else(denied)?;
        if scope
            .addresses()
            .iter()
            .any(|address| !policy.addresses.contains(address))
            || (include_bambu_discovery && !policy.allow_multicast_discovery)
        {
            return Err(denied());
        }
        if inner.plans.len() >= MAX_PLANS {
            return Err(busy());
        }
        let token = uuid::Uuid::new_v4().to_string();
        let result=PrinterDiscoveryPlan{plan_token:token.clone(),normalized_range:scope.to_string(),address_count:scope.addresses().len() as u32,include_bambu_discovery,multicast_disclosure:include_bambu_discovery.then(||"Bambu SSDP multicast reaches the routed LAN interface beyond the selected range; only selected addresses may produce candidates.".into()),expires_at:(time::OffsetDateTime::now_utc()+time::Duration::seconds(PLAN_TTL.as_secs() as i64)).format(&time::format_description::well_known::Rfc3339).map_err(|_|internal("Printer plan clock unavailable"))?};
        let generation = inner.generation;
        inner.plans.insert(
            token,
            Plan {
                owner: context.principal_id.clone(),
                generation,
                expires: Instant::now() + PLAN_TTL,
                scope,
                include_bambu: include_bambu_discovery,
            },
        );
        Ok(result)
    }
    fn start_printer_discovery(
        &self,
        context: &RequestContext,
        plan_token: &str,
        confirm: bool,
    ) -> Result<PrinterDiscoveryJob, RuntimeError> {
        let state = self.printer_network_state(context, false)?;
        if !confirm {
            return Err(invalid("Explicit discovery confirmation is required"));
        }
        let runtime = self
            .self_weak
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| internal("Runtime ownership unavailable"))?;
        let io = runtime.begin_io()?;
        let (plan, result, cancelled) = {
            let mut inner = state.inner.lock().unwrap();
            prune(&mut inner);
            let plan = inner
                .plans
                .get(plan_token)
                .filter(|plan| plan.owner == context.principal_id)
                .ok_or_else(missing)?;
            if inner.policy.is_none() || plan.generation != inner.generation {
                return Err(cancelled_error());
            }
            if inner.active_discovery.is_some() || inner.jobs.len() >= MAX_JOBS {
                return Err(busy());
            }
            let plan = inner.plans.remove(plan_token).unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            let cancelled = Arc::new(AtomicBool::new(false));
            let value = PrinterDiscoveryJob {
                job_id: id.clone(),
                status: PrinterDiscoveryJobStatus::Running,
                suggestions: Vec::new(),
                normalized_range: plan.scope.to_string(),
                error_code: None,
            };
            inner.jobs.insert(
                id.clone(),
                Job {
                    owner: context.principal_id.clone(),
                    generation: plan.generation,
                    expires: Instant::now() + JOB_TTL,
                    cancelled: Arc::clone(&cancelled),
                    value: value.clone(),
                },
            );
            inner.active_discovery = Some(id);
            (plan, value, cancelled)
        };
        let worker_state = Arc::clone(&state);
        let id = result.job_id.clone();
        let fail_id = id.clone();
        let spawned = std::thread::Builder::new()
            .name("printer-discovery".into())
            .spawn(move || {
                let _io = io;
                let operation_guard = guard(&worker_state, plan.generation, Arc::clone(&cancelled));
                let candidates = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker_state.backend.discover(
                        &plan.scope,
                        Arc::clone(&cancelled),
                        operation_guard,
                        plan.include_bambu,
                    )
                }));
                let mut inner = worker_state.inner.lock().unwrap();
                let allowed = live(&inner, plan.generation, &cancelled);
                if let Some(job) = inner.jobs.get_mut(&id) {
                    if !allowed {
                        job.value.status = PrinterDiscoveryJobStatus::Cancelled;
                        job.value.suggestions.clear();
                        job.value.error_code = Some("cancelled".into());
                    } else {
                        match candidates {
                            Ok(values) => {
                                job.value.suggestions =
                                    values.into_iter().take(33).map(wire_suggestion).collect();
                                job.value.status = PrinterDiscoveryJobStatus::Completed;
                            }
                            Err(_) => {
                                job.value.status = PrinterDiscoveryJobStatus::Failed;
                                job.value.error_code = Some("discovery_failed".into());
                            }
                        }
                    }
                }
                if inner.active_discovery.as_ref() == Some(&id) {
                    inner.active_discovery = None;
                }
            });
        if spawned.is_err() {
            let mut inner = state.inner.lock().unwrap();
            inner.active_discovery = None;
            inner.jobs.remove(&fail_id);
            return Err(internal("Printer discovery worker could not start"));
        }
        Ok(result)
    }
    fn get_printer_discovery(
        &self,
        context: &RequestContext,
        job_id: &str,
    ) -> Result<PrinterDiscoveryJob, RuntimeError> {
        let state = self.printer_network_state(context, false)?;
        let mut inner = state.inner.lock().unwrap();
        if inner.policy.is_none() {
            return Err(denied());
        }
        prune(&mut inner);
        let job = inner
            .jobs
            .get(job_id)
            .filter(|job| job.owner == context.principal_id)
            .ok_or_else(missing)?;
        let mut value = job.value.clone();
        if job.generation != inner.generation {
            value.status = PrinterDiscoveryJobStatus::Cancelled;
            value.suggestions.clear();
            value.error_code = Some("policy_changed".into());
        }
        Ok(value)
    }
    fn cancel_printer_discovery(
        &self,
        context: &RequestContext,
        job_id: &str,
    ) -> Result<PrinterDiscoveryJob, RuntimeError> {
        let state = self.printer_network_state(context, false)?;
        let mut inner = state.inner.lock().unwrap();
        if inner.policy.is_none() {
            return Err(denied());
        }
        prune(&mut inner);
        let job = inner
            .jobs
            .get_mut(job_id)
            .filter(|job| job.owner == context.principal_id)
            .ok_or_else(missing)?;
        job.cancelled.store(true, Ordering::Release);
        job.value.status = PrinterDiscoveryJobStatus::Cancelled;
        job.value.suggestions.clear();
        job.value.error_code = Some("cancelled".into());
        Ok(job.value.clone())
    }
    fn set_printer_endpoint_approval(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        approved: bool,
    ) -> Result<Printer, RuntimeError> {
        let state = self.printer_network_state(context, true)?;
        let _io = self.begin_borrowed_io()?;
        let inner = state.inner.lock().unwrap();
        let policy = inner.policy.as_ref().ok_or_else(denied)?;
        let printer = self.read_db(|db| db.get_printer(id))?.ok_or_else(missing)?;
        if printer.metadata_version != expected_version {
            return Err(conflict(printer.metadata_version));
        }
        if approved {
            admitted(policy, &printer)?;
        }
        let input = core_input(&printer);
        self.mutate_db(|db| db.update_printer(id, expected_version, &input, Some(approved)))
            .map(crate::fleet_printer)
    }
    fn test_printer_connection(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        discover_mainboard_id: bool,
        confirm: bool,
    ) -> Result<PrinterConnectionResponse, RuntimeError> {
        let state = self.printer_network_state(context, true)?;
        if !confirm {
            return Err(invalid("Explicit connection confirmation is required"));
        }
        let _io = self.begin_borrowed_io()?;
        let printer = self.read_db(|db| db.get_printer(id))?.ok_or_else(missing)?;
        if printer.metadata_version != expected_version {
            return Err(conflict(printer.metadata_version));
        }
        if !printer.endpoint_approved {
            return Err(invalid(
                "Review and approve the saved printer endpoint before checking",
            ));
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let check_id = uuid::Uuid::new_v4().to_string();
        let generation = {
            let mut inner = state.inner.lock().unwrap();
            let policy = inner.policy.as_ref().ok_or_else(denied)?;
            admitted(policy, &printer)?;
            let carbon = network::is_original_elegoo_centauri_carbon(&printer);
            if carbon && (!discover_mainboard_id || !policy.allow_carbon_broadcast) {
                return Err(denied());
            }
            if discover_mainboard_id && !carbon {
                return Err(invalid(
                    "Mainboard discovery requires an original Centauri Carbon",
                ));
            }
            if inner.checks.len() >= MAX_CHECKS {
                return Err(busy());
            }
            inner
                .checks
                .insert(check_id.clone(), Arc::clone(&cancelled));
            inner.generation
        };
        let _check = CheckLease {
            state: Arc::clone(&state),
            id: check_id,
        };
        let outcome = state.backend.check(
            &printer,
            discover_mainboard_id,
            guard(&state, generation, Arc::clone(&cancelled)),
        );
        let inner = state.inner.lock().unwrap();
        if !live(&inner, generation, &cancelled) {
            return Err(cancelled_error());
        }
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| internal("Printer status clock unavailable"))?;
        let updated = self.mutate_db(|db| {
            db.record_printer_status_if_current(
                &printer,
                outcome.connection_status,
                outcome.job_status,
                &now,
                outcome.error_code,
            )
        })?;
        let response = PrinterConnectionResponse {
            result: wire_result(outcome.result),
            printer: crate::fleet_printer(updated),
            status_event_observed: outcome.status_event_observed,
        };
        drop(inner);
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use library_protocol::{PrinterFleetEngine, RuntimeInfoEngine};
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::{self, Sender};

    #[derive(Default)]
    struct FakeBackend {
        discoveries: AtomicUsize,
        checks: AtomicUsize,
        multicast: AtomicUsize,
        broadcast: AtomicUsize,
        entered: Mutex<Option<Sender<()>>>,
        release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }
    impl FakeBackend {
        fn wait(&self) {
            if let Some(entered) = self.entered.lock().unwrap().take() {
                entered.send(()).unwrap();
            }
            if let Some(release) = self.release.lock().unwrap().take() {
                release.recv_timeout(Duration::from_secs(3)).unwrap();
            }
        }
    }
    impl PrinterNetworkBackend for FakeBackend {
        fn discover(
            &self,
            _scope: &network::scope::ScanScope,
            _cancelled: Arc<AtomicBool>,
            _guard: NetworkGuard,
            include_bambu: bool,
        ) -> Vec<network::Suggestion> {
            self.discoveries.fetch_add(1, Ordering::AcqRel);
            if include_bambu {
                self.multicast.fetch_add(1, Ordering::AcqRel);
            }
            self.wait();
            vec![network::Suggestion {
                key: "moonraker:192.168.200.10".into(),
                name: "Synthetic".into(),
                model: None,
                bed_mm: None,
                connection: network::NetworkConnection {
                    kind: "network".into(),
                    address: "192.168.200.10".into(),
                },
                source: network::SuggestionSource::Moonraker,
                printer_type: "FDM".into(),
            }]
        }
        fn check(
            &self,
            _printer: &CorePrinter,
            discover_mainboard_id: bool,
            _guard: NetworkGuard,
        ) -> network::NetworkOutcome {
            self.checks.fetch_add(1, Ordering::AcqRel);
            if discover_mainboard_id {
                self.broadcast.fetch_add(1, Ordering::AcqRel);
            }
            self.wait();
            network::NetworkOutcome {
                result: network::PrinterConnectionTestResult::Connected,
                connection_status: library_core::PrinterConnectionStatus::Online,
                job_status: library_core::PrinterJobStatus::Printing,
                error_code: None,
                status_event_observed: true,
            }
        }
    }
    fn context() -> RequestContext {
        RequestContext {
            principal_id: "operator".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::CatalogWrite,
                Capability::PrinterNetwork,
            ]),
            request_id: "synthetic".into(),
        }
    }
    fn fixture() -> (tempfile::TempDir, Arc<LibraryRuntime>, Arc<FakeBackend>) {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(crate::RuntimeConfig::desktop(
            temp.path().join("catalog"),
            temp.path().join("integrations.json"),
        ))
        .unwrap();
        let backend = Arc::new(FakeBackend::default());
        runtime
            .install_shared_state(Arc::new(PrinterNetworkState::with_backend(backend.clone())))
            .unwrap();
        (temp, runtime, backend)
    }
    fn policy(multicast: bool, broadcast: bool) -> PrinterNetworkPolicy {
        PrinterNetworkPolicy::new(vec!["192.168.200.10/32".into()], multicast, broadcast).unwrap()
    }
    fn printer(runtime: &LibraryRuntime, carbon: bool) -> Printer {
        runtime
            .create_printer(
                &context(),
                library_protocol::PrinterInput {
                    nickname: "Synthetic".into(),
                    make: carbon.then(|| "Elegoo".into()),
                    model: carbon.then(|| "Centauri Carbon".into()),
                    printer_type: None,
                    firmware: None,
                    host: Some("192.168.200.10".into()),
                    web_url: None,
                    bed_mm: None,
                    notes: "".into(),
                },
            )
            .unwrap()
    }
    fn wait_job(runtime: &LibraryRuntime, job_id: &str) -> PrinterDiscoveryJob {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let job = runtime.get_printer_discovery(&context(), job_id).unwrap();
            if job.status != PrinterDiscoveryJobStatus::Running {
                return job;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    fn block(backend: &FakeBackend) -> (std::sync::mpsc::Receiver<()>, Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        *backend.entered.lock().unwrap() = Some(entered_tx);
        *backend.release.lock().unwrap() = Some(release_rx);
        (entered_rx, release_tx)
    }

    #[test]
    fn host_policy_defaults_off_and_capabilities_are_intersected() {
        let (_temp, runtime, backend) = fixture();
        let ctx = context();
        assert!(!runtime.get_printer_network_policy(&ctx).unwrap().enabled);
        assert!(!runtime
            .runtime_info(&ctx)
            .unwrap()
            .capabilities
            .contains(&Capability::PrinterNetwork));
        assert!(runtime
            .plan_printer_discovery(&ctx, "192.168.200.10", false)
            .is_err());
        runtime
            .configure_printer_network(Some(policy(false, false)))
            .unwrap();
        assert!(runtime
            .runtime_info(&ctx)
            .unwrap()
            .capabilities
            .contains(&Capability::PrinterNetwork));
        let mut reader = ctx.clone();
        reader.capabilities.remove(&Capability::PrinterNetwork);
        assert!(!runtime.get_printer_network_policy(&reader).unwrap().enabled);
        assert!(runtime
            .plan_printer_discovery(&reader, "192.168.200.10", false)
            .is_err());
        assert!(runtime
            .plan_printer_discovery(&ctx, "192.168.200.11", false)
            .is_err());
        assert!(runtime
            .plan_printer_discovery(&ctx, "127.0.0.1", false)
            .is_err());
        assert!(runtime
            .plan_printer_discovery(&ctx, "192.168.200.10", true)
            .is_err());
        assert_eq!(backend.discoveries.load(Ordering::Acquire), 0);
        assert_eq!(backend.checks.load(Ordering::Acquire), 0);
    }

    #[test]
    fn discovery_review_is_owner_bound_one_use_expiring_and_no_io() {
        let (_temp, runtime, backend) = fixture();
        runtime
            .configure_printer_network(Some(policy(false, false)))
            .unwrap();
        let ctx = context();
        let plan = runtime
            .plan_printer_discovery(&ctx, "192.168.200.10", false)
            .unwrap();
        assert_eq!(plan.address_count, 1);
        assert!(plan.multicast_disclosure.is_none());
        assert_eq!(backend.discoveries.load(Ordering::Acquire), 0);
        let mut other = ctx.clone();
        other.principal_id = "other".into();
        assert!(runtime
            .start_printer_discovery(&other, &plan.plan_token, true)
            .is_err());
        assert!(runtime
            .start_printer_discovery(&ctx, &plan.plan_token, false)
            .is_err());
        let job = runtime
            .start_printer_discovery(&ctx, &plan.plan_token, true)
            .unwrap();
        assert!(runtime
            .start_printer_discovery(&ctx, &plan.plan_token, true)
            .is_err());
        assert!(runtime.get_printer_discovery(&other, &job.job_id).is_err());
        assert!(runtime
            .cancel_printer_discovery(&other, &job.job_id)
            .is_err());
        let done = wait_job(&runtime, &job.job_id);
        assert_eq!(done.status, PrinterDiscoveryJobStatus::Completed);
        assert_eq!(done.suggestions.len(), 1);
        assert_eq!(backend.multicast.load(Ordering::Acquire), 0);
        let plan = runtime
            .plan_printer_discovery(&ctx, "192.168.200.10", false)
            .unwrap();
        let state = runtime.shared_state::<PrinterNetworkState>().unwrap();
        state
            .inner
            .lock()
            .unwrap()
            .plans
            .get_mut(&plan.plan_token)
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        assert!(runtime
            .start_printer_discovery(&ctx, &plan.plan_token, true)
            .is_err());
    }

    #[test]
    fn cancellation_and_disable_reenable_cannot_publish_late_discovery() {
        let (_temp, runtime, backend) = fixture();
        runtime
            .configure_printer_network(Some(policy(true, false)))
            .unwrap();
        let ctx = context();
        let plan = runtime
            .plan_printer_discovery(&ctx, "192.168.200.10", true)
            .unwrap();
        assert!(plan.multicast_disclosure.is_some());
        let (entered, release) = block(&backend);
        let job = runtime
            .start_printer_discovery(&ctx, &plan.plan_token, true)
            .unwrap();
        entered.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(runtime.begin_maintenance().is_err());
        assert!(runtime
            .start_printer_discovery(
                &ctx,
                &runtime
                    .plan_printer_discovery(&ctx, "192.168.200.10", false)
                    .unwrap()
                    .plan_token,
                true
            )
            .is_err());
        runtime.cancel_printer_discovery(&ctx, &job.job_id).unwrap();
        runtime.configure_printer_network(None).unwrap();
        runtime
            .configure_printer_network(Some(policy(true, false)))
            .unwrap();
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while runtime
            .shared_state::<PrinterNetworkState>()
            .unwrap()
            .inner
            .lock()
            .unwrap()
            .active_discovery
            .is_some()
        {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let job = runtime.get_printer_discovery(&ctx, &job.job_id).unwrap();
        assert_eq!(job.status, PrinterDiscoveryJobStatus::Cancelled);
        assert!(job.suggestions.is_empty());
        assert_eq!(backend.multicast.load(Ordering::Acquire), 1);
    }

    #[test]
    fn status_checks_reject_stale_unapproved_and_missing_capabilities_before_io() {
        let (_temp, runtime, backend) = fixture();
        runtime
            .configure_printer_network(Some(policy(false, false)))
            .unwrap();
        let ctx = context();
        let p = printer(&runtime, false);
        assert!(runtime
            .test_printer_connection(&ctx, &p.id, p.metadata_version, false, true)
            .is_err());
        let p = runtime
            .set_printer_endpoint_approval(&ctx, &p.id, p.metadata_version, true)
            .unwrap();
        assert!(runtime
            .test_printer_connection(&ctx, &p.id, p.metadata_version - 1, false, true)
            .is_err());
        let mut reader = ctx.clone();
        reader.capabilities.remove(&Capability::CatalogWrite);
        assert!(runtime
            .test_printer_connection(&reader, &p.id, p.metadata_version, false, true)
            .is_err());
        assert!(runtime
            .set_printer_endpoint_approval(&reader, &p.id, p.metadata_version, false)
            .is_err());
        assert!(runtime
            .test_printer_connection(&ctx, &p.id, p.metadata_version, false, false)
            .is_err());
        assert_eq!(backend.checks.load(Ordering::Acquire), 0);
        let result = runtime
            .test_printer_connection(&ctx, &p.id, p.metadata_version, false, true)
            .unwrap();
        assert_eq!(result.result, PrinterConnectionResult::Connected);
        assert_eq!(
            result.printer.job_status,
            library_protocol::PrinterJobStatus::Printing
        );
        assert_eq!(result.printer.metadata_version, p.metadata_version + 1);
    }

    #[test]
    fn carbon_broadcast_requires_its_own_permission_and_explicit_choice() {
        let (_temp, runtime, backend) = fixture();
        runtime
            .configure_printer_network(Some(policy(true, false)))
            .unwrap();
        let ctx = context();
        let p = printer(&runtime, true);
        let p = runtime
            .set_printer_endpoint_approval(&ctx, &p.id, p.metadata_version, true)
            .unwrap();
        assert!(runtime
            .test_printer_connection(&ctx, &p.id, p.metadata_version, true, true)
            .is_err());
        assert_eq!(backend.checks.load(Ordering::Acquire), 0);
        runtime
            .configure_printer_network(Some(policy(false, true)))
            .unwrap();
        assert!(runtime
            .test_printer_connection(&ctx, &p.id, p.metadata_version, false, true)
            .is_err());
        let result = runtime
            .test_printer_connection(&ctx, &p.id, p.metadata_version, true, true)
            .unwrap();
        assert_eq!(result.result, PrinterConnectionResult::Connected);
        assert_eq!(backend.broadcast.load(Ordering::Acquire), 1);
        assert_eq!(backend.multicast.load(Ordering::Acquire), 0);
    }

    #[test]
    fn policy_revocation_and_profile_edits_prevent_status_commit_after_io() {
        let (_temp, runtime, backend) = fixture();
        runtime
            .configure_printer_network(Some(policy(false, false)))
            .unwrap();
        let ctx = context();
        let p = printer(&runtime, false);
        let p = runtime
            .set_printer_endpoint_approval(&ctx, &p.id, p.metadata_version, true)
            .unwrap();
        let (entered, release) = block(&backend);
        let worker = Arc::clone(&runtime);
        let worker_id = p.id.clone();
        let version = p.metadata_version;
        let thread = std::thread::spawn(move || {
            worker.test_printer_connection(&context(), &worker_id, version, false, true)
        });
        entered.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(runtime.begin_maintenance().is_err());
        runtime.configure_printer_network(None).unwrap();
        runtime
            .configure_printer_network(Some(policy(false, false)))
            .unwrap();
        release.send(()).unwrap();
        assert!(thread.join().unwrap().is_err());
        let unchanged = runtime.get_printer(&ctx, &p.id).unwrap().unwrap();
        assert_eq!(unchanged.metadata_version, p.metadata_version);
        assert!(unchanged.last_checked_at.is_none());
        let (entered, release) = block(&backend);
        let worker = Arc::clone(&runtime);
        let worker_id = p.id.clone();
        let thread = std::thread::spawn(move || {
            worker.test_printer_connection(&context(), &worker_id, version, false, true)
        });
        entered.recv_timeout(Duration::from_secs(3)).unwrap();
        let prior = runtime
            .read_db(|db| db.get_printer(&p.id))
            .unwrap()
            .unwrap();
        let edited = CorePrinterInput {
            nickname: "Edited".into(),
            ..core_input(&prior)
        };
        runtime
            .mutate_db(|db| db.update_printer(&p.id, p.metadata_version, &edited, None))
            .unwrap();
        release.send(()).unwrap();
        assert_eq!(
            thread.join().unwrap().unwrap_err().code,
            RuntimeErrorCode::Conflict
        );
        assert!(runtime
            .get_printer(&ctx, &p.id)
            .unwrap()
            .unwrap()
            .last_checked_at
            .is_none());
    }
    #[test]
    fn review_and_result_memory_are_bounded_and_expired_jobs_disappear() {
        let (_temp, runtime, _backend) = fixture();
        runtime
            .configure_printer_network(Some(policy(false, false)))
            .unwrap();
        let ctx = context();
        for _ in 0..MAX_PLANS {
            runtime
                .plan_printer_discovery(&ctx, "192.168.200.10", false)
                .unwrap();
        }
        assert_eq!(
            runtime
                .plan_printer_discovery(&ctx, "192.168.200.10", false)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        let state = runtime.shared_state::<PrinterNetworkState>().unwrap();
        for plan in state.inner.lock().unwrap().plans.values_mut() {
            plan.expires = Instant::now() - Duration::from_secs(1);
        }
        let mut last = None;
        for _ in 0..MAX_JOBS {
            let plan = runtime
                .plan_printer_discovery(&ctx, "192.168.200.10", false)
                .unwrap();
            let job = runtime
                .start_printer_discovery(&ctx, &plan.plan_token, true)
                .unwrap();
            wait_job(&runtime, &job.job_id);
            last = Some(job.job_id);
        }
        let plan = runtime
            .plan_printer_discovery(&ctx, "192.168.200.10", false)
            .unwrap();
        assert_eq!(
            runtime
                .start_printer_discovery(&ctx, &plan.plan_token, true)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        let last = last.unwrap();
        state
            .inner
            .lock()
            .unwrap()
            .jobs
            .get_mut(&last)
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            runtime.get_printer_discovery(&ctx, &last).unwrap_err().code,
            RuntimeErrorCode::NotFound
        );
        assert!(state.inner.lock().unwrap().jobs.len() < MAX_JOBS);
    }

    #[test]
    fn maintenance_prevents_transport_and_does_not_consume_a_review() {
        let (_temp, runtime, backend) = fixture();
        runtime
            .configure_printer_network(Some(policy(false, false)))
            .unwrap();
        let ctx = context();
        let p = printer(&runtime, false);
        let p = runtime
            .set_printer_endpoint_approval(&ctx, &p.id, p.metadata_version, true)
            .unwrap();
        let plan = runtime
            .plan_printer_discovery(&ctx, "192.168.200.10", false)
            .unwrap();
        let maintenance = runtime.begin_maintenance().unwrap();
        assert_eq!(
            runtime
                .start_printer_discovery(&ctx, &plan.plan_token, true)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Maintenance
        );
        assert_eq!(
            runtime
                .test_printer_connection(&ctx, &p.id, p.metadata_version, false, true)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Maintenance
        );
        assert_eq!(backend.checks.load(Ordering::Acquire), 0);
        assert_eq!(backend.discoveries.load(Ordering::Acquire), 0);
        drop(maintenance);
        let job = runtime
            .start_printer_discovery(&ctx, &plan.plan_token, true)
            .unwrap();
        assert_eq!(
            wait_job(&runtime, &job.job_id).status,
            PrinterDiscoveryJobStatus::Completed
        );
    }
    #[test]
    fn policy_generation_exhaustion_revokes_and_cannot_restore_authority() {
        let (_temp, runtime, _backend) = fixture();
        runtime
            .configure_printer_network(Some(policy(false, false)))
            .unwrap();
        runtime
            .shared_state::<PrinterNetworkState>()
            .unwrap()
            .inner
            .lock()
            .unwrap()
            .generation = u64::MAX;
        assert!(runtime.configure_printer_network(None).is_err());
        assert!(!runtime.printer_network_enabled());
        assert!(runtime
            .configure_printer_network(Some(policy(false, false)))
            .is_err());
        assert!(!runtime.printer_network_enabled());
    }
    #[test]
    fn out_of_scope_saved_endpoint_can_be_revoked_but_not_granted_or_checked() {
        let (_temp, runtime, backend) = fixture();
        runtime
            .configure_printer_network(Some(policy(false, false)))
            .unwrap();
        let ctx = context();
        let p = printer(&runtime, false);
        let approved = runtime
            .set_printer_endpoint_approval(&ctx, &p.id, p.metadata_version, true)
            .unwrap();
        runtime
            .configure_printer_network(Some(
                PrinterNetworkPolicy::new(vec!["192.168.200.11/32".into()], false, false).unwrap(),
            ))
            .unwrap();
        assert_eq!(
            runtime
                .set_printer_endpoint_approval(&ctx, &p.id, approved.metadata_version, true)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert_eq!(
            runtime
                .test_printer_connection(&ctx, &p.id, approved.metadata_version, false, true)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        let revoked = runtime
            .set_printer_endpoint_approval(&ctx, &p.id, approved.metadata_version, false)
            .unwrap();
        assert!(!revoked.endpoint_approved);
        assert_eq!(revoked.metadata_version, approved.metadata_version + 1);
        assert_eq!(
            runtime
                .set_printer_endpoint_approval(&ctx, &p.id, approved.metadata_version, false)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Conflict
        );
        assert_eq!(backend.checks.load(Ordering::Acquire), 0);
        assert_eq!(backend.discoveries.load(Ordering::Acquire), 0);
    }
}
