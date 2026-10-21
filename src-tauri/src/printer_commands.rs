use serde::Deserialize;
use tauri::State;

use crate::extension_api::AppError;
use crate::library_commands::{library_error, LibraryState};
use crate::settings::{self, SettingsState};
use library_core::models::{
    LegacyPrinter, LegacyPrinterImportSummary, Printer, PrinterDataImport, PrinterDataImportResult,
    PrinterInput, PrinterMaintenanceInput, PrinterMaintenanceLog, PrinterReminder,
    PrinterReminderCompletion, PrinterReminderCompletionInput, PrinterReminderInput,
};

fn library_failure(message: String) -> AppError {
    AppError::Library { message }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterCreateBatchItem {
    pub input: PrinterInput,
    pub endpoint_approved: bool,
}

/// Migrates the old settings.json bed-fit entries into the backed-up catalog.
/// The database import is idempotent, so a crash before settings.json is
/// cleared safely retries on the next launch without duplicating records.
fn migrate_legacy_settings_printers_with<E>(
    current_settings: &mut settings::Settings,
    import_legacy: impl FnOnce(&[LegacyPrinter]) -> Result<LegacyPrinterImportSummary, E>,
    save_settings: impl FnOnce(&settings::Settings) -> Result<(), E>,
) -> Result<LegacyPrinterImportSummary, E> {
    if current_settings.printers.is_empty() {
        return Ok(LegacyPrinterImportSummary::default());
    }

    let legacy = current_settings
        .printers
        .iter()
        .map(|printer| LegacyPrinter {
            id: printer.id.clone(),
            name: printer.name.clone(),
            bed_mm: printer.bed_mm,
        })
        .collect::<Vec<_>>();
    let summary = import_legacy(&legacy)?;

    let mut updated = current_settings.clone();
    updated.printers.clear();
    save_settings(&updated)?;
    *current_settings = updated;
    Ok(summary)
}

pub(crate) fn migrate_legacy_settings_printers(
    app: &tauri::AppHandle,
    library: &LibraryState,
    settings_state: &SettingsState,
) -> Result<LegacyPrinterImportSummary, AppError> {
    let mut current_settings = settings_state.0.lock().unwrap();
    migrate_legacy_settings_printers_with(
        &mut current_settings,
        |legacy| library.with_db(|db| db.import_legacy_printers(legacy).map_err(library_error)),
        |updated| settings::save(app, updated).map_err(library_failure),
    )
}

#[tauri::command]
pub fn library_list_printers(state: State<'_, LibraryState>) -> Result<Vec<Printer>, AppError> {
    state.with_db(|db| db.list_printers().map_err(library_error))
}

#[tauri::command]
pub fn library_create_printer(
    state: State<'_, LibraryState>,
    input: PrinterInput,
    endpoint_approved: bool,
) -> Result<Printer, AppError> {
    state.with_db(|db| {
        db.create_printer(&input, endpoint_approved)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_create_printers_batch(
    state: State<'_, LibraryState>,
    inputs: Vec<PrinterCreateBatchItem>,
) -> Result<Vec<Printer>, AppError> {
    let items = inputs
        .into_iter()
        .map(|item| (item.input, item.endpoint_approved))
        .collect::<Vec<_>>();
    state.with_db(|db| db.create_discovered_printers(&items).map_err(library_error))
}

#[tauri::command]
pub fn library_import_printer_data(
    state: State<'_, LibraryState>,
    input: PrinterDataImport,
) -> Result<PrinterDataImportResult, AppError> {
    state.with_db(|db| db.import_printer_data(&input).map_err(library_error))
}

#[tauri::command]
pub fn library_update_printer(
    state: State<'_, LibraryState>,
    id: String,
    expected_version: u64,
    input: PrinterInput,
    endpoint_approved: Option<bool>,
) -> Result<Printer, AppError> {
    state.with_db(|db| {
        db.update_printer(&id, expected_version, &input, endpoint_approved)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_delete_printer(
    app: tauri::AppHandle,
    state: State<'_, LibraryState>,
    settings_state: State<'_, SettingsState>,
    id: String,
    expected_version: u64,
) -> Result<(), AppError> {
    state.with_db(|db| {
        db.delete_printer(&id, expected_version)
            .map_err(library_error)
    })?;

    let mut settings = settings_state.0.lock().unwrap();
    if settings.active_printer == id {
        let mut updated = settings.clone();
        updated.active_printer = "generic-256".into();
        settings::save(&app, &updated).map_err(library_failure)?;
        *settings = updated;
    }
    Ok(())
}

#[tauri::command]
pub fn library_list_printer_maintenance_logs(
    state: State<'_, LibraryState>,
    printer_id: Option<String>,
) -> Result<Vec<PrinterMaintenanceLog>, AppError> {
    state.with_db(|db| {
        db.list_printer_maintenance_logs(printer_id.as_deref())
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_create_printer_maintenance_log(
    state: State<'_, LibraryState>,
    printer_id: String,
    input: PrinterMaintenanceInput,
) -> Result<PrinterMaintenanceLog, AppError> {
    state.with_db(|db| {
        db.create_printer_maintenance_log(&printer_id, &input)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_update_printer_maintenance_log(
    state: State<'_, LibraryState>,
    id: String,
    expected_version: u64,
    input: PrinterMaintenanceInput,
) -> Result<PrinterMaintenanceLog, AppError> {
    state.with_db(|db| {
        db.update_printer_maintenance_log(&id, expected_version, &input)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_delete_printer_maintenance_log(
    state: State<'_, LibraryState>,
    id: String,
    expected_version: u64,
) -> Result<(), AppError> {
    state.with_db(|db| {
        db.delete_printer_maintenance_log(&id, expected_version)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_list_printer_reminders(
    state: State<'_, LibraryState>,
    printer_id: Option<String>,
    include_completed: bool,
) -> Result<Vec<PrinterReminder>, AppError> {
    state.with_db(|db| {
        db.list_printer_reminders(printer_id.as_deref(), include_completed)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_create_printer_reminder(
    state: State<'_, LibraryState>,
    printer_id: String,
    input: PrinterReminderInput,
) -> Result<PrinterReminder, AppError> {
    state.with_db(|db| {
        db.create_printer_reminder(&printer_id, &input)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_update_printer_reminder(
    state: State<'_, LibraryState>,
    id: String,
    expected_version: u64,
    input: PrinterReminderInput,
) -> Result<PrinterReminder, AppError> {
    state.with_db(|db| {
        db.update_printer_reminder(&id, expected_version, &input)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_delete_printer_reminder(
    state: State<'_, LibraryState>,
    id: String,
    expected_version: u64,
) -> Result<(), AppError> {
    state.with_db(|db| {
        db.delete_printer_reminder(&id, expected_version)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_complete_printer_reminder(
    state: State<'_, LibraryState>,
    id: String,
    expected_version: u64,
    completion: PrinterReminderCompletionInput,
) -> Result<PrinterReminderCompletion, AppError> {
    state.with_db(|db| {
        db.complete_printer_reminder(&id, expected_version, &completion)
            .map_err(library_error)
    })
}

#[cfg(test)]
mod migration_tests {
    use super::migrate_legacy_settings_printers_with;
    use crate::settings::{Printer as LegacySettingsPrinter, Settings};
    use library_core::models::LegacyPrinterImportSummary;

    const LEGACY_COUNT: usize = 5_000;

    fn settings_with_legacy_printers(count: usize) -> Settings {
        Settings {
            active_printer: "legacy-printer-0".into(),
            printers: (0..count)
                .map(|index| LegacySettingsPrinter {
                    id: format!("legacy-printer-{index}"),
                    name: format!("Legacy printer {index}"),
                    bed_mm: [220.0, 220.0, 250.0],
                })
                .collect(),
            ..Settings::default()
        }
    }

    #[test]
    fn migration_save_failure_keeps_legacy_settings_for_idempotent_retry() {
        let mut settings = settings_with_legacy_printers(LEGACY_COUNT);
        let active_printer = settings.active_printer.clone();
        let mut imported_record_counts = Vec::new();

        let error = migrate_legacy_settings_printers_with::<String>(
            &mut settings,
            |legacy| {
                imported_record_counts.push(legacy.len());
                assert_eq!(legacy.first().unwrap().id, "legacy-printer-0");
                assert_eq!(legacy.last().unwrap().id, "legacy-printer-4999");
                Ok(LegacyPrinterImportSummary {
                    inserted: LEGACY_COUNT,
                    already_present: 0,
                })
            },
            |updated| {
                assert!(updated.printers.is_empty());
                assert_eq!(updated.active_printer, active_printer);
                Err("settings write failed".to_owned())
            },
        )
        .unwrap_err();

        assert_eq!(error, "settings write failed");
        assert_eq!(settings.printers.len(), LEGACY_COUNT);
        assert_eq!(settings.active_printer, active_printer);

        let summary = migrate_legacy_settings_printers_with::<String>(
            &mut settings,
            |legacy| {
                imported_record_counts.push(legacy.len());
                Ok(LegacyPrinterImportSummary {
                    inserted: 0,
                    already_present: legacy.len(),
                })
            },
            |updated| {
                assert!(updated.printers.is_empty());
                assert_eq!(updated.active_printer, active_printer);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(imported_record_counts, [LEGACY_COUNT, LEGACY_COUNT]);
        assert_eq!(summary.inserted, 0);
        assert_eq!(summary.already_present, LEGACY_COUNT);
        assert!(settings.printers.is_empty());
        assert_eq!(settings.active_printer, active_printer);
    }
}
