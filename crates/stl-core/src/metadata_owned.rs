//! Host-funded immutable path backing. This does not bound other parser allocations.

use std::alloc::{alloc, dealloc, Layout};
use std::borrow::Borrow;
use std::cell::RefCell;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::ptr::{self, NonNull};
use std::sync::atomic::{fence, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::zip_bounds::{ZipFundingBusy, ZipResourceObserver};
use crate::{ResourceStop, StlError};

/// Trusted bridge to the host's existing shared Owned funding authority.
///
/// Successful acquisition must reserve exactly `bytes` until `release`. Neither
/// callback may panic; poisoned or uncertain accounting must fail closed. Release
/// must not call back into this owner. No quota or default allowance is provided here.
pub trait HostOwnedFunding: Send + Sync {
    fn try_acquire(&self, bytes: usize) -> Result<(), ZipFundingBusy>;
    fn release(&self, bytes: usize);
}

/// A cheap allocation handle sharing the context's existing observer and host.
/// Cloning it neither allocates a new allowance nor resets a processing cause.
#[derive(Clone)]
pub struct PathAllocator {
    host: Option<Arc<dyn HostOwnedFunding>>,
    observer: ZipResourceObserver,
}

impl PathAllocator {
    pub(crate) fn new(
        observer: ZipResourceObserver,
        host: Option<Arc<dyn HostOwnedFunding>>,
    ) -> Self {
        Self { host, observer }
    }

    pub fn copy_path(&self, text: &str) -> Result<SharedPath, StlError> {
        self.copy_path_using(text, CopyControl::default())
    }

    fn copy_path_using(&self, text: &str, control: CopyControl) -> Result<SharedPath, StlError> {
        self.observer.check()?;
        let (layout, offset) = match path_layout(text.len()) {
            Some(layout) => layout,
            None => {
                let _ = self.observer.stop_io(ResourceStop::InputLimitExceeded);
                return Err(self.recorded_error());
            }
        };
        control.stage(CopyStage::BeforeAcquire, &self.observer);
        self.observer.check()?;
        let credit = control.attach_probe(self.acquire_layout(layout)?);
        control.stage(CopyStage::AfterAcquire, &self.observer);
        self.observer.check()?;
        // Layout includes the entire header and payload, and is never zero-sized.
        let raw = control.allocate(layout);
        let Some(pointer) = NonNull::new(raw.cast::<PathHeader>()) else {
            let _ = self.observer.allocation_failed_io();
            return Err(self.recorded_error());
        };
        // Initialize the only fields read by Drop before any later stop can return.
        // The local owner cannot expose its uninitialized payload before copying it.
        unsafe {
            pointer.as_ptr().write(PathHeader {
                strong: AtomicUsize::new(1),
                len: text.len(),
                credit,
            });
        }
        let owned = SharedPath { pointer };
        control.stage(CopyStage::AfterAllocate, &self.observer);
        self.observer.check()?;
        control.stage(CopyStage::BeforeCopy, &self.observer);
        self.observer.check()?;
        // The source is valid UTF-8 and disjoint from this newly allocated block.
        unsafe {
            ptr::copy_nonoverlapping(text.as_ptr(), raw.add(offset), text.len());
        }
        control.copied();
        control.stage(CopyStage::AfterCopy, &self.observer);
        self.observer.check()?;
        Ok(owned)
    }

    pub(crate) fn check(&self) -> Result<(), StlError> {
        self.observer.check().map_err(Into::into)
    }

    pub(crate) fn acquire_layout(&self, layout: Layout) -> Result<AllocationCredit, StlError> {
        self.check()?;
        if let Some(host) = &self.host {
            if host.try_acquire(layout.size()).is_err() {
                let _ = self.observer.funding_busy_io();
                return Err(self.recorded_error());
            }
        }
        let credit = AllocationCredit {
            host: self.host.clone(),
            bytes: layout.size(),
            #[cfg(test)]
            probe: None,
        };
        self.check()?;
        Ok(credit)
    }

    pub(crate) fn input_error(&self) -> StlError {
        let _ = self.observer.stop_io(ResourceStop::InputLimitExceeded);
        self.recorded_error()
    }

    pub(crate) fn allocation_error(&self) -> StlError {
        let _ = self.observer.allocation_failed_io();
        self.recorded_error()
    }

    pub(crate) fn map_allocator(&self) -> MapAllocator {
        MapAllocator::new(self.host.clone(), self.observer.clone())
    }

    fn recorded_error(&self) -> StlError {
        self.observer
            .check()
            .expect_err("a terminal cause was recorded")
            .into()
    }
}

struct MapBlockHeader {
    block_layout: Layout,
    payload_offset: usize,
    credit: AllocationCredit,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
struct MapAllocationLayout {
    bucket: Layout,
    block: Layout,
    payload_offset: usize,
    returned_len: usize,
}

#[cfg(test)]
#[derive(Default)]
struct MapAllocatorProbe {
    layouts: std::sync::Mutex<Vec<MapAllocationLayout>>,
    fail_allocation: std::sync::atomic::AtomicBool,
    deallocated: std::sync::atomic::AtomicBool,
}

fn map_block_layout(bucket_layout: Layout) -> Option<(Layout, usize)> {
    let (layout, offset) = Layout::new::<MapBlockHeader>().extend(bucket_layout).ok()?;
    Some((layout.pad_to_align(), offset))
}

/// Hash table allocator whose bucket blocks share the package's existing Owned funding.
pub(crate) struct MapAllocator {
    host: RefCell<Option<Arc<dyn HostOwnedFunding>>>,
    observer: RefCell<Option<ZipResourceObserver>>,
    #[cfg(test)]
    probe: Option<Arc<MapAllocatorProbe>>,
}

impl MapAllocator {
    fn new(host: Option<Arc<dyn HostOwnedFunding>>, observer: ZipResourceObserver) -> Self {
        Self {
            host: RefCell::new(host),
            observer: RefCell::new(Some(observer)),
            #[cfg(test)]
            probe: None,
        }
    }

    #[cfg(test)]
    fn with_test_probe(
        host: Option<Arc<dyn HostOwnedFunding>>,
        observer: ZipResourceObserver,
        probe: Arc<MapAllocatorProbe>,
    ) -> Self {
        Self {
            host: RefCell::new(host),
            observer: RefCell::new(Some(observer)),
            probe: Some(probe),
        }
    }

    /// Detach parser state before an immutable object map can outlive its package.
    pub(crate) fn seal(&self) {
        self.observer.borrow_mut().take();
        self.host.borrow_mut().take();
    }
}

impl Clone for MapAllocator {
    fn clone(&self) -> Self {
        Self {
            host: RefCell::new(self.host.borrow().clone()),
            observer: RefCell::new(self.observer.borrow().clone()),
            #[cfg(test)]
            probe: self.probe.clone(),
        }
    }
}

unsafe impl allocator_api2::alloc::Allocator for MapAllocator {
    fn allocate(
        &self,
        bucket_layout: Layout,
    ) -> Result<NonNull<[u8]>, allocator_api2::alloc::AllocError> {
        use allocator_api2::alloc::AllocError;

        let observer = self.observer.borrow().clone().ok_or(AllocError)?;
        if observer.check().is_err() {
            return Err(AllocError);
        }
        let Some((block_layout, payload_offset)) = map_block_layout(bucket_layout) else {
            let _ = observer.stop_io(ResourceStop::InputLimitExceeded);
            return Err(AllocError);
        };
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe.layouts.lock().unwrap().push(MapAllocationLayout {
                bucket: bucket_layout,
                block: block_layout,
                payload_offset,
                returned_len: bucket_layout.size(),
            });
        }
        let host = self.host.borrow().clone();
        if let Some(host) = &host {
            if host.try_acquire(block_layout.size()).is_err() {
                let _ = observer.funding_busy_io();
                return Err(AllocError);
            }
        }
        let credit = AllocationCredit {
            host,
            bytes: block_layout.size(),
            #[cfg(test)]
            probe: None,
        };
        if observer.check().is_err() {
            drop(credit);
            return Err(AllocError);
        }
        #[cfg(test)]
        if self
            .probe
            .as_ref()
            .is_some_and(|probe| probe.fail_allocation.load(Ordering::SeqCst))
        {
            let _ = observer.allocation_failed_io();
            drop(credit);
            return Err(AllocError);
        }
        let raw = unsafe { alloc(block_layout) };
        let Some(base) = NonNull::new(raw) else {
            let _ = observer.allocation_failed_io();
            drop(credit);
            return Err(AllocError);
        };
        unsafe {
            base.cast::<MapBlockHeader>()
                .as_ptr()
                .write(MapBlockHeader {
                    block_layout,
                    payload_offset,
                    credit,
                });
        }
        let payload = unsafe { base.as_ptr().add(payload_offset) };
        let payload = NonNull::new(payload).expect("non-null map allocation remains non-null");
        Ok(NonNull::slice_from_raw_parts(payload, bucket_layout.size()))
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, bucket_layout: Layout) {
        let (expected_block_layout, expected_offset) =
            map_block_layout(bucket_layout).expect("hashbrown returns its original valid layout");
        let base = unsafe { ptr.as_ptr().sub(expected_offset) };
        let header = unsafe { base.cast::<MapBlockHeader>().read() };
        assert_eq!(header.block_layout, expected_block_layout);
        assert_eq!(header.payload_offset, expected_offset);
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe.deallocated.store(false, Ordering::SeqCst);
        }
        unsafe { dealloc(base, header.block_layout) };
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe.deallocated.store(true, Ordering::SeqCst);
        }
        header.credit.note_deallocated();
        drop(header.credit);
    }
}

pub(crate) struct AllocationCredit {
    host: Option<Arc<dyn HostOwnedFunding>>,
    bytes: usize,
    #[cfg(test)]
    probe: Option<Arc<CopyProbe>>,
}
impl AllocationCredit {
    pub(crate) fn note_deallocated(&self) {
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe.deallocated.store(true, Ordering::SeqCst);
        }
    }
}
impl Drop for AllocationCredit {
    fn drop(&mut self) {
        if let Some(host) = &self.host {
            host.release(self.bytes);
        }
    }
}

struct PathHeader {
    strong: AtomicUsize,
    len: usize,
    credit: AllocationCredit,
}

fn path_layout(len: usize) -> Option<(Layout, usize)> {
    let payload = Layout::array::<u8>(len).ok()?;
    let (layout, offset) = Layout::new::<PathHeader>().extend(payload).ok()?;
    Some((layout.pad_to_align(), offset))
}

/// Immutable UTF-8 path whose final owner frees its full block before refunding it.
/// External compatibility contexts use the same owner without a host debit.
pub struct SharedPath {
    pointer: NonNull<PathHeader>,
}

// Only immutable UTF-8 bytes, an atomic owner count and a Send+Sync host credit
// are shared. Publication happens after initialization, and mutation is private.
unsafe impl Send for SharedPath {}
unsafe impl Sync for SharedPath {}

impl SharedPath {
    pub fn as_str(&self) -> &str {
        // The constructor publishes only copied UTF-8. This owner pins the block.
        // A u8 payload needs no extra alignment after the complete header.
        unsafe {
            let header = self.pointer.as_ref();
            let bytes = std::slice::from_raw_parts(
                self.pointer
                    .as_ptr()
                    .cast::<u8>()
                    .add(std::mem::size_of::<PathHeader>()),
                header.len,
            );
            std::str::from_utf8_unchecked(bytes)
        }
    }
}

fn try_retain(strong: &AtomicUsize) -> bool {
    strong
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            count
                .checked_add(1)
                .filter(|next| *next <= isize::MAX as usize)
        })
        .is_ok()
}
impl Clone for SharedPath {
    fn clone(&self) -> Self {
        // A live caller owns a strong reference, so the header cannot be freed.
        if !try_retain(unsafe { &self.pointer.as_ref().strong }) {
            std::process::abort();
        }
        Self {
            pointer: self.pointer,
        }
    }
}
impl Drop for SharedPath {
    fn drop(&mut self) {
        // Each owner decrements exactly once. Acquire pairs with other releases
        // before the last owner destroys the immutable allocation.
        let header = unsafe { self.pointer.as_ref() };
        if header.strong.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        fence(Ordering::Acquire);
        let (layout, _) = path_layout(header.len).expect("constructed path layout remains valid");
        // Move the sole dropping field to the stack; all remaining header fields
        // are scalar/atomic. Never drop an already-deallocated header or payload.
        let credit = unsafe { ptr::read(&header.credit) };
        unsafe { dealloc(self.pointer.as_ptr().cast::<u8>(), layout) };
        credit.note_deallocated();
        drop(credit);
    }
}
impl Deref for SharedPath {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}
impl AsRef<str> for SharedPath {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}
impl Borrow<str> for SharedPath {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}
impl PartialEq for SharedPath {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}
impl Eq for SharedPath {}
impl Hash for SharedPath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}
impl fmt::Debug for SharedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SharedPath").field(&self.as_str()).finish()
    }
}
impl fmt::Display for SharedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyStage {
    BeforeAcquire,
    AfterAcquire,
    AfterAllocate,
    BeforeCopy,
    AfterCopy,
}

#[derive(Default)]
struct CopyControl {
    #[cfg(test)]
    probe: Option<Arc<CopyProbe>>,
}
impl CopyControl {
    #[cfg(test)]
    fn attach_probe(&self, mut credit: AllocationCredit) -> AllocationCredit {
        credit.probe = self.probe.clone();
        credit
    }
    #[cfg(not(test))]
    fn attach_probe(&self, credit: AllocationCredit) -> AllocationCredit {
        credit
    }
    fn stage(&self, stage: CopyStage, observer: &ZipResourceObserver) {
        #[cfg(not(test))]
        let _ = (stage, observer);
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            if let Some((at, failure)) = probe.stop {
                if at == stage {
                    failure.record(observer);
                }
            }
        }
    }
    fn allocate(&self, layout: Layout) -> *mut u8 {
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe.allocations.fetch_add(1, Ordering::SeqCst);
            if probe.fail_allocate {
                return ptr::null_mut();
            }
        }
        // path_layout guarantees a nonzero, valid layout.
        unsafe { alloc(layout) }
    }
    fn copied(&self) {
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe.copies.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
enum TestFailure {
    Input,
    Work,
    Changed,
    Allocation,
    Busy,
}
#[cfg(test)]
impl TestFailure {
    fn record(self, observer: &ZipResourceObserver) {
        let _ = match self {
            Self::Input => observer.stop_io(ResourceStop::InputLimitExceeded),
            Self::Work => observer.stop_io(ResourceStop::ReadWorkExceeded),
            Self::Changed => observer.stop_io(ResourceStop::EnvelopeChanged),
            Self::Allocation => observer.allocation_failed_io(),
            Self::Busy => observer.funding_busy_io(),
        };
    }
    fn error(self) -> StlError {
        match self {
            Self::Input => StlError::ResourceStopped(ResourceStop::InputLimitExceeded),
            Self::Work => StlError::ResourceStopped(ResourceStop::ReadWorkExceeded),
            Self::Changed => StlError::ResourceStopped(ResourceStop::EnvelopeChanged),
            Self::Allocation => StlError::AllocationFailed,
            Self::Busy => StlError::FundingBusy,
        }
    }
}
#[cfg(test)]
#[derive(Default)]
struct CopyProbe {
    allocations: AtomicUsize,
    copies: AtomicUsize,
    deallocated: std::sync::atomic::AtomicBool,
    fail_allocate: bool,
    stop: Option<(CopyStage, TestFailure)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip_bounds::ZipOpenError;
    use std::sync::Mutex;

    #[derive(Default)]
    struct State {
        active: usize,
        acquired: usize,
        released: usize,
    }
    struct Host {
        limit: usize,
        state: Mutex<State>,
        deallocated_before_release: Option<Arc<CopyProbe>>,
    }
    impl Host {
        fn new(limit: usize) -> Arc<Self> {
            Arc::new(Self {
                limit,
                state: Mutex::new(State::default()),
                deallocated_before_release: None,
            })
        }
    }
    impl HostOwnedFunding for Host {
        fn try_acquire(&self, bytes: usize) -> Result<(), ZipFundingBusy> {
            let mut state = self.state.lock().unwrap();
            state.acquired += 1;
            let next = state.active.checked_add(bytes).ok_or(ZipFundingBusy)?;
            if next > self.limit {
                return Err(ZipFundingBusy);
            }
            state.active = next;
            Ok(())
        }
        fn release(&self, bytes: usize) {
            if let Some(probe) = &self.deallocated_before_release {
                assert!(
                    probe.deallocated.load(Ordering::SeqCst),
                    "actual deallocation must precede the host callback"
                );
            }
            let mut state = self.state.lock().unwrap();
            state.active = state.active.checked_sub(bytes).unwrap();
            state.released += 1;
        }
    }
    fn allocator(host: &Arc<Host>, observer: ZipResourceObserver) -> PathAllocator {
        PathAllocator::new(observer, Some(host.clone()))
    }

    #[derive(Default)]
    struct MapHostState {
        active: usize,
        peak: usize,
        acquired: usize,
        released: usize,
        active_before_acquire: Vec<usize>,
    }

    struct MapHost {
        limit: usize,
        state: Mutex<MapHostState>,
        probe: Arc<MapAllocatorProbe>,
    }

    impl HostOwnedFunding for MapHost {
        fn try_acquire(&self, bytes: usize) -> Result<(), ZipFundingBusy> {
            let mut state = self.state.lock().unwrap();
            let active = state.active;
            state.active_before_acquire.push(active);
            state.acquired += 1;
            let next = state.active.checked_add(bytes).ok_or(ZipFundingBusy)?;
            if next > self.limit {
                return Err(ZipFundingBusy);
            }
            state.active = next;
            state.peak = state.peak.max(next);
            Ok(())
        }

        fn release(&self, bytes: usize) {
            if !self.probe.fail_allocation.load(Ordering::SeqCst) {
                assert!(
                    self.probe.deallocated.load(Ordering::SeqCst),
                    "map buckets must be physically deallocated before refund"
                );
            }
            let mut state = self.state.lock().unwrap();
            state.active = state.active.checked_sub(bytes).unwrap();
            state.released += 1;
        }
    }

    fn test_hash_map(
        host: &Arc<MapHost>,
        observer: ZipResourceObserver,
    ) -> hashbrown::HashMap<u32, u32, std::collections::hash_map::RandomState, MapAllocator> {
        let allocator =
            MapAllocator::with_test_probe(Some(host.clone()), observer, host.probe.clone());
        hashbrown::HashMap::with_hasher_in(
            std::collections::hash_map::RandomState::new(),
            allocator,
        )
    }

    #[test]
    fn map_rehash_admits_the_actual_complete_layout_while_old_buckets_remain_live() {
        let probe = Arc::new(MapAllocatorProbe::default());
        let host = Arc::new(MapHost {
            limit: usize::MAX,
            state: Mutex::new(MapHostState::default()),
            probe: probe.clone(),
        });
        let mut map = test_hash_map(&host, ZipResourceObserver::new());
        map.try_reserve(1).unwrap();
        let first_capacity = map.capacity();
        for key in 0..first_capacity as u32 {
            assert!(map.insert(key, key).is_none());
        }

        let first = probe.layouts.lock().unwrap()[0];
        let (expected_layout, expected_offset) = Layout::new::<MapBlockHeader>()
            .extend(first.bucket)
            .unwrap();
        assert_eq!(first.block, expected_layout.pad_to_align());
        assert_eq!(first.payload_offset, expected_offset);
        assert_eq!(first.returned_len, first.bucket.size());
        assert_eq!(host.state.lock().unwrap().active, first.block.size());

        map.try_reserve(1).unwrap();

        let layouts = probe.layouts.lock().unwrap();
        assert_eq!(layouts.len(), 2);
        let second = layouts[1];
        let state = host.state.lock().unwrap();
        assert_eq!(state.active_before_acquire[1], first.block.size());
        assert!(state.peak >= first.block.size() + second.block.size());
        assert_eq!(state.active, second.block.size());
        assert_eq!(state.released, 1);
        assert!(probe.deallocated.load(Ordering::SeqCst));
        drop(state);
        drop(layouts);
        drop(map);
        let state = host.state.lock().unwrap();
        assert_eq!((state.active, state.acquired, state.released), (0, 2, 2));
    }

    #[test]
    fn map_allocation_failure_refunds_without_a_physical_free() {
        let probe = Arc::new(MapAllocatorProbe::default());
        probe.fail_allocation.store(true, Ordering::SeqCst);
        let host = Arc::new(MapHost {
            limit: usize::MAX,
            state: Mutex::new(MapHostState::default()),
            probe: probe.clone(),
        });
        let observer = ZipResourceObserver::new();
        let mut map = test_hash_map(&host, observer.clone());
        assert!(map.try_reserve(1).is_err());
        assert!(matches!(
            observer.check(),
            Err(ZipOpenError::AllocationFailed)
        ));
        assert!(!probe.deallocated.load(Ordering::SeqCst));
        let state = host.state.lock().unwrap();
        assert_eq!((state.active, state.acquired, state.released), (0, 1, 1));
        assert_eq!(map.capacity(), 0);
    }
    fn probed(probe: &Arc<CopyProbe>) -> CopyControl {
        CopyControl {
            probe: Some(probe.clone()),
        }
    }
    const FAILURES: [TestFailure; 5] = [
        TestFailure::Input,
        TestFailure::Work,
        TestFailure::Changed,
        TestFailure::Allocation,
        TestFailure::Busy,
    ];

    #[test]
    fn whole_layout_admission_precedes_allocation_and_copy() {
        let host = Host::new(path_layout(4).unwrap().0.size() - 1);
        let observer = ZipResourceObserver::new();
        let probe = Arc::new(CopyProbe::default());
        let error = allocator(&host, observer.clone())
            .copy_path_using("path", probed(&probe))
            .unwrap_err();
        assert_eq!(error, StlError::FundingBusy);
        assert!(matches!(
            observer.check(),
            Err(crate::zip_bounds::ZipOpenError::FundingBusy)
        ));
        assert_eq!(probe.allocations.load(Ordering::SeqCst), 0);
        assert_eq!(probe.copies.load(Ordering::SeqCst), 0);
        let state = host.state.lock().unwrap();
        assert_eq!((state.active, state.acquired, state.released), (0, 1, 0));
    }

    #[test]
    fn null_allocation_refunds_without_touching_uninitialized_header() {
        let host = Host::new(usize::MAX);
        let observer = ZipResourceObserver::new();
        let probe = Arc::new(CopyProbe {
            fail_allocate: true,
            ..Default::default()
        });
        let error = allocator(&host, observer)
            .copy_path_using("path", probed(&probe))
            .unwrap_err();
        assert_eq!(error, StlError::AllocationFailed);
        assert_eq!(probe.allocations.load(Ordering::SeqCst), 1);
        assert_eq!(probe.copies.load(Ordering::SeqCst), 0);
        assert!(!probe.deallocated.load(Ordering::SeqCst));
        let state = host.state.lock().unwrap();
        assert_eq!((state.active, state.acquired, state.released), (0, 1, 1));
    }

    #[test]
    fn shared_backing_outlives_allocator_and_frees_before_refund() {
        let probe = Arc::new(CopyProbe::default());
        let host = Arc::new(Host {
            limit: usize::MAX,
            state: Mutex::new(State::default()),
            deallocated_before_release: Some(probe.clone()),
        });
        let factory = allocator(&host, ZipResourceObserver::new());
        let path = factory
            .copy_path_using("3D/🧪.model", probed(&probe))
            .unwrap();
        let cloned = path.clone();
        assert_eq!(path.as_str().as_ptr(), cloned.as_str().as_ptr());
        assert_eq!(host.state.lock().unwrap().acquired, 1);
        assert_eq!(
            host.state.lock().unwrap().active,
            path_layout(path.len()).unwrap().0.size()
        );
        drop(factory);
        drop(path);
        assert!(!probe.deallocated.load(Ordering::SeqCst));
        std::thread::spawn(move || {
            assert_eq!(cloned.as_str(), "3D/🧪.model");
            drop(cloned);
        })
        .join()
        .unwrap();
        assert!(probe.deallocated.load(Ordering::SeqCst));
        let state = host.state.lock().unwrap();
        assert_eq!((state.active, state.acquired, state.released), (0, 1, 1));
    }

    #[test]
    fn concurrent_clones_and_drops_keep_the_original_block_funded() {
        let probe = Arc::new(CopyProbe::default());
        let host = Arc::new(Host {
            limit: usize::MAX,
            state: Mutex::new(State::default()),
            deallocated_before_release: Some(probe.clone()),
        });
        let path = allocator(&host, ZipResourceObserver::new())
            .copy_path_using("3D/shared.model", probed(&probe))
            .unwrap();
        let address = path.as_str().as_ptr() as usize;
        let barrier = std::sync::Barrier::new(9);
        std::thread::scope(|scope| {
            let handles = (0..8)
                .map(|_| {
                    let path = &path;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let held = path.clone();
                        barrier.wait();
                        let mut same = true;
                        for _ in 0..128 {
                            let cloned = held.clone();
                            same &= cloned.as_str().as_ptr() as usize == address;
                            same &= cloned.as_str() == "3D/shared.model";
                            drop(cloned);
                        }
                        drop(held);
                        same
                    })
                })
                .collect::<Vec<_>>();
            barrier.wait();
            for handle in handles {
                assert!(handle.join().unwrap());
            }
        });
        assert!(!probe.deallocated.load(Ordering::SeqCst));
        {
            let state = host.state.lock().unwrap();
            assert_eq!((state.acquired, state.released), (1, 0));
            assert!(state.active > 0);
        }
        drop(path);
        assert!(probe.deallocated.load(Ordering::SeqCst));
        let state = host.state.lock().unwrap();
        assert_eq!((state.active, state.acquired, state.released), (0, 1, 1));
    }

    #[test]
    fn pending_causes_and_all_acquisition_stages_preserve_first_cause() {
        for first in FAILURES {
            let host = Host::new(usize::MAX);
            let observer = ZipResourceObserver::new();
            first.record(&observer);
            let probe = Arc::new(CopyProbe::default());
            assert_eq!(
                allocator(&host, observer)
                    .copy_path_using("path", probed(&probe))
                    .unwrap_err(),
                first.error()
            );
            assert_eq!(probe.allocations.load(Ordering::SeqCst), 0);
            assert_eq!(host.state.lock().unwrap().acquired, 0);
            for stage in [
                CopyStage::BeforeAcquire,
                CopyStage::AfterAcquire,
                CopyStage::AfterAllocate,
                CopyStage::BeforeCopy,
                CopyStage::AfterCopy,
            ] {
                let host = Host::new(usize::MAX);
                let observer = ZipResourceObserver::new();
                let probe = Arc::new(CopyProbe {
                    stop: Some((stage, first)),
                    ..Default::default()
                });
                assert_eq!(
                    allocator(&host, observer.clone())
                        .copy_path_using("path", probed(&probe))
                        .unwrap_err(),
                    first.error()
                );
                TestFailure::Busy.record(&observer);
                assert_eq!(StlError::from(observer.check().unwrap_err()), first.error());
                assert_eq!(host.state.lock().unwrap().active, 0);
                assert_eq!(
                    probe.allocations.load(Ordering::SeqCst),
                    usize::from(matches!(
                        stage,
                        CopyStage::AfterAllocate | CopyStage::BeforeCopy | CopyStage::AfterCopy
                    ))
                );
                assert_eq!(
                    probe.copies.load(Ordering::SeqCst),
                    usize::from(stage == CopyStage::AfterCopy)
                );
            }
        }
    }

    #[test]
    fn contexts_split_reader_and_allocator_without_resetting_host_or_observer() {
        use std::io::{Cursor, Read};
        let host = Host::new(usize::MAX);
        let observer = ZipResourceObserver::new();
        let mut context = crate::package_bounds::PackageProcessingContext::new_funded(
            observer.clone(),
            host.clone(),
        );
        let factory = context.path_allocator();
        let mut reader = context.model_reader(Cursor::new(b"abc"));
        let path = factory.copy_path("3D/root.model").unwrap();
        let mut bytes = [0; 3];
        reader.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"abc");
        let _ = observer.funding_busy_io();
        assert!(ZipFundingBusy::from_io_error(&reader.read(&mut [0; 1]).unwrap_err()).is_some());
        drop(reader);
        assert_eq!(context.usage().inflation_bytes, 3);
        assert_eq!(
            context.copy_path("never copied").unwrap_err(),
            StlError::FundingBusy
        );
        drop(context);
        drop(factory);
        assert!(host.state.lock().unwrap().active > 0);
        drop(path);
        assert_eq!(host.state.lock().unwrap().active, 0);
    }

    #[test]
    fn independent_operations_contend_for_the_same_host_capacity() {
        let host = Host::new(path_layout(4).unwrap().0.size());
        let first = allocator(&host, ZipResourceObserver::new())
            .copy_path("path")
            .unwrap();
        let denied = allocator(&host, ZipResourceObserver::new());
        assert_eq!(denied.copy_path("path").unwrap_err(), StlError::FundingBusy);
        drop(first);
        assert_eq!(denied.copy_path("path").unwrap_err(), StlError::FundingBusy);
        let next = allocator(&host, ZipResourceObserver::new())
            .copy_path("path")
            .unwrap();
        drop(next);
        let state = host.state.lock().unwrap();
        assert_eq!((state.active, state.acquired, state.released), (0, 3, 2));
    }

    #[test]
    fn layout_boundaries_and_external_mode_have_no_hidden_funding() {
        let (empty, offset) = path_layout(0).unwrap();
        assert_eq!(offset, std::mem::size_of::<PathHeader>());
        assert!(empty.size() >= offset);
        assert!(path_layout(usize::MAX).is_none());
        let maximum_header = isize::MAX as usize - offset;
        assert!(path_layout(maximum_header + 1).is_none());
        let observer = ZipResourceObserver::new();
        let factory = PathAllocator::new(observer, None);
        let empty_path = factory.copy_path("").unwrap();
        assert!(empty_path.is_empty());
        let mut map = std::collections::HashMap::new();
        map.insert(factory.copy_path("same").unwrap(), 1);
        assert_eq!(map.get("same"), Some(&1));
        assert_eq!(
            factory.copy_path("same").unwrap(),
            factory.copy_path("same").unwrap()
        );
        let saturated = AtomicUsize::new(isize::MAX as usize);
        assert!(!try_retain(&saturated));
        assert_eq!(saturated.load(Ordering::Relaxed), isize::MAX as usize);
    }
}
