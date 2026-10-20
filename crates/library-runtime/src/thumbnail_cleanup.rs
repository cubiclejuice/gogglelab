//! Bounded, source-proven raster cleanup. No publication or source-copy deletion.
use super::*;
use library_protocol::{
    ThumbnailBatch, ThumbnailCleanupMode as Mode, ThumbnailCleanupRequest, ThumbnailProgress,
};
use std::ffi::OsString;
use std::time::Instant;

const PNG_LIMIT: u64 = 4 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(300);
const SOURCE_LIMIT: u64 = 400 * 1024 * 1024;
const RASTER_LIMIT: u64 = 128_000_000;

#[derive(Default)]
struct Usage {
    bytes: u64,
    fds: usize,
}
struct Permit {
    usage: Arc<Mutex<Usage>>,
    bytes: u64,
    fds: usize,
}
impl Permit {
    fn resize(&mut self, bytes: u64) {
        let mut usage = self.usage.lock().unwrap();
        usage.bytes -= self.bytes - bytes;
        self.bytes = bytes;
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut usage = self.usage.lock().unwrap();
        usage.bytes -= self.bytes;
        usage.fds -= self.fds;
    }
}
#[derive(Default)]
pub(super) struct Store {
    worker: AtomicBool,
    usage: Arc<Mutex<Usage>>,
    cursors: Mutex<HashMap<String, Cursor>>,
    #[cfg(test)]
    before_read: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    after_read: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    after_reset_commit: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    after_retire: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    after_render: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    force_deadline: Arc<AtomicBool>,
}
impl Store {
    fn reserve(&self, bytes: u64, fds: usize) -> Result<Permit, RuntimeError> {
        let mut usage = self.usage.lock().unwrap();
        if usage.bytes + bytes > 64 * 1024 * 1024 || usage.fds + fds > 64 {
            return Err(busy(
                "Preview review resources are full; discard an existing review",
            ));
        }
        usage.bytes += bytes;
        usage.fds += fds;
        Ok(Permit {
            usage: self.usage.clone(),
            bytes,
            fds,
        })
    }
    fn worker(&self) -> Result<Worker<'_>, RuntimeError> {
        self.worker
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| busy("A preview review is already running"))?;
        Ok(Worker(&self.worker))
    }
}
struct Worker<'a>(&'a AtomicBool);
impl Drop for Worker<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
struct Binding {
    root: secure_fs::Directory,
    thumbnails: Option<secure_fs::Directory>,
    _permit: Permit,
}
impl Binding {
    fn verify(&self, root: &Path) -> Result<(), RuntimeError> {
        if !self
            .root
            .matches_path(root)
            .map_err(|_| stale("Library binding changed"))?
            || self.thumbnails.as_ref().is_some_and(|directory| {
                !directory
                    .matches_path(&root.join("thumbnails"))
                    .unwrap_or(false)
            })
        {
            return Err(stale("Preview namespace changed"));
        }
        Ok(())
    }
}
struct Cursor {
    owner: String,
    epoch: u64,
    revision: u64,
    expires: SystemTime,
    binding: Arc<Binding>,
    last: OsString,
    audit_unknown: bool,
}
struct Source {
    model: Model,
    source: library_core::ResolvedModelSource,
    limit: Option<u64>,
}
struct Candidate {
    receipt: secure_fs::PngFileReceipt,
    source: Source,
    relative: String,
    _proof: Arc<thumbnails::RasterGeneratedPng>,
    _permit: Permit,
}
pub(super) struct Plan {
    binding: Arc<Binding>,
    candidates: Vec<Candidate>,
    namespace: Option<(secure_fs::PngNamespace, Permit)>,
    pub(super) summary: ThumbnailBatch,
    reset_snapshot: Option<(u32, [u8; 32])>,
    next: Option<(String, Cursor)>,
}
struct Work {
    start: Instant,
    bytes: u64,
    samples: u64,
    renders: usize,
    #[cfg(test)]
    force_deadline: Option<Arc<AtomicBool>>,
}
impl Work {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            bytes: 0,
            samples: 0,
            renders: 0,
            #[cfg(test)]
            force_deadline: None,
        }
    }
    fn expired(&self) -> bool {
        #[cfg(test)]
        if self
            .force_deadline
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
        {
            return true;
        }
        self.start.elapsed() >= Duration::from_secs(2)
    }
    fn admit(&mut self, bytes: u64, samples: u64, render: bool) -> bool {
        if self.expired()
            || self.bytes.saturating_add(bytes) > SOURCE_LIMIT
            || self.samples.saturating_add(samples) > RASTER_LIMIT
            || (render && self.renders >= 4)
        {
            return false;
        }
        self.bytes += bytes;
        self.samples += samples;
        self.renders += usize::from(render);
        true
    }
}
fn busy(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Busy,
        message: message.into(),
        retryable: true,
        current_version: None,
        source_holds: Vec::new(),
    }
}
fn require(runtime: &LibraryRuntime, ctx: &RequestContext, mode: Mode) -> Result<(), RuntimeError> {
    require_durable_source_mutations()?;
    runtime.require(ctx, Capability::Maintenance)?;
    runtime.require(ctx, Capability::MediaWrite)?;
    if mode == Mode::ResetDefaults {
        runtime.require(ctx, Capability::CatalogWrite)?;
    }
    Ok(())
}
pub(super) fn prune(runtime: &LibraryRuntime) {
    let now = SystemTime::now();
    let epoch = runtime.library_epoch();
    let revision = runtime.catalog_revision();
    runtime.actions.lock().unwrap().retain(|_, p| {
        p.expires > now && p.library_epoch == epoch && p.catalog_revision == revision
    });
    runtime
        .thumbnail_cleanup
        .cursors
        .lock()
        .unwrap()
        .retain(|_, c| c.expires > now && c.epoch == epoch && c.revision == revision);
    let plans = runtime
        .actions
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(key, p)| {
            p.thumbnail_cleanup
                .as_ref()
                .map(|p| (key.clone(), p.binding.clone()))
        })
        .collect::<Vec<_>>();
    let stale_plans = plans
        .iter()
        .filter(|(_, binding)| binding.verify(&runtime.root).is_err())
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    let cursors = runtime
        .thumbnail_cleanup
        .cursors
        .lock()
        .unwrap()
        .iter()
        .map(|(key, c)| (key.clone(), c.binding.clone()))
        .collect::<Vec<_>>();
    let stale_cursors = cursors
        .iter()
        .filter(|(_, binding)| binding.verify(&runtime.root).is_err())
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    {
        let mut store = runtime.actions.lock().unwrap();
        for key in stale_plans {
            store.remove(&key);
        }
    }
    {
        let mut store = runtime.thumbnail_cleanup.cursors.lock().unwrap();
        for key in stale_cursors {
            store.remove(&key);
        }
    }
}
fn source(
    runtime: &LibraryRuntime,
    db: &LibraryDb,
    hash: &str,
) -> Result<Option<Source>, RuntimeError> {
    let Some(model) = db
        .current_model_for_content_hash(hash)
        .map_err(runtime_library_error)?
    else {
        return Ok(None);
    };
    if model.source_missing {
        return Ok(None);
    }
    let resolved = db
        .resolve_model_source_ref(&model.id)
        .map_err(runtime_library_error)?;
    if resolved.archive_member.is_some() {
        return Ok(None);
    }
    if runtime
        .validate_indexed_source_admission(&model, &resolved)
        .is_err()
    {
        return Ok(None);
    }
    let limit = if model.source_kind == "indexed" {
        let Some(root) = db
            .list_index_roots()
            .map_err(runtime_library_error)?
            .into_iter()
            .find(|r| Some(r.id.as_str()) == model.source_root_id.as_deref())
        else {
            return Ok(None);
        };
        Some(root.max_file_bytes)
    } else {
        None
    };
    if resolved.bytes > stl_core::limits::MAX_FILE_BYTES
        || limit.is_some_and(|n| resolved.bytes > n)
        || resolved.content_hash != hash
    {
        return Ok(None);
    }
    Ok(Some(Source {
        model,
        source: resolved,
        limit,
    }))
}
struct VerifiedSource {
    file: File,
    bytes: Vec<u8>,
    path: PathBuf,
    metadata: fs::Metadata,
    _fds: Permit,
}
impl VerifiedSource {
    fn verify(&self) -> Result<(), RuntimeError> {
        if self.path.canonicalize().ok().as_ref() != Some(&self.path) {
            return Err(stale("Source path changed"));
        }
        let current = library_core::catalog_vfs::open_pinned_file(&self.path)
            .map_err(|_| stale("Source entry changed"))?;
        let actual = self.file.metadata().map_err(|_| io_error())?;
        let named = current.metadata().map_err(|_| io_error())?;
        if !library_core::catalog_vfs::same_file_identity(&self.file, &current).unwrap_or(false)
            || actual.len() != self.metadata.len()
            || named.len() != self.metadata.len()
            || actual.modified().map_err(|_| io_error())?
                != self.metadata.modified().map_err(|_| io_error())?
            || named.modified().map_err(|_| io_error())?
                != self.metadata.modified().map_err(|_| io_error())?
        {
            return Err(stale("Source was replaced or changed while reading"));
        }
        Ok(())
    }
}
fn read_source(runtime: &LibraryRuntime, source: &Source) -> Result<VerifiedSource, RuntimeError> {
    if source.source.archive_member.is_some()
        || source.limit.is_some_and(|n| source.source.bytes > n)
    {
        return Err(stale("Source is no longer admitted"));
    }
    #[cfg(test)]
    {
        let hook = runtime.thumbnail_cleanup.before_read.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
    }
    let fds = runtime.thumbnail_cleanup.reserve(0, 2)?;
    let path = &source.source.outer_path;
    if path.canonicalize().ok().as_ref() != Some(path) {
        return Err(stale("Source path changed"));
    }
    let mut file =
        library_core::catalog_vfs::open_pinned_file(path).map_err(|_| stale("Source changed"))?;
    let metadata = file.metadata().map_err(|_| io_error())?;
    if metadata.len() != source.source.bytes {
        return Err(stale("Source size changed"));
    }
    let mut bytes = Vec::with_capacity(source.source.bytes as usize);
    Read::by_ref(&mut file)
        .take(source.source.bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| io_error())?;
    if bytes.len() as u64 != source.source.bytes
        || format!("{:x}", Sha256::digest(&bytes)) != source.source.content_hash
    {
        return Err(stale("Source bytes changed"));
    }
    #[cfg(test)]
    {
        let hook = runtime.thumbnail_cleanup.after_read.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
    }
    let verified = VerifiedSource {
        file,
        bytes,
        path: path.clone(),
        metadata,
        _fds: fds,
    };
    verified.verify()?;
    Ok(verified)
}
fn bind(runtime: &LibraryRuntime) -> Result<Arc<Binding>, RuntimeError> {
    let permit = runtime.thumbnail_cleanup.reserve(0, 2)?;
    let root = secure_fs::Directory::open(&runtime.root).map_err(|_| io_error())?;
    let thumbnails = match root.open_child(std::ffi::OsStr::new("thumbnails")) {
        Ok(d) => Some(d),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return Err(stale("Preview namespace is unsafe")),
    };
    let value = Arc::new(Binding {
        root,
        thumbnails,
        _permit: permit,
    });
    value.verify(&runtime.root)?;
    Ok(value)
}
fn protected_copies(runtime: &LibraryRuntime, work: &Work) -> Result<u32, RuntimeError> {
    let root = &runtime.root;
    let cache = root.join("indexed-cache");
    let Ok(metadata) = fs::symlink_metadata(&cache) else {
        return Ok(0);
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(stale("Source-copy namespace is unsafe"));
    }
    if work.expired() {
        return Err(busy("Source-copy audit deadline reached"));
    }
    let _fds = runtime.thumbnail_cleanup.reserve(0, 2)?;
    let directory = secure_fs::Directory::open(&cache).map_err(|_| io_error())?;
    let mut count = 0u32;
    for name in directory
        .read_entry_names(MAX_ORPHAN_CLEANUP_FILES)
        .map_err(|_| busy("Source-copy namespace exceeds its review bound"))?
    {
        if work.expired() {
            return Err(busy("Preview review deadline reached"));
        }
        match directory.open_child(&name) {
            Ok(child) => {
                count = count.saturating_add(
                    child
                        .read_entry_names(MAX_ORPHAN_CLEANUP_FILES - count as usize)
                        .map_err(|_| busy("Source-copy namespace exceeds its review bound"))?
                        .len() as u32,
                )
            }
            Err(_) => count = count.saturating_add(1),
        }
        if count as usize >= MAX_ORPHAN_CLEANUP_FILES {
            return Err(busy("Source-copy namespace exceeds its review bound"));
        }
    }
    Ok(count)
}
impl LibraryRuntime {
    pub fn review_thumbnail_cleanup(
        &self,
        ctx: &RequestContext,
        request: ThumbnailCleanupRequest,
    ) -> Result<ActionPlan, RuntimeError> {
        self.plan_action(
            ctx,
            match request.mode {
                Mode::Orphans => ManagementAction::CleanupOrphans {
                    continuation: request.continuation,
                },
                Mode::ResetDefaults => {
                    if request.continuation.is_some() {
                        return Err(invalid("Default reset does not accept a continuation"));
                    }
                    ManagementAction::ResetDefaultThumbnails
                }
            },
        )
    }
}
pub(super) fn review(
    runtime: &LibraryRuntime,
    ctx: &RequestContext,
    request: ThumbnailCleanupRequest,
) -> Result<ActionPlan, RuntimeError> {
    require(runtime, ctx, request.mode)?;
    prune(runtime);
    let _worker = runtime.thumbnail_cleanup.worker()?;
    let _io = runtime.begin_borrowed_io()?;
    let epoch = runtime.library_epoch();
    let revision = runtime.catalog_revision();
    let (binding, last, audit_unknown) = if let Some(token) = request.continuation {
        let mut cursors = runtime.thumbnail_cleanup.cursors.lock().unwrap();
        let cursor = cursors
            .get(&token)
            .ok_or_else(|| stale("Review continuation is unknown or expired"))?;
        if cursor.owner != ctx.principal_id {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Review continuation belongs to another principal".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        let cursor = cursors.remove(&token).unwrap();
        drop(cursors);
        cursor.binding.verify(&runtime.root)?;
        (cursor.binding, Some(cursor.last), cursor.audit_unknown)
    } else {
        (bind(runtime)?, None, false)
    };
    let mut summary = ThumbnailBatch {
        mode: request.mode,
        reviewed_files: 0,
        eligible_files: 0,
        eligible_bytes: 0,
        preserved_files: 0,
        protected_source_copies: 0,
        default_records_to_clear: 0,
        partial: audit_unknown,
        has_more: false,
        remaining_files: if audit_unknown { None } else { Some(0) },
        continuation: None,
    };
    let mut plan = Plan {
        binding,
        namespace: None,
        candidates: Vec::new(),
        summary: summary.clone(),
        reset_snapshot: None,
        next: None,
    };
    if request.mode == Mode::ResetDefaults {
        let snapshot = runtime.read_db(|db| db.default_thumbnail_reset_snapshot())?;
        summary.default_records_to_clear = snapshot.0;
        plan.reset_snapshot = Some(snapshot);
    } else {
        let mut work = Work::new();
        #[cfg(test)]
        {
            work.force_deadline = Some(runtime.thumbnail_cleanup.force_deadline.clone());
        }
        let active = runtime.active_content_lease_paths();
        let names = match &plan.binding.thumbnails {
            Some(d) => match d.read_entry_names(MAX_ORPHAN_CLEANUP_FILES) {
                Ok(mut n) => {
                    n.sort();
                    n
                }
                Err(_) => {
                    summary.partial = true;
                    summary.remaining_files = None;
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        let first_page = last.is_none();
        let names = names
            .into_iter()
            .filter(|n| last.as_ref().is_none_or(|last| n > last))
            .collect::<Vec<_>>();
        let mut inspected = 0usize;
        let mut last_inspected = None;
        for name in &names {
            if work.expired() || plan.candidates.len() >= 4 {
                break;
            }
            let relative = Path::new("thumbnails")
                .join(name)
                .to_string_lossy()
                .replace('\\', "/");
            let proof_recipe = thumbnails::ThumbnailRecipe::parse_filename(name);
            let referenced = if proof_recipe.is_some() {
                runtime.read_db(|db| db.thumbnail_path_is_referenced(&relative))?
            } else {
                true
            };
            if referenced || active.contains(&runtime.root.join(&relative)) {
                summary.preserved_files += 1;
            } else {
                let recipe = proof_recipe.unwrap();
                let snapshot =
                    runtime.read_db(|db| Ok(source(runtime, db, recipe.source_hash())))??;
                if let Some(snapshot) = snapshot {
                    if !work.admit(snapshot.source.bytes, recipe.raster_sample_budget(), true) {
                        break;
                    }
                    let mut permit = runtime.thumbnail_cleanup.reserve(PNG_LIMIT, 1)?;
                    let _buffer =
                        runtime.reserve_temp(ReservationKind::Owned, snapshot.source.bytes)?;
                    let proof = read_source(runtime, &snapshot).and_then(|verified| {
                        let proof = thumbnails::reproduce_thumbnail(
                            &recipe,
                            &verified.bytes,
                            snapshot.source.bytes,
                        )
                        .map_err(|_| stale("Preview could not be reproduced"))?;
                        verified.verify()?;
                        #[cfg(test)]
                        {
                            let hook = runtime
                                .thumbnail_cleanup
                                .after_render
                                .lock()
                                .unwrap()
                                .take();
                            if let Some(hook) = hook {
                                hook();
                            }
                        }
                        Ok(proof.map(|proof| (proof, verified)))
                    });
                    if let Ok(Some((proof, verified))) = proof {
                        let proof = Arc::new(proof);
                        if plan.namespace.is_none() {
                            let p = runtime.thumbnail_cleanup.reserve(0, 2)?;
                            let n =
                                secure_fs::PngNamespace::open(&runtime.root, &plan.binding.root)
                                    .map_err(|_| stale("Preview namespace changed"))?;
                            plan.namespace = Some((n, p));
                        }
                        verified.verify()?;
                        match plan
                            .namespace
                            .as_ref()
                            .unwrap()
                            .0
                            .admit_reproduced_png(name, proof.clone())
                        {
                            Ok(receipt) => {
                                permit.resize(proof.bytes().len() as u64);
                                summary.eligible_files += 1;
                                summary.eligible_bytes += proof.bytes().len() as u64;
                                plan.candidates.push(Candidate {
                                    receipt,
                                    source: snapshot,
                                    relative,
                                    _proof: proof,
                                    _permit: permit,
                                });
                            }
                            Err(_) => summary.preserved_files += 1,
                        }
                    } else {
                        summary.preserved_files += 1;
                    }
                } else {
                    summary.preserved_files += 1;
                }
            }
            inspected += 1;
            summary.reviewed_files += 1;
            last_inspected = Some(name.clone());
        }
        summary.has_more = inspected < names.len();
        summary.partial |= summary.has_more || work.expired();
        if summary.remaining_files.is_some() {
            summary.remaining_files = Some((names.len() - inspected) as u32);
        }
        match if first_page {
            protected_copies(runtime, &work)
        } else {
            Ok(0)
        } {
            Ok(n) => {
                summary.protected_source_copies = n;
                summary.preserved_files += n;
                summary.reviewed_files += n;
            }
            Err(_) => {
                summary.partial = true;
                summary.remaining_files = None;
            }
        }
        if summary.has_more {
            if let Some(last) = last_inspected.or(last) {
                let token = uuid::Uuid::new_v4().simple().to_string();
                summary.continuation = Some(token.clone());
                plan.next = Some((
                    token,
                    Cursor {
                        owner: ctx.principal_id.clone(),
                        epoch,
                        revision,
                        expires: SystemTime::now() + TTL,
                        binding: plan.binding.clone(),
                        last,
                        audit_unknown: summary.remaining_files.is_none(),
                    },
                ));
            }
        }
    }
    plan.binding.verify(&runtime.root)?;
    recheck_management_plan(runtime, epoch, revision)?;
    plan.summary = summary.clone();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let expires = SystemTime::now() + TTL;
    let mut actions = runtime.actions.lock().unwrap();
    if actions.len() >= 64 {
        return Err(busy("Management review limit reached"));
    }
    actions.insert(
        token.clone(),
        PlannedAction {
            action: match request.mode {
                Mode::Orphans => ManagementAction::CleanupOrphans { continuation: None },
                Mode::ResetDefaults => ManagementAction::ResetDefaultThumbnails,
            },
            principal_id: ctx.principal_id.clone(),
            expires,
            library_epoch: epoch,
            catalog_revision: revision,
            targets: Vec::new(),
            input_fingerprint: None,
            physical_sources: Vec::new(),
            managed_assets: Vec::new(),
            thumbnail_cleanup: Some(plan),
        },
    );
    Ok(ActionPlan {
        token,
        action_kind: match request.mode {
            Mode::Orphans => "cleanupOrphans",
            Mode::ResetDefaults => "resetDefaultThumbnails",
        }
        .into(),
        expires_at: format_system_time(expires)?,
        library_epoch: epoch,
        catalog_revision: revision,
        targets: Vec::new(),
        impact: vec![format!(
            "Reviewed {} files; {} rebuilt previews may be removed; {} files preserved",
            summary.reviewed_files, summary.eligible_files, summary.preserved_files
        )],
        input_bytes: None,
        input_sha256: None,
        thumbnail_review: Some(summary),
    })
}
fn retire_candidate(
    runtime: &LibraryRuntime,
    binding: &Binding,
    namespace: &secure_fs::PngNamespace,
    candidate: &mut Candidate,
    epoch: u64,
    revision: u64,
) -> Result<secure_fs::PngRetirementOutcome, secure_fs::PngRetirementOutcome> {
    let checks = (|| -> Result<_, RuntimeError> {
        binding.verify(&runtime.root)?;
        recheck_management_plan(runtime, epoch, revision)?;
        let db = LibraryDb::open(&runtime.root).map_err(runtime_library_error)?;
        let referenced = db
            .thumbnail_path_is_referenced(&candidate.relative)
            .map_err(runtime_library_error)?;
        let current = source(runtime, &db, &candidate.source.source.content_hash)?;
        drop(db);
        let _buffer =
            runtime.reserve_temp(ReservationKind::Owned, candidate.source.source.bytes)?;
        if referenced
            || runtime
                .active_content_lease_paths()
                .contains(&runtime.root.join(&candidate.relative))
            || current.as_ref().is_none_or(|s| {
                s.model.id != candidate.source.model.id
                    || s.model.metadata_version != candidate.source.model.metadata_version
                    || s.source.outer_path != candidate.source.source.outer_path
                    || s.source.bytes != candidate.source.source.bytes
                    || s.source.content_hash != candidate.source.source.content_hash
            })
        {
            return Err(stale("Preview review conditions changed"));
        }
        let verified = read_source(runtime, &candidate.source)?;
        binding.verify(&runtime.root)?;
        recheck_management_plan(runtime, epoch, revision)?;
        // Recheck references after the bounded source read immediately
        // before capture; never repeat the source-byte prepass.
        let db = LibraryDb::open(&runtime.root).map_err(runtime_library_error)?;
        let referenced = db
            .thumbnail_path_is_referenced(&candidate.relative)
            .map_err(runtime_library_error)?;
        let current = source(runtime, &db, &candidate.source.source.content_hash)?;
        drop(db);
        if referenced
            || current.as_ref().is_none_or(|s| {
                s.model.id != candidate.source.model.id
                    || s.model.metadata_version != candidate.source.model.metadata_version
                    || s.source.outer_path != candidate.source.source.outer_path
            })
            || runtime
                .active_content_lease_paths()
                .contains(&runtime.root.join(&candidate.relative))
        {
            return Err(stale("Preview review conditions changed"));
        }
        verified.verify()?;
        Ok((verified, _buffer))
    })();
    let Ok((verified, _buffer)) = checks else {
        return Err(secure_fs::PngRetirementOutcome::default());
    };
    if verified.verify().is_err() {
        return Err(secure_fs::PngRetirementOutcome::default());
    }
    namespace
        .retire_png(&mut candidate.receipt)
        .map_err(|error| error.outcome)
}

pub(super) fn execute(
    runtime: &LibraryRuntime,
    ctx: &RequestContext,
    mut plan: Plan,
    epoch: u64,
    revision: u64,
) -> Result<ManagementResult, RuntimeError> {
    require(runtime, ctx, plan.summary.mode)?;
    let _worker = runtime.thumbnail_cleanup.worker()?;
    let arc = runtime
        .self_weak
        .get()
        .and_then(Weak::upgrade)
        .ok_or_else(|| internal("Runtime is unavailable"))?;
    let _maintenance = arc.begin_maintenance()?;
    recheck_management_plan(runtime, epoch, revision)?;
    plan.binding.verify(&runtime.root)?;
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stopped = false;
    let mut cleared = 0;
    if plan.summary.mode == Mode::ResetDefaults {
        let db = LibraryDb::open(&runtime.root).map_err(runtime_library_error)?;
        let snapshot = plan
            .reset_snapshot
            .as_ref()
            .ok_or_else(|| internal("Default reset snapshot is missing"))?;
        let _models = runtime
            .thumbnail_cleanup
            .reserve(u64::from(snapshot.0) * 192, 0)?;
        cleared = db
            .reset_default_thumbnail_records(snapshot)
            .map_err(runtime_library_error)?;
        #[cfg(test)]
        {
            let hook = runtime
                .thumbnail_cleanup
                .after_reset_commit
                .lock()
                .unwrap()
                .take();
            if let Some(hook) = hook {
                hook();
            }
        }
        stopped = plan.binding.verify(&runtime.root).is_err();
        runtime.record_mutation("defaultThumbnailsReset", Vec::new());
    } else {
        let mut work = Work::new();
        #[cfg(test)]
        {
            work.force_deadline = Some(runtime.thumbnail_cleanup.force_deadline.clone());
        }
        for candidate in &mut plan.candidates {
            if !work.admit(candidate.source.source.bytes, 0, false) {
                stopped = true;
                break;
            }
            let outcome = retire_candidate(
                runtime,
                &plan.binding,
                &plan.namespace.as_ref().unwrap().0,
                candidate,
                epoch,
                revision,
            );
            match outcome {
                Ok(o) => {
                    files += u64::from(o.unlinked_now);
                    bytes += o.removed_bytes;
                    #[cfg(test)]
                    {
                        let hook = runtime
                            .thumbnail_cleanup
                            .after_retire
                            .lock()
                            .unwrap()
                            .take();
                        if let Some(hook) = hook {
                            hook();
                        }
                    }
                }
                Err(e) => {
                    files += u64::from(e.unlinked_now);
                    bytes += e.removed_bytes;
                    stopped = true;
                    break;
                }
            }
        }
    }
    if plan.binding.verify(&runtime.root).is_err()
        || (plan.summary.mode == Mode::Orphans
            && recheck_management_plan(runtime, epoch, revision).is_err())
    {
        stopped = true;
    }
    if stopped {
        plan.summary.continuation = None;
        plan.summary.partial = true;
    } else if let Some((token, mut cursor)) = plan.next.take() {
        let mut cursors = runtime.thumbnail_cleanup.cursors.lock().unwrap();
        if cursors.len() < 64 {
            cursor.expires = SystemTime::now() + TTL;
            cursors.insert(token, cursor);
        } else {
            plan.summary.continuation = None;
            plan.summary.partial = true;
        }
    }
    Ok(ManagementResult::CleanupCompleted {
        files,
        bytes,
        thumbnail_progress: Some(ThumbnailProgress {
            review: plan.summary,
            default_records_cleared: cleared,
            stopped,
        }),
    })
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    struct Fixture {
        _temp: tempfile::TempDir,
        runtime: Arc<LibraryRuntime>,
        ctx: RequestContext,
        model: Model,
        paths: Vec<PathBuf>,
    }
    fn fixture(count: usize) -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("library");
        let runtime = LibraryRuntime::open(RuntimeConfig::desktop(
            root.clone(),
            temp.path().join("integrations.json"),
        ))
        .unwrap();
        let mut bytes = vec![0u8; 84];
        bytes[80..84].copy_from_slice(&1u32.to_le_bytes());
        for n in [0f32, 0., 1., 0., 0., 0., 20., 0., 0., 0., 20., 0.] {
            bytes.extend_from_slice(&n.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
        let input = temp.path().join("source.stl");
        fs::write(&input, &bytes).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&input, None, None).map(|x| x.model))
            .unwrap();
        let mut paths = Vec::new();
        for index in 0..count {
            let settings = library_core::CatalogSettings {
                thumbnail_size: 128,
                thumbnail_background: format!("#{index:06x}"),
                thumbnail_concurrency: 1,
                scan_batch_size: 128,
            };
            let artifact = thumbnails::ensure_thumbnail_from_bytes(
                &root,
                &bytes,
                model.content_hash.as_deref().unwrap(),
                bytes.len() as u64,
                &settings,
                true,
            )
            .unwrap();
            paths.push(root.join(artifact.relative_path));
        }
        let ctx = RequestContext {
            principal_id: "review-owner".into(),
            request_id: "synthetic-cleanup".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::CatalogWrite,
                Capability::Maintenance,
                Capability::MediaWrite,
            ]),
        };
        Fixture {
            _temp: temp,
            runtime,
            ctx,
            model,
            paths,
        }
    }
    fn review(f: &Fixture, continuation: Option<String>) -> ActionPlan {
        f.runtime
            .review_thumbnail_cleanup(
                &f.ctx,
                ThumbnailCleanupRequest {
                    mode: Mode::Orphans,
                    continuation,
                },
            )
            .unwrap()
    }
    #[test]
    fn thumbnail_cleanup_proves_raster_and_continues_after_four_receipts() {
        let f = fixture(6);
        let plan = review(&f, None);
        let summary = plan.thumbnail_review.as_ref().unwrap();
        assert_eq!(summary.eligible_files, 4);
        assert!(summary.partial);
        assert_eq!(summary.remaining_files, Some(2));
        let continuation = summary.continuation.clone().unwrap();
        assert!(f
            .runtime
            .review_thumbnail_cleanup(
                &f.ctx,
                ThumbnailCleanupRequest {
                    mode: Mode::Orphans,
                    continuation: Some(continuation.clone())
                }
            )
            .is_err());
        let result = f.runtime.execute_action(&f.ctx, &plan.token).unwrap();
        assert!(matches!(
            result,
            ManagementResult::CleanupCompleted {
                files: 4,
                thumbnail_progress: Some(ThumbnailProgress { stopped: false, .. }),
                ..
            }
        ));
        let next = review(&f, Some(continuation));
        assert_eq!(next.thumbnail_review.as_ref().unwrap().eligible_files, 2);
        assert!(matches!(
            f.runtime.execute_action(&f.ctx, &next.token).unwrap(),
            ManagementResult::CleanupCompleted { files: 2, .. }
        ));
        assert!(f.paths.iter().all(|p| !p.exists()));
        assert_eq!(f.runtime.thumbnail_cleanup.usage.lock().unwrap().bytes, 0);
        assert_eq!(f.runtime.thumbnail_cleanup.usage.lock().unwrap().fds, 0);
    }
    #[test]
    fn thumbnail_cleanup_discard_owner_reduced_caps_and_expiry_release_resources() {
        let f = fixture(1);
        let plan = review(&f, None);
        let mut other = f.ctx.clone();
        other.principal_id = "other".into();
        assert_eq!(
            f.runtime
                .discard_action(&other, &plan.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert!(f.runtime.actions.lock().unwrap().contains_key(&plan.token));
        let mut owner = f.ctx.clone();
        owner.capabilities.clear();
        f.runtime.discard_action(&owner, &plan.token).unwrap();
        assert!(f.paths[0].exists());
        assert_eq!(f.runtime.thumbnail_cleanup.usage.lock().unwrap().bytes, 0);
        assert_eq!(f.runtime.thumbnail_cleanup.usage.lock().unwrap().fds, 0);
        let expired = review(&f, None);
        f.runtime
            .actions
            .lock()
            .unwrap()
            .get_mut(&expired.token)
            .unwrap()
            .expires = SystemTime::UNIX_EPOCH;
        let next = review(&f, None);
        assert!(!f
            .runtime
            .actions
            .lock()
            .unwrap()
            .contains_key(&expired.token));
        f.runtime.discard_action(&owner, &next.token).unwrap();
        assert_eq!(f.runtime.thumbnail_cleanup.usage.lock().unwrap().fds, 0);
    }
    #[test]
    fn thumbnail_cleanup_preserves_same_bytes_replacement_and_changed_source() {
        for changed_source in [false, true] {
            let f = fixture(1);
            let plan = review(&f, None);
            if changed_source {
                let source = f
                    .runtime
                    .read_db(|db| db.resolve_model_source_ref(&f.model.id))
                    .unwrap();
                fs::write(source.outer_path, b"changed original source").unwrap();
            } else {
                let bytes = fs::read(&f.paths[0]).unwrap();
                fs::rename(&f.paths[0], f.paths[0].with_extension("old")).unwrap();
                fs::write(&f.paths[0], bytes).unwrap();
            }
            let result = f.runtime.execute_action(&f.ctx, &plan.token).unwrap();
            assert!(matches!(
                result,
                ManagementResult::CleanupCompleted {
                    files: 0,
                    thumbnail_progress: Some(ThumbnailProgress { stopped: true, .. }),
                    ..
                }
            ));
            assert!(f.paths[0].exists());
        }
    }
    #[test]
    fn thumbnail_cleanup_reference_and_reset_are_metadata_only() {
        let f = fixture(1);
        let bytes = fs::read(&f.paths[0]).unwrap();
        let relative = f.paths[0].strip_prefix(&f.runtime.root).unwrap();
        f.runtime
            .mutate_db(|db| {
                db.set_thumbnail(
                    &f.model.id,
                    &format!("{:x}", Sha256::digest(&bytes)),
                    relative,
                )
            })
            .unwrap();
        let plan = review(&f, None);
        assert_eq!(plan.thumbnail_review.as_ref().unwrap().eligible_files, 0);
        assert_eq!(plan.thumbnail_review.as_ref().unwrap().preserved_files, 1);
        f.runtime.discard_action(&f.ctx, &plan.token).unwrap();
        let reset = f
            .runtime
            .review_thumbnail_cleanup(
                &f.ctx,
                ThumbnailCleanupRequest {
                    mode: Mode::ResetDefaults,
                    continuation: None,
                },
            )
            .unwrap();
        assert_eq!(
            reset
                .thumbnail_review
                .as_ref()
                .unwrap()
                .default_records_to_clear,
            1
        );
        let result = f.runtime.execute_action(&f.ctx, &reset.token).unwrap();
        assert!(matches!(
            result,
            ManagementResult::CleanupCompleted {
                files: 0,
                thumbnail_progress: Some(ThumbnailProgress {
                    default_records_cleared: 1,
                    ..
                }),
                ..
            }
        ));
        assert_eq!(fs::read(&f.paths[0]).unwrap(), bytes);
        let next = review(&f, None);
        assert_eq!(next.thumbnail_review.as_ref().unwrap().eligible_files, 1);
    }
    #[test]
    fn thumbnail_cleanup_actual_review_holds_io_and_global_worker_before_source_read() {
        let f = fixture(1);
        let runtime = f.runtime.clone();
        let weak = Arc::downgrade(&runtime);
        let (tx, rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *runtime.thumbnail_cleanup.before_read.lock().unwrap() = Some(Box::new(move || {
            let runtime = weak.upgrade().unwrap();
            assert_eq!(
                runtime.begin_maintenance().err().unwrap().code,
                RuntimeErrorCode::Busy
            );
            tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        let ctx = f.ctx.clone();
        let thread = std::thread::spawn(move || {
            runtime.review_thumbnail_cleanup(
                &ctx,
                ThumbnailCleanupRequest {
                    mode: Mode::Orphans,
                    continuation: None,
                },
            )
        });
        rx.recv().unwrap();
        assert_eq!(
            f.runtime
                .review_thumbnail_cleanup(
                    &f.ctx,
                    ThumbnailCleanupRequest {
                        mode: Mode::Orphans,
                        continuation: None
                    }
                )
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        release_tx.send(()).unwrap();
        let plan = thread.join().unwrap().unwrap();
        f.runtime.discard_action(&f.ctx, &plan.token).unwrap();
    }
    #[test]
    fn thumbnail_cleanup_work_and_global_permits_are_bounded_and_released() {
        let store = Store::default();
        let mut permit = store.reserve(64 * 1024 * 1024, 64).unwrap();
        assert!(store.reserve(1, 0).is_err());
        assert!(store.reserve(0, 1).is_err());
        permit.resize(4);
        drop(permit);
        assert_eq!(store.usage.lock().unwrap().bytes, 0);
        let mut work = Work::new();
        for _ in 0..4 {
            assert!(work.admit(100 * 1024 * 1024, 32_000_000, true));
        }
        assert!(!work.admit(1, 0, false));
        assert!(!work.admit(0, 0, true));
    }
    #[test]
    fn thumbnail_cleanup_continuation_advances_foreign_prefix_and_is_owner_bound() {
        let f = fixture(6);
        for i in 0..20 {
            fs::write(
                f.runtime
                    .root
                    .join("thumbnails")
                    .join(format!("000-foreign-{i:02}.png")),
                b"operator bytes",
            )
            .unwrap();
        }
        let first = review(&f, None);
        let summary = first.thumbnail_review.as_ref().unwrap();
        assert_eq!(summary.reviewed_files, 24);
        assert_eq!(summary.preserved_files, 20);
        let next = summary.continuation.clone().unwrap();
        f.runtime.execute_action(&f.ctx, &first.token).unwrap();
        let mut other = f.ctx.clone();
        other.principal_id = "other".into();
        assert_eq!(
            f.runtime
                .review_thumbnail_cleanup(
                    &other,
                    ThumbnailCleanupRequest {
                        mode: Mode::Orphans,
                        continuation: Some(next.clone())
                    }
                )
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        let final_page = review(&f, Some(next));
        assert_eq!(
            final_page.thumbnail_review.as_ref().unwrap().eligible_files,
            2
        );
    }
    #[test]
    fn thumbnail_cleanup_namespace_replacement_prunes_receipts_without_touching_previous_files() {
        let f = fixture(1);
        let plan = review(&f, None);
        let original = f.runtime.root.join("thumbnails");
        let previous = f.runtime.root.join("previous-thumbnails");
        fs::rename(&original, &previous).unwrap();
        fs::create_dir(&original).unwrap();
        assert_eq!(
            f.runtime
                .discard_action(&f.ctx, &plan.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
        assert_eq!(f.runtime.thumbnail_cleanup.usage.lock().unwrap().fds, 0);
        assert!(previous.join(f.paths[0].file_name().unwrap()).exists());
        assert_eq!(fs::read_dir(original).unwrap().count(), 0);
    }
    #[test]
    fn thumbnail_cleanup_partial_source_failure_reports_prior_removal_and_stops() {
        let f = fixture(3);
        let plan = review(&f, None);
        let path = f
            .runtime
            .read_db(|db| db.resolve_model_source_ref(&f.model.id))
            .unwrap()
            .outer_path;
        let weak = Arc::downgrade(&f.runtime);
        *f.runtime.thumbnail_cleanup.before_read.lock().unwrap() = Some(Box::new(move || {
            let runtime = weak.upgrade().unwrap();
            *runtime.thumbnail_cleanup.before_read.lock().unwrap() = Some(Box::new(move || {
                fs::write(path, b"ordinary changed source").unwrap();
            }));
        }));
        let result = f.runtime.execute_action(&f.ctx, &plan.token).unwrap();
        assert!(matches!(
            result,
            ManagementResult::CleanupCompleted {
                files: 1,
                thumbnail_progress: Some(ThumbnailProgress { stopped: true, .. }),
                ..
            }
        ));
        assert_eq!(f.paths.iter().filter(|p| p.exists()).count(), 2);
        assert_eq!(f.runtime.thumbnail_cleanup.usage.lock().unwrap().fds, 0);
    }
    #[test]
    fn thumbnail_cleanup_active_reader_blocks_execution_without_removing_files() {
        let f = fixture(1);
        let plan = review(&f, None);
        let _reader = f.runtime.begin_io().unwrap();
        assert_eq!(
            f.runtime
                .execute_action(&f.ctx, &plan.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        assert!(f.paths[0].exists());
        assert_eq!(f.runtime.thumbnail_cleanup.usage.lock().unwrap().fds, 0);
    }
    #[test]
    fn thumbnail_cleanup_source_replacement_after_hash_is_not_blessed_by_equal_bytes() {
        for execution in [false, true] {
            let f = fixture(1);
            let plan = if execution {
                Some(review(&f, None))
            } else {
                None
            };
            let path = f
                .runtime
                .read_db(|db| db.resolve_model_source_ref(&f.model.id))
                .unwrap()
                .outer_path;
            let old = path.with_extension("old-source");
            let bytes = fs::read(&path).unwrap();
            *f.runtime.thumbnail_cleanup.after_read.lock().unwrap() = Some(Box::new(move || {
                fs::rename(&path, old).unwrap();
                fs::write(path, bytes).unwrap();
            }));
            if let Some(plan) = plan {
                assert!(matches!(
                    f.runtime.execute_action(&f.ctx, &plan.token).unwrap(),
                    ManagementResult::CleanupCompleted {
                        files: 0,
                        thumbnail_progress: Some(ThumbnailProgress { stopped: true, .. }),
                        ..
                    }
                ));
            } else {
                let plan = review(&f, None);
                assert_eq!(plan.thumbnail_review.as_ref().unwrap().eligible_files, 0);
            }
            assert!(f.paths[0].exists());
        }
    }
    #[test]
    fn thumbnail_cleanup_postcommit_reset_failure_keeps_confirmed_record_count() {
        let f = fixture(1);
        let bytes = fs::read(&f.paths[0]).unwrap();
        f.runtime
            .mutate_db(|db| {
                db.set_thumbnail(
                    &f.model.id,
                    &format!("{:x}", Sha256::digest(&bytes)),
                    f.paths[0].strip_prefix(&f.runtime.root).unwrap(),
                )
            })
            .unwrap();
        let plan = f
            .runtime
            .review_thumbnail_cleanup(
                &f.ctx,
                ThumbnailCleanupRequest {
                    mode: Mode::ResetDefaults,
                    continuation: None,
                },
            )
            .unwrap();
        let path = f.runtime.root.join("thumbnails");
        let previous = f.runtime.root.join("previous-previews");
        let preserved = previous.join(f.paths[0].file_name().unwrap());
        *f.runtime
            .thumbnail_cleanup
            .after_reset_commit
            .lock()
            .unwrap() = Some(Box::new(move || {
            fs::rename(&path, previous).unwrap();
            fs::create_dir(path).unwrap();
        }));
        assert!(matches!(
            f.runtime.execute_action(&f.ctx, &plan.token).unwrap(),
            ManagementResult::CleanupCompleted {
                files: 0,
                thumbnail_progress: Some(ThumbnailProgress {
                    default_records_cleared: 1,
                    stopped: true,
                    ..
                }),
                ..
            }
        ));
        assert_eq!(fs::read(preserved).unwrap(), bytes);
        assert_eq!(
            f.runtime
                .read_db(|db| db.default_thumbnail_reset_snapshot())
                .unwrap()
                .0,
            0
        );
    }
    #[test]
    fn thumbnail_cleanup_after_first_retire_root_or_quota_fault_reports_prior_counts() {
        for quota in [false, true] {
            let f = fixture(3);
            let plan = review(&f, None);
            let weak = Arc::downgrade(&f.runtime);
            let path = f.runtime.root.join("thumbnails");
            let previous = f.runtime.root.join("previous-previews");
            *f.runtime.thumbnail_cleanup.after_retire.lock().unwrap() = Some(Box::new(move || {
                if quota {
                    weak.upgrade()
                        .unwrap()
                        .temp_limits
                        .lock()
                        .unwrap()
                        .owned_bytes = 0;
                } else {
                    fs::rename(&path, previous).unwrap();
                    fs::create_dir(path).unwrap();
                }
            }));
            assert!(matches!(
                f.runtime.execute_action(&f.ctx, &plan.token).unwrap(),
                ManagementResult::CleanupCompleted {
                    files: 1,
                    thumbnail_progress: Some(ThumbnailProgress { stopped: true, .. }),
                    ..
                }
            ));
        }
    }
    #[test]
    fn thumbnail_cleanup_deadline_keeps_completed_unit_and_continues_before_next() {
        let f = fixture(2);
        let flag = f.runtime.thumbnail_cleanup.force_deadline.clone();
        *f.runtime.thumbnail_cleanup.after_render.lock().unwrap() =
            Some(Box::new(move || flag.store(true, Ordering::Release)));
        let plan = review(&f, None);
        let summary = plan.thumbnail_review.as_ref().unwrap();
        assert_eq!(summary.eligible_files, 1);
        assert!(summary.partial);
        assert!(summary.has_more);
        let continuation = summary.continuation.clone().unwrap();
        f.runtime
            .thumbnail_cleanup
            .force_deadline
            .store(false, Ordering::Release);
        assert!(matches!(
            f.runtime.execute_action(&f.ctx, &plan.token).unwrap(),
            ManagementResult::CleanupCompleted { files: 1, .. }
        ));
        assert_eq!(
            review(&f, Some(continuation))
                .thumbnail_review
                .as_ref()
                .unwrap()
                .eligible_files,
            1
        );
    }
    #[test]
    fn thumbnail_cleanup_uncertain_source_copy_audit_keeps_png_proofs_and_cursor() {
        let f = fixture(6);
        let foreign = f._temp.path().join("unknown-source-cache");
        fs::create_dir(&foreign).unwrap();
        std::os::unix::fs::symlink(&foreign, f.runtime.root.join("indexed-cache")).unwrap();
        let plan = review(&f, None);
        let summary = plan.thumbnail_review.as_ref().unwrap();
        assert_eq!(summary.eligible_files, 4);
        assert!(summary.partial);
        assert_eq!(summary.remaining_files, None);
        let continuation = summary.continuation.clone().unwrap();
        assert!(matches!(
            f.runtime.execute_action(&f.ctx, &plan.token).unwrap(),
            ManagementResult::CleanupCompleted { files: 4, .. }
        ));
        assert!(f.runtime.root.join("indexed-cache").is_symlink());
        let next = review(&f, Some(continuation));
        let summary = next.thumbnail_review.as_ref().unwrap();
        assert_eq!(summary.eligible_files, 2);
        assert!(summary.partial);
        assert_eq!(summary.remaining_files, None);
    }
    #[test]
    fn thumbnail_cleanup_reset_fallback_budget_is_admitted_before_any_row_mutation() {
        let f = fixture(1);
        let bytes = fs::read(&f.paths[0]).unwrap();
        f.runtime
            .mutate_db(|db| {
                db.set_thumbnail(
                    &f.model.id,
                    &format!("{:x}", Sha256::digest(&bytes)),
                    f.paths[0].strip_prefix(&f.runtime.root).unwrap(),
                )
            })
            .unwrap();
        let plan = f
            .runtime
            .review_thumbnail_cleanup(
                &f.ctx,
                ThumbnailCleanupRequest {
                    mode: Mode::ResetDefaults,
                    continuation: None,
                },
            )
            .unwrap();
        let _full = f
            .runtime
            .thumbnail_cleanup
            .reserve(64 * 1024 * 1024, 0)
            .unwrap();
        assert_eq!(
            f.runtime
                .execute_action(&f.ctx, &plan.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        assert_eq!(
            f.runtime
                .read_db(|db| db.default_thumbnail_reset_snapshot())
                .unwrap()
                .0,
            1
        );
        assert_eq!(fs::read(&f.paths[0]).unwrap(), bytes);
    }
    #[test]
    fn thumbnail_cleanup_continuation_ttl_starts_at_successful_activation() {
        let f = fixture(6);
        let plan = review(&f, None);
        let token = plan
            .thumbnail_review
            .as_ref()
            .unwrap()
            .continuation
            .clone()
            .unwrap();
        {
            let mut plans = f.runtime.actions.lock().unwrap();
            let planned = plans.get_mut(&plan.token).unwrap();
            planned
                .thumbnail_cleanup
                .as_mut()
                .unwrap()
                .next
                .as_mut()
                .unwrap()
                .1
                .expires = SystemTime::UNIX_EPOCH;
        }
        let before = SystemTime::now();
        f.runtime.execute_action(&f.ctx, &plan.token).unwrap();
        let after = SystemTime::now();
        let activated = f
            .runtime
            .thumbnail_cleanup
            .cursors
            .lock()
            .unwrap()
            .get(&token)
            .unwrap()
            .expires;
        assert!(activated >= before + TTL);
        assert!(activated <= after + TTL);
        let next = review(&f, Some(token));
        assert_eq!(next.thumbnail_review.as_ref().unwrap().eligible_files, 2);
        f.runtime
            .actions
            .lock()
            .unwrap()
            .get_mut(&next.token)
            .unwrap()
            .expires = SystemTime::UNIX_EPOCH;
        assert_eq!(
            f.runtime
                .execute_action(&f.ctx, &next.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
        assert_eq!(f.paths.iter().filter(|path| path.exists()).count(), 2);
    }
    #[test]
    fn thumbnail_cleanup_preserves_inactive_custom_alias_to_generated_png() {
        for separator in ["/./", "//", "/"] {
            let f = fixture(1);
            let bytes = fs::read(&f.paths[0]).unwrap();
            let hash = format!("{:x}", Sha256::digest(&bytes));
            let filename = f.paths[0].file_name().unwrap().to_str().unwrap();
            let filename = if separator == "/" {
                filename.to_ascii_uppercase()
            } else {
                filename.to_owned()
            };
            let alias = format!("thumbnails{separator}{filename}");
            fs::write(f.runtime.root.join(&alias), &bytes).unwrap();
            let model = f
                .runtime
                .mutate_db(|db| {
                    db.add_custom_thumbnail(
                        &f.model.id,
                        f.model.metadata_version,
                        &hash,
                        Path::new(&alias),
                    )
                    .map(|(model, _)| model)
                })
                .unwrap();
            fs::write(f.runtime.root.join("thumbnails/custom-other.png"), &bytes).unwrap();
            f.runtime
                .mutate_db(|db| {
                    db.add_custom_thumbnail(
                        &model.id,
                        model.metadata_version,
                        &hash,
                        Path::new("thumbnails/custom-other.png"),
                    )
                    .map(|_| ())
                })
                .unwrap();
            let plan = review(&f, None);
            assert_eq!(plan.thumbnail_review.as_ref().unwrap().eligible_files, 0);
            assert!(matches!(
                f.runtime.execute_action(&f.ctx, &plan.token).unwrap(),
                ManagementResult::CleanupCompleted { files: 0, .. }
            ));
            assert_eq!(fs::read(&f.paths[0]).unwrap(), bytes);
        }
    }
}
