use crate::error::AppError;
use crate::library_commands::{library_error, LibraryState};
use library_runtime::printer_network::{
    self as shared, is_original_elegoo_centauri_carbon, parse_scan_scope, same_printer_snapshot,
};
pub use shared::{PrinterConnectionTestResponse, PrinterConnectionTestResult, Suggestion};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::State;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

pub(crate) fn native_network_guard() -> shared::NetworkGuard {
    shared::NetworkGuard::new(|| crate::visual_qa::require_printer_network().is_ok())
}

#[derive(Clone, Default)]
pub struct PrinterNetworkState {
    active_scan: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

impl PrinterNetworkState {
    fn begin_scan(&self) -> Result<ScanLease, AppError> {
        let mut active = self.active_scan.lock().map_err(|_| AppError::Library {
            message: "Printer discovery state is unavailable".into(),
        })?;
        if active.is_some() {
            return Err(AppError::Library {
                message: "Printer discovery is already running".into(),
            });
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        *active = Some(cancelled.clone());
        Ok(ScanLease {
            active_scan: self.active_scan.clone(),
            cancelled,
        })
    }

    fn cancel_scan(&self) -> bool {
        let Ok(active) = self.active_scan.lock() else {
            return false;
        };
        if let Some(cancelled) = active.as_ref() {
            cancelled.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }
}

struct ScanLease {
    active_scan: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for ScanLease {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active_scan.lock() {
            if active
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &self.cancelled))
            {
                *active = None;
            }
        }
    }
}

/// Starts a bounded discovery operation only when explicitly invoked by the UI.
#[tauri::command]
pub async fn library_discover_printers(
    state: State<'_, PrinterNetworkState>,
    range: String,
) -> Result<Vec<Suggestion>, AppError> {
    crate::visual_qa::require_printer_network()?;
    let scope = parse_scan_scope(&range).map_err(|error| AppError::Library {
        message: error.to_string(),
    })?;
    let lease = state.begin_scan()?;
    let cancelled = lease.cancelled.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        shared::discover(&scope, cancelled, native_network_guard())
    })
    .await
    .map_err(|_| AppError::Library {
        message: "Printer discovery could not complete".into(),
    })
}

/// Cancels the current user-started scan between bounded requests.
#[tauri::command]
pub fn library_cancel_printer_discovery(state: State<'_, PrinterNetworkState>) -> bool {
    state.cancel_scan()
}

/// Checks a previously approved, saved private IPv4 target on explicit request.
#[tauri::command]
pub async fn library_test_printer_connection(
    app: tauri::AppHandle,
    state: State<'_, LibraryState>,
    id: String,
    mainboard_id: Option<String>,
    discover_mainboard_id: bool,
) -> Result<PrinterConnectionTestResponse, AppError> {
    crate::visual_qa::require_printer_network()?;
    let printer = state.with_db(|db| {
        db.get_printer(&id)
            .map_err(library_error)?
            .ok_or_else(|| AppError::Library {
                message: "printer not found".into(),
            })
    })?;
    if discover_mainboard_id
        && (mainboard_id.is_some()
            || !printer.endpoint_approved
            || !is_original_elegoo_centauri_carbon(&printer))
    {
        return Err(AppError::Library {
            message: "MainboardID discovery is not available for this printer".into(),
        });
    }
    let discovery_target = if discover_mainboard_id {
        Some(
            shared::printer_address(&printer).ok_or_else(|| AppError::Library {
                message: "Printer target is not approved for discovery".into(),
            })?,
        )
    } else {
        None
    };
    // This value is a pre-I/O snapshot. The conditional write checks its
    // metadata version, endpoint, and approval before saving the result.
    let library = (*state).clone();
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if discover_mainboard_id {
            let target = discovery_target.ok_or_else(|| AppError::Library {
                message: "Printer target is not approved for discovery".into(),
            })?;
            let confirmed = app
                .dialog()
                .message(
                    format!(
                        "This sends one M99999 UDP broadcast on the local interface selected for {} ({target}) to port 3000. Other devices on that LAN may receive it. GoggleLab accepts only this printer's matching response, then sends one read-only SDCP Cmd 0 WebSocket request. The MainboardID is not saved.",
                        printer.nickname
                    ),
                )
                .title("Find MainboardID and test")
                .kind(MessageDialogKind::Warning)
                .buttons(MessageDialogButtons::OkCancelCustom(
                    "Continue".into(),
                    "Cancel".into(),
                ))
                .blocking_show();
            if !confirmed {
                return Ok(PrinterConnectionTestResponse {
                    result: PrinterConnectionTestResult::Cancelled,
                    printer: printer.clone(),
                    status_event_observed: false,
                });
            }
            let current_printer = library.with_db(|db| {
                db.get_printer(&printer.id).map_err(library_error)
            })?;
            if !same_printer_snapshot(&printer, current_printer.as_ref()) {
                return Err(AppError::Library {
                    message: "Printer details changed while the confirmation was open. Review the printer and try again.".into(),
                });
            }
        }
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());
        let outcome = shared::check_connection(&printer, mainboard_id.as_deref(), discover_mainboard_id, native_network_guard());
        let updated = library.with_db(|db| {
            db.record_printer_status_if_current(
                &printer,
                outcome.connection_status,
                outcome.job_status,
                &now,
                outcome.error_code.as_deref(),
            )
            .map_err(library_error)
        })?;
        Ok(PrinterConnectionTestResponse {
            result: outcome.result,
            printer: updated,
            status_event_observed: outcome.status_event_observed,
        })
    })
    .await
    .map_err(|_| AppError::Library {
        message: "Printer connection check could not complete".into(),
    })?
}
