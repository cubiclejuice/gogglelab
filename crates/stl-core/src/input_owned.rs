//! Unique private input buffers funded before acquiring their complete backing.

use std::alloc::{alloc, dealloc, Layout};
use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;
use std::ptr::{self, NonNull};

use crate::metadata_owned::{AllocationCredit, PathAllocator};
use crate::StlError;

/// The private supported input types must have nonzero size.
pub(crate) trait InputElement: Sized {
    const NON_ZERO: () = assert!(std::mem::size_of::<Self>() > 0);
}
impl InputElement for [f32; 3] {}
impl InputElement for [u32; 3] {}

/// Read-only selection of an actual private input element type.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputBufferKind {
    Vertices,
    Triangles,
    Components,
    Build,
}

/// Actual compiled layout terms; inspecting these grants no funding or allowance.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputBufferLayoutProfile {
    pub element_size: usize,
    pub element_align: usize,
    pub header_size: usize,
    pub header_align: usize,
    pub capacity: usize,
    pub payload_offset: usize,
    pub allocation_size: usize,
    pub allocation_align: usize,
}

struct Header {
    len: usize,
    capacity: usize,
    credit: AllocationCredit,
}

fn layout<T: InputElement>(capacity: usize) -> Option<(Layout, usize)> {
    let () = T::NON_ZERO;
    if capacity == 0 {
        return None;
    }
    let payload = Layout::array::<T>(capacity).ok()?;
    let (layout, offset) = Layout::new::<Header>().extend(payload).ok()?;
    Some((layout.pad_to_align(), offset))
}

pub(crate) fn input_buffer_layout<T: InputElement>(
    capacity: usize,
) -> Option<InputBufferLayoutProfile> {
    let (allocation, offset) = layout::<T>(capacity)?;
    Some(InputBufferLayoutProfile {
        element_size: std::mem::size_of::<T>(),
        element_align: std::mem::align_of::<T>(),
        header_size: std::mem::size_of::<Header>(),
        header_align: std::mem::align_of::<Header>(),
        capacity,
        payload_offset: offset,
        allocation_size: allocation.size(),
        allocation_align: allocation.align(),
    })
}

/// Owns exactly one initialized prefix. It cannot clone or export an uncharged Vec.
pub(crate) struct OwnedInputBuffer<T: InputElement> {
    block: Option<NonNull<Header>>,
    _owns: PhantomData<T>,
}

// A unique owner transfers its initialized T values. Shared borrows expose only
// &[T]; the allocation credit's host is Send+Sync and has no Runtime reference.
unsafe impl<T: InputElement + Send> Send for OwnedInputBuffer<T> {}
unsafe impl<T: InputElement + Sync> Sync for OwnedInputBuffer<T> {}

impl<T: InputElement> OwnedInputBuffer<T> {
    pub(crate) fn empty() -> Self {
        let () = T::NON_ZERO;
        Self {
            block: None,
            _owns: PhantomData,
        }
    }
    pub(crate) fn len(&self) -> usize {
        self.block.map_or(0, |block| unsafe { block.as_ref().len })
    }
    pub(crate) fn capacity(&self) -> usize {
        self.block
            .map_or(0, |block| unsafe { block.as_ref().capacity })
    }
    pub(crate) fn as_slice(&self) -> &[T] {
        match self.block {
            None => &[],
            Some(block) => unsafe {
                let header = block.as_ref();
                std::slice::from_raw_parts(payload::<T>(block), header.len)
            },
        }
    }
    pub(crate) fn reserve_capacity(
        &mut self,
        target: usize,
        allocator: &PathAllocator,
    ) -> Result<(), StlError> {
        self.reserve_using(target, allocator, |_| {}, ReserveControl::default())
    }
    #[cfg(test)]
    pub(crate) fn reserve_capacity_with_hook(
        &mut self,
        target: usize,
        allocator: &PathAllocator,
        hook: impl FnMut(InputReserveStage),
    ) -> Result<(), StlError> {
        self.reserve_using(target, allocator, hook, ReserveControl::default())
    }
    fn reserve_using(
        &mut self,
        target: usize,
        allocator: &PathAllocator,
        mut hook: impl FnMut(InputReserveStage),
        control: ReserveControl,
    ) -> Result<(), StlError> {
        allocator.check()?;
        let len = self.len();
        let capacity = self.capacity();
        if len > capacity || target < len {
            return Err(allocator.input_error());
        }
        if target <= capacity {
            return Ok(());
        }
        let (new_layout, offset) = layout::<T>(target).ok_or_else(|| allocator.input_error())?;
        // Old backing remains fully charged throughout new admission/acquisition.
        let credit = allocator.acquire_layout(new_layout)?;
        hook(InputReserveStage::AfterAcquire);
        allocator.check()?;
        let raw = control.allocate(new_layout);
        let Some(block) = NonNull::new(raw.cast::<Header>()) else {
            return Err(allocator.allocation_error());
        };
        // The provisional block owns no T values, even after raw bytes are copied.
        unsafe {
            block.as_ptr().write(Header {
                len: 0,
                capacity: target,
                credit,
            });
        }
        let mut provisional = Self {
            block: Some(block),
            _owns: PhantomData,
        };
        hook(InputReserveStage::AfterAllocate);
        allocator.check()?;
        if let Some(old) = self.block {
            // Disjoint live allocations; only the old initialized prefix is copied.
            unsafe {
                ptr::copy_nonoverlapping(payload::<T>(old), raw.add(offset).cast::<T>(), len);
            }
        }
        hook(InputReserveStage::AfterCopyBeforeCommit);
        allocator.check()?;
        // No callback, allocation, check or T destructor may interrupt this transfer.
        unsafe {
            if let Some(old) = self.block {
                (*old.as_ptr()).len = 0;
            }
            (*block.as_ptr()).len = len;
        }
        std::mem::swap(self, &mut provisional);
        drop(provisional);
        hook(InputReserveStage::Committed);
        allocator.check()?;
        Ok(())
    }
    pub(crate) fn push_reserved(&mut self, value: T) {
        assert!(
            self.len() < self.capacity(),
            "input append needs admitted capacity"
        );
        let block = self.block.expect("nonempty capacity has backing");
        // Compute the payload before writing header fields; no exclusive header
        // reference overlaps the shared header inspection in payload().
        let len = self.len();
        unsafe {
            let data = payload::<T>(block);
            data.add(len).write(value);
            ptr::addr_of_mut!((*block.as_ptr()).len).write(len + 1);
        }
    }
}
impl<T: InputElement> Default for OwnedInputBuffer<T> {
    fn default() -> Self {
        Self::empty()
    }
}
impl<T: InputElement> Deref for OwnedInputBuffer<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}
impl<'a, T: InputElement> IntoIterator for &'a OwnedInputBuffer<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}
impl<T: InputElement + fmt::Debug> fmt::Debug for OwnedInputBuffer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_slice().fmt(f)
    }
}
impl<T: InputElement> Drop for OwnedInputBuffer<T> {
    fn drop(&mut self) {
        if let Some(block) = self.block.take() {
            drop_block::<T, _>(block, || {});
        }
    }
}

unsafe fn payload<T: InputElement>(block: NonNull<Header>) -> *mut T {
    // Every constructed capacity was layout-checked and is immutable thereafter.
    let capacity = unsafe { block.as_ref().capacity };
    let (_, offset) = layout::<T>(capacity).expect("constructed buffer layout remains valid");
    unsafe { block.as_ptr().cast::<u8>().add(offset).cast::<T>() }
}

struct RawBlockGuard<F: FnOnce()> {
    block: NonNull<Header>,
    layout: Layout,
    after_free: Option<F>,
    credit: AllocationCredit,
}
impl<F: FnOnce()> Drop for RawBlockGuard<F> {
    fn drop(&mut self) {
        // The header credit was moved to this guard; no header field is dropped twice.
        unsafe { dealloc(self.block.as_ptr().cast::<u8>(), self.layout) };
        self.credit.note_deallocated();
        if let Some(after_free) = self.after_free.take() {
            after_free();
        }
        // credit is declared last and drops after the actual allocation is gone.
    }
}
fn drop_block<T: InputElement, F: FnOnce()>(block: NonNull<Header>, after_free: F) {
    // Install the raw guard before any T destructor can unwind. Slice drop glue
    // owns the initialized prefix; uninitialized capacity is never exposed/dropped.
    unsafe {
        let capacity = ptr::addr_of!((*block.as_ptr()).capacity).read();
        let len = ptr::addr_of!((*block.as_ptr()).len).read();
        let (layout, _) = layout::<T>(capacity).expect("constructed buffer layout remains valid");
        let initialized = std::ptr::slice_from_raw_parts_mut(payload::<T>(block), len);
        ptr::addr_of_mut!((*block.as_ptr()).len).write(0);
        let guard = RawBlockGuard {
            block,
            layout,
            after_free: Some(after_free),
            credit: ptr::addr_of!((*block.as_ptr()).credit).read(),
        };
        ptr::drop_in_place(initialized);
        drop(guard);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputReserveStage {
    AfterAcquire,
    AfterAllocate,
    AfterCopyBeforeCommit,
    Committed,
}

#[derive(Default)]
struct ReserveControl {
    #[cfg(test)]
    fail_allocate: bool,
}
impl ReserveControl {
    fn allocate(&self, layout: Layout) -> *mut u8 {
        #[cfg(test)]
        if self.fail_allocate {
            return ptr::null_mut();
        }
        // Complete checked header+payload layout, never a zero-size request.
        unsafe { alloc(layout) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata_owned::HostOwnedFunding;
    use crate::zip_bounds::{ZipFundingBusy, ZipResourceObserver};
    use crate::ResourceStop;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct FundingState {
        active: usize,
        requests: Vec<usize>,
        releases: usize,
    }
    struct Host {
        limit: AtomicUsize,
        state: Mutex<FundingState>,
        require_freed: Option<Arc<AtomicBool>>,
    }
    impl Host {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                limit: AtomicUsize::new(usize::MAX),
                state: Mutex::new(FundingState::default()),
                require_freed: None,
            })
        }
    }
    impl HostOwnedFunding for Host {
        fn try_acquire(&self, bytes: usize) -> Result<(), ZipFundingBusy> {
            let mut state = self.state.lock().unwrap();
            state.requests.push(bytes);
            let active = state.active.checked_add(bytes).ok_or(ZipFundingBusy)?;
            if active > self.limit.load(Ordering::SeqCst) {
                return Err(ZipFundingBusy);
            }
            state.active = active;
            Ok(())
        }
        fn release(&self, bytes: usize) {
            if let Some(freed) = &self.require_freed {
                assert!(freed.load(Ordering::SeqCst));
            }
            let mut state = self.state.lock().unwrap();
            state.active = state.active.checked_sub(bytes).unwrap();
            state.releases += 1;
        }
    }
    fn factory(host: &Arc<Host>) -> (PathAllocator, ZipResourceObserver) {
        let observer = ZipResourceObserver::new();
        let context = crate::package_bounds::PackageProcessingContext::new_funded(
            observer.clone(),
            host.clone(),
        );
        (context.path_allocator(), observer)
    }
    struct Value {
        id: usize,
        drops: Arc<[AtomicUsize; 3]>,
        panic_on_drop: bool,
    }
    impl InputElement for Value {}
    impl Drop for Value {
        fn drop(&mut self) {
            let previous = self.drops[self.id].fetch_add(1, Ordering::SeqCst);
            if self.panic_on_drop && previous == 0 {
                panic!("intentional element drop failure");
            }
        }
    }
    fn counts() -> Arc<[AtomicUsize; 3]> {
        Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)))
    }
    fn values(factory: &PathAllocator, drops: &Arc<[AtomicUsize; 3]>) -> OwnedInputBuffer<Value> {
        let mut buffer = OwnedInputBuffer::empty();
        buffer.reserve_capacity(2, factory).unwrap();
        for id in 0..2 {
            buffer.push_reserved(Value {
                id,
                drops: drops.clone(),
                panic_on_drop: false,
            });
        }
        buffer
    }
    #[derive(Clone, Copy)]
    enum Cause {
        Input,
        Work,
        Changed,
        Allocation,
        Busy,
    }
    impl Cause {
        fn stop(self, observer: &ZipResourceObserver) {
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

    #[test]
    fn old_and_full_new_layouts_must_fit_together_before_growth() {
        let host = Host::new();
        let (factory, _) = factory(&host);
        let drops = counts();
        let mut buffer = values(&factory, &drops);
        let pointer = buffer.as_slice().as_ptr();
        let old = input_buffer_layout::<Value>(2).unwrap().allocation_size;
        let new = input_buffer_layout::<Value>(4).unwrap().allocation_size;
        host.limit.store(old + new - 1, Ordering::SeqCst);
        assert_eq!(
            buffer.reserve_capacity(4, &factory),
            Err(StlError::FundingBusy)
        );
        assert_eq!(buffer.as_slice().as_ptr(), pointer);
        assert_eq!((buffer.len(), buffer.capacity()), (2, 2));
        assert!(drops.iter().all(|count| count.load(Ordering::SeqCst) == 0));
        {
            let state = host.state.lock().unwrap();
            assert_eq!(state.requests, [old, new]);
            assert_eq!((state.active, state.releases), (old, 0));
        }
        drop(buffer);
        assert_eq!(host.state.lock().unwrap().active, 0);
        assert_eq!(drops[0].load(Ordering::SeqCst), 1);
        assert_eq!(drops[1].load(Ordering::SeqCst), 1);
    }

    #[test]
    fn null_new_backing_refunds_only_new_credit_and_keeps_old_values() {
        let host = Host::new();
        let (factory, _) = factory(&host);
        let drops = counts();
        let mut buffer = values(&factory, &drops);
        let pointer = buffer.as_slice().as_ptr();
        let old = host.state.lock().unwrap().active;
        let error = buffer.reserve_using(
            4,
            &factory,
            |_| {},
            ReserveControl {
                fail_allocate: true,
            },
        );
        assert_eq!(error, Err(StlError::AllocationFailed));
        assert_eq!(buffer.as_slice().as_ptr(), pointer);
        assert_eq!((buffer.len(), buffer.capacity()), (2, 2));
        assert_eq!(host.state.lock().unwrap().active, old);
        assert_eq!(host.state.lock().unwrap().releases, 1);
        assert!(drops.iter().all(|count| count.load(Ordering::SeqCst) == 0));
        drop(buffer);
        assert_eq!(host.state.lock().unwrap().active, 0);
        assert_eq!(drops[0].load(Ordering::SeqCst), 1);
        assert_eq!(drops[1].load(Ordering::SeqCst), 1);
    }

    #[test]
    fn all_stops_preserve_noncopy_ownership_before_and_after_commit() {
        for stage in [
            InputReserveStage::AfterAcquire,
            InputReserveStage::AfterAllocate,
            InputReserveStage::AfterCopyBeforeCommit,
            InputReserveStage::Committed,
        ] {
            for cause in [
                Cause::Input,
                Cause::Work,
                Cause::Changed,
                Cause::Allocation,
                Cause::Busy,
            ] {
                let host = Host::new();
                let (factory, observer) = factory(&host);
                let drops = counts();
                let mut buffer = values(&factory, &drops);
                let old_pointer = buffer.as_slice().as_ptr();
                let result = buffer.reserve_capacity_with_hook(4, &factory, |at| {
                    if at == stage {
                        cause.stop(&observer);
                    }
                });
                assert_eq!(result, Err(cause.error()));
                let capacity = if stage == InputReserveStage::Committed {
                    4
                } else {
                    2
                };
                assert_eq!((buffer.len(), buffer.capacity()), (2, capacity));
                assert_eq!(buffer[0].id, 0);
                assert_eq!(buffer[1].id, 1);
                if stage == InputReserveStage::Committed {
                    assert_ne!(buffer.as_slice().as_ptr(), old_pointer);
                } else {
                    assert_eq!(buffer.as_slice().as_ptr(), old_pointer);
                }
                assert!(drops.iter().all(|count| count.load(Ordering::SeqCst) == 0));
                assert_eq!(
                    host.state.lock().unwrap().active,
                    input_buffer_layout::<Value>(capacity)
                        .unwrap()
                        .allocation_size
                );
                let _ = observer.allocation_failed_io();
                assert_eq!(StlError::from(observer.check().unwrap_err()), cause.error());
                drop(buffer);
                assert_eq!(host.state.lock().unwrap().active, 0);
                assert_eq!(drops[0].load(Ordering::SeqCst), 1);
                assert_eq!(drops[1].load(Ordering::SeqCst), 1);
            }
        }
    }

    #[test]
    fn noncopy_shared_paths_move_without_clone_or_new_path_acquisition() {
        struct PathValue {
            path: crate::metadata_owned::SharedPath,
            drops: Arc<AtomicUsize>,
        }
        impl InputElement for PathValue {}
        impl Drop for PathValue {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }
        let host = Host::new();
        let (factory, _) = factory(&host);
        let path = factory.copy_path("3D/component.model").unwrap();
        let address = path.as_str().as_ptr();
        let drops = Arc::new(AtomicUsize::new(0));
        let mut buffer = OwnedInputBuffer::empty();
        buffer.reserve_capacity(1, &factory).unwrap();
        buffer.push_reserved(PathValue {
            path,
            drops: drops.clone(),
        });
        buffer.reserve_capacity(4, &factory).unwrap();
        buffer.reserve_capacity(8, &factory).unwrap();
        assert_eq!(buffer[0].path.as_str().as_ptr(), address);
        assert_eq!(host.state.lock().unwrap().requests.len(), 4);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(factory);
        drop(buffer);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(host.state.lock().unwrap().active, 0);
    }

    #[test]
    fn panicking_element_drop_still_drops_tail_and_frees_before_refund() {
        let freed = Arc::new(AtomicBool::new(false));
        let host = Arc::new(Host {
            limit: AtomicUsize::new(usize::MAX),
            state: Mutex::new(FundingState::default()),
            require_freed: Some(freed.clone()),
        });
        let (factory, _) = factory(&host);
        let drops = counts();
        let mut buffer = OwnedInputBuffer::empty();
        buffer.reserve_capacity(8, &factory).unwrap();
        for id in 0..3 {
            buffer.push_reserved(Value {
                id,
                drops: drops.clone(),
                panic_on_drop: id == 0,
            });
        }
        let block = buffer.block.take().unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop_block::<Value, _>(block, || freed.store(true, Ordering::SeqCst));
        }));
        assert!(result.is_err());
        assert!(freed.load(Ordering::SeqCst));
        assert!(drops.iter().all(|count| count.load(Ordering::SeqCst) == 1));
        assert_eq!(host.state.lock().unwrap().active, 0);
        assert_eq!(host.state.lock().unwrap().releases, 1);
        assert!(buffer.is_empty());
    }

    #[test]
    fn empty_fastpath_and_smaller_targets_never_allocate_or_truncate() {
        let host = Host::new();
        let (factory, observer) = factory(&host);
        let mut buffer = OwnedInputBuffer::<[u32; 3]>::empty();
        assert!(buffer.as_slice().is_empty());
        buffer.reserve_capacity(0, &factory).unwrap();
        assert!(host.state.lock().unwrap().requests.is_empty());
        buffer.reserve_capacity(2, &factory).unwrap();
        buffer.push_reserved([1, 2, 3]);
        buffer.reserve_capacity(1, &factory).unwrap();
        assert_eq!(host.state.lock().unwrap().requests.len(), 1);
        assert_eq!(
            buffer.reserve_capacity(0, &factory),
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        );
        assert_eq!(buffer.as_slice(), &[[1, 2, 3]]);
        assert_eq!(buffer.capacity(), 2);
        assert_eq!(
            buffer.reserve_capacity(2, &factory),
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        );
        assert!(observer.check().is_err());
        assert_eq!(host.state.lock().unwrap().requests.len(), 1);
        drop(buffer);
        assert_eq!(host.state.lock().unwrap().active, 0);
    }

    #[test]
    fn profiles_are_checked_actual_t_layouts_and_overflow_stops_before_admission() {
        #[repr(align(64))]
        struct Aligned(u8);
        impl InputElement for Aligned {}
        assert!(input_buffer_layout::<[f32; 3]>(0).is_none());
        assert!(input_buffer_layout::<[u32; 3]>(usize::MAX).is_none());
        let profile = input_buffer_layout::<Aligned>(2).unwrap();
        assert_eq!(profile.element_size, 64);
        assert_eq!(profile.element_align, 64);
        assert_eq!(profile.payload_offset % 64, 0);
        assert_eq!(profile.allocation_align, 64);
        let host = Host::new();
        let (factory, _) = factory(&host);
        let mut buffer = OwnedInputBuffer::<Aligned>::empty();
        buffer.reserve_capacity(2, &factory).unwrap();
        buffer.push_reserved(Aligned(7));
        assert_eq!(buffer.as_slice().as_ptr() as usize % 64, 0);
        assert_eq!(buffer[0].0, 7);
        drop(buffer);
        let (factory, _) = self::factory(&host);
        let mut buffer = OwnedInputBuffer::<[u32; 3]>::empty();
        assert_eq!(
            buffer.reserve_capacity(usize::MAX, &factory),
            Err(StlError::ResourceStopped(ResourceStop::InputLimitExceeded))
        );
        assert_eq!(host.state.lock().unwrap().requests.len(), 1);
        assert_eq!((buffer.len(), buffer.capacity()), (0, 0));
    }
}
