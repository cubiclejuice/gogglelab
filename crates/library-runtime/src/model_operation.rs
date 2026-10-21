use std::sync::Arc;

use stl_core::metadata_owned::HostOwnedFunding;
use stl_core::package_bounds::PackageProcessingContext;
use stl_core::zip_bounds::{ZipFundingBusy, ZipResourceObserver};

use crate::{LibraryRuntime, TempResourceLimits, TempResourceState};
use std::sync::Mutex;

// Created once with Runtime; its control allocation is a host bootstrap term.
pub(super) struct RuntimeOwnedFunding {
    state: Arc<Mutex<TempResourceState>>,
    limits: Arc<Mutex<TempResourceLimits>>,
}

impl RuntimeOwnedFunding {
    pub(super) fn new(
        state: Arc<Mutex<TempResourceState>>,
        limits: Arc<Mutex<TempResourceLimits>>,
    ) -> Self {
        Self { state, limits }
    }
}

impl HostOwnedFunding for RuntimeOwnedFunding {
    fn try_acquire(&self, bytes: usize) -> Result<(), ZipFundingBusy> {
        let bytes = u64::try_from(bytes).map_err(|_| ZipFundingBusy)?;
        let limits = *self.limits.lock().map_err(|_| ZipFundingBusy)?;
        let mut state = self.state.lock().map_err(|_| ZipFundingBusy)?;
        if state.owned_accounting_uncertain
            || state
                .owned_bytes
                .checked_add(state.reserved_owned_bytes)
                .and_then(|used| used.checked_add(bytes))
                .is_none_or(|used| used > limits.owned_bytes)
        {
            return Err(ZipFundingBusy);
        }
        state.reserved_owned_bytes += bytes;
        Ok(())
    }

    fn release(&self, bytes: usize) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Ok(bytes) = u64::try_from(bytes) else {
            state.owned_accounting_uncertain = true;
            return;
        };
        // Retain known charges and deny future Owned admissions after uncertainty.
        // A later valid refund cannot reestablish the missing accounting authority.
        if let Some(remaining) = state.reserved_owned_bytes.checked_sub(bytes) {
            state.reserved_owned_bytes = remaining;
        } else {
            state.owned_accounting_uncertain = true;
        }
    }
}

impl LibraryRuntime {
    pub(crate) fn model_processing_context(&self) -> PackageProcessingContext {
        self.refresh_geometry_retained_accounting();
        let funding: Arc<dyn HostOwnedFunding> = self.model_funding.clone();
        PackageProcessingContext::new_funded(ZipResourceObserver::new(), funding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReservationKind, RuntimeConfig};
    use stl_core::StlError;

    fn runtime(temp: &tempfile::TempDir) -> Arc<LibraryRuntime> {
        LibraryRuntime::open(RuntimeConfig::desktop(
            temp.path().join("library"),
            temp.path().join("integrations.json"),
        ))
        .unwrap()
    }

    #[test]
    fn model_funding_competes_with_existing_owned_reservations() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = runtime(&temp);
        runtime.set_temp_limits_for_test(4096, 4096);
        for kind in [
            ReservationKind::Owned,
            ReservationKind::Geometry,
            ReservationKind::Bundle,
        ] {
            let held = runtime.reserve_temp(kind, 4096).unwrap();
            let context = runtime.model_processing_context();
            assert!(matches!(
                context.path_allocator().copy_path("3D/root.model"),
                Err(StlError::FundingBusy)
            ));
            assert_eq!(
                runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
                4096
            );
            drop(held);
            let context = runtime.model_processing_context();
            let path = context.path_allocator().copy_path("3D/root.model").unwrap();
            assert!(runtime.temp_resources.lock().unwrap().reserved_owned_bytes > 0);
            drop(path);
            assert_eq!(
                runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
                0
            );
        }
    }

    #[test]
    fn model_funding_reads_current_limits_on_every_admission() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = runtime(&temp);
        runtime.set_temp_limits_for_test(4096, 4096);
        let context = runtime.model_processing_context();
        let path = context.path_allocator().copy_path("3D/root.model").unwrap();
        let charged = runtime.temp_resources.lock().unwrap().reserved_owned_bytes;
        assert!(charged > 0);
        runtime.temp_limits.lock().unwrap().owned_bytes = charged;
        assert!(matches!(
            context.path_allocator().copy_path("3D/another.model"),
            Err(StlError::FundingBusy)
        ));
        assert_eq!(
            runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
            charged
        );
        drop(path);
        assert_eq!(
            runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
            0
        );
    }

    #[test]
    fn model_path_credit_survives_context_and_runtime_until_last_owner() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = runtime(&temp);
        let weak_runtime = Arc::downgrade(&runtime);
        let state = runtime.temp_resources.clone();
        let context = runtime.model_processing_context();
        let path = context.path_allocator().copy_path("3D/root.model").unwrap();
        let clone = path.clone();
        let charged = state.lock().unwrap().reserved_owned_bytes;
        assert!(charged > 0);
        drop(context);
        drop(runtime);
        assert!(weak_runtime.upgrade().is_none());
        drop(path);
        assert_eq!(state.lock().unwrap().reserved_owned_bytes, charged);
        drop(clone);
        assert_eq!(state.lock().unwrap().reserved_owned_bytes, 0);
    }

    #[test]
    fn model_funding_release_underflow_and_poison_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = runtime(&temp);
        runtime.temp_resources.lock().unwrap().reserved_owned_bytes = 7;
        runtime.model_funding.release(8);
        assert_eq!(
            runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
            7
        );
        for after_valid_release in [false, true] {
            if after_valid_release {
                runtime.model_funding.release(7);
            }
            let context = runtime.model_processing_context();
            assert!(matches!(
                context.path_allocator().copy_path("3D/denied.model"),
                Err(StlError::FundingBusy)
            ));
            for kind in [
                ReservationKind::Owned,
                ReservationKind::Geometry,
                ReservationKind::Bundle,
            ] {
                let error = match runtime.reserve_temp(kind, 1) {
                    Ok(_) => panic!("uncertain accounting must reject every Owned admission"),
                    Err(error) => error,
                };
                assert_eq!(error.code, library_protocol::RuntimeErrorCode::Busy);
                assert!(error.retryable);
            }
            let upload = runtime.reserve_temp(ReservationKind::Upload, 1).unwrap();
            drop(upload);
            assert_eq!(
                runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
                if after_valid_release { 0 } else { 7 }
            );
        }
        drop(runtime);
        let poison_temp = tempfile::tempdir().unwrap();
        let runtime = self::runtime(&poison_temp);
        runtime.temp_resources.lock().unwrap().reserved_owned_bytes = 7;
        let state = runtime.temp_resources.clone();
        assert!(std::thread::spawn(move || {
            let _guard = state.lock().unwrap();
            panic!("intentional accounting poison");
        })
        .join()
        .is_err());
        assert!(runtime.model_funding.try_acquire(1).is_err());
        runtime.model_funding.release(7);
        match runtime.temp_resources.lock() {
            Err(error) => assert_eq!(error.into_inner().reserved_owned_bytes, 7),
            Ok(_) => panic!("expected poisoned accounting state"),
        };
    }

    #[test]
    fn model_funding_preserves_known_retained_and_committed_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = runtime(&temp);
        runtime.set_temp_limits_for_test(4096, 4096);
        let context = runtime.model_processing_context();
        runtime.temp_resources.lock().unwrap().owned_bytes = 4096;
        assert!(matches!(
            context.path_allocator().copy_path("3D/root.model"),
            Err(StlError::FundingBusy)
        ));
        assert_eq!(runtime.temp_resources.lock().unwrap().owned_bytes, 4096);
        assert_eq!(
            runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
            0
        );
    }
}
