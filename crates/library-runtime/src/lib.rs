use library_protocol::{
    Printer, PrinterConnectionStatus, PrinterFleetEngine, PrinterInput, PrinterJobStatus,
    PrinterMaintenanceInput, PrinterMaintenanceLog, PrinterReminder, PrinterReminderCompletion,
    PrinterReminderCompletionInput, PrinterReminderInput, PrinterReminderStatus, PrinterSourceKind,
};
use std::any::{Any, TypeId};
use std::cell::Cell;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime};

use library_core::models::PrintAttemptInputWithPrinter as CorePrintAttemptInputWithPrinter;
use library_core::{
    CaptureDownloadInput as CoreCaptureDownload, CapturePageInput as CoreCapturePage,
    CaptureReceipt as CoreCaptureReceipt, CaptureReceiptInput as CoreCaptureReceiptInput,
    CaptureSummary as CoreCaptureSummary, CatalogFacets as CoreCatalogFacets,
    CatalogSettings as CoreCatalogSettings, CatalogStats as CoreCatalogStats,
    Filament as CoreFilament, FilamentInput as CoreFilamentInput,
    FilamentOrigin as CoreFilamentOrigin, Folder as CoreFolder,
    IndexedModelMove as CoreIndexedModelMove, LibraryDb, LibraryError,
    MetadataPatch as CoreMetadataPatch, Model,
    ModelFilamentAssignment as CoreModelFilamentAssignment,
    ModelGroupFilter as CoreModelGroupFilter, ModelPageQuery as CoreModelPageQuery,
    ModelSort as CoreModelSort, PartStock as CorePartStock, PartStockInput as CorePartStockInput,
    PartUsageInput as CorePartUsageInput, PartUsageSnapshot as CorePartUsageSnapshot,
    PrintAttempt as CorePrintAttempt, PrintAttemptBatchItem as CorePrintAttemptBatchItem,
    PrintAttemptInput as CorePrintAttemptInput, PrintAttemptPart as CorePrintAttemptPart,
    PrintOutcome as CorePrintOutcome, PrintStatus as CorePrintStatus,
    SortDirection as CoreSortDirection, TagMode as CoreTagMode,
    VersionedModelUpdate as CoreVersionedModelUpdate, VersionedTarget as CoreVersionedTarget,
    MAX_MODEL_PAGE_OFFSET,
};
use library_protocol::{
    ActionPlan, ActionTargetSnapshot, BundleEngine, BundlePreviewSummary, Capability,
    CaptureDownloadResult, CaptureDownloadStatus, CaptureEngine, CaptureEnvelope,
    CaptureImportResult, CaptureItemStatus, CaptureSummary, CatalogEngine, CatalogFacetValue,
    CatalogFacets, CatalogModel, CatalogSettings, CatalogStats, ContentEngine, ContentKind,
    ContentLease, DownloadEngine, DuplicateEngine, DuplicateGroup, EmbeddedMetadataEngine,
    EmbeddedMetadataFields, EmbeddedMetadataProposal, EmbeddedMetadataSelection, Filament,
    FilamentEngine, FilamentInput, Folder, FolderEngine, GalleryEngine, GeometryEngine,
    GeometryLease, IndexRootSettings, IndexRootSettingsPatch, ManagementAction, ManagementEngine,
    ManagementResult, MediaLease, MetadataEngine, MetadataPatch, MetadataValueKind,
    ModelAvailability, ModelFilamentAssignment, ModelGroupFilter, ModelQuery, ModelSort,
    MoveLooseSourceTarget, MutationVersion, Page, PageRequest, PartStock, PartStockEngine,
    PartStockInput, PrintAttempt, PrintAttemptBatchItem, PrintAttemptInput,
    PrintAttemptInputWithPrinter, PrintAttemptPart, PrintHistoryEngine, PrintOutcome, PrintStatus,
    RequestContext, RestoreRootMapping, RootAccess, RootHandle, RuntimeError, RuntimeErrorCode,
    RuntimeEvent, RuntimeInfo, RuntimeInfoEngine, RuntimeOwnership, RuntimeStatus,
    RuntimeStatusEngine, ScanEngine, ScanStatus, SettingsEngine, SortDirection, SourceHoldReceipt,
    SourceLease, Tag, TagEngine, TagMode, ThumbnailDescriptor, UploadEngine, UploadIntent,
    UploadToken, VersionedMetadataUpdate, VersionedTarget, CAPTURE_ENVELOPE_VERSION,
    MAX_PAGE_LIMIT,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use stl_core::package_bounds::{collect_verified_model_source, PackageProcessingContext};
use stl_core::zip_bounds::ZipResourceObserver;

pub mod archives;
pub mod bundles;
pub mod catalog_jobs;
pub mod catalog_thumbnails;
pub mod embedded;
pub mod geometry;
pub mod images;
pub mod integration_state;
pub mod integrations;
mod png_output;
pub mod printer_network;
pub mod printer_network_api;
mod processing_error;
#[doc(hidden)]
pub use processing_error::ProcessingError;
pub mod scanner;
mod secure_fs;
pub use secure_fs::{
    GeneratedControlKind, GeneratedControlNamespace, GeneratedControlRole, GeneratedFileLocation,
    GeneratedFileReceipt, HeldEntry, MovedEntry,
};
mod thumbnail_cleanup;
pub mod thumbnails;

/// Opaque handle used by native management code for one admitted directory.
#[doc(hidden)]
pub struct GuardedDirectory(secure_fs::Directory);

impl GuardedDirectory {
    #[doc(hidden)]
    pub fn create_new_file_read_write(&self, name: &std::ffi::OsStr) -> std::io::Result<File> {
        self.0.create_new_file_read_write(name)
    }

    #[doc(hidden)]
    pub fn admit_created_generated(
        &self,
        namespace: &GeneratedControlNamespace,
        nonce: &str,
        role: GeneratedControlRole,
        file: File,
        bytes: &[u8],
    ) -> std::io::Result<GeneratedFileReceipt> {
        self.0
            .admit_created_generated(namespace, nonce, role, file, bytes)
    }
    #[doc(hidden)]
    pub fn recover_generated_file<T>(
        &self,
        namespace: &GeneratedControlNamespace,
        role: GeneratedControlRole,
        maximum: u64,
        validate: impl FnOnce(&[u8], Option<&str>) -> std::io::Result<(String, T)>,
    ) -> std::io::Result<Option<(GeneratedFileReceipt, T)>> {
        self.0
            .recover_generated_file(namespace, role, maximum, validate)
    }
    #[doc(hidden)]
    pub fn publish_generated_file(
        &self,
        receipt: &mut GeneratedFileReceipt,
        destination: GeneratedControlRole,
    ) -> std::io::Result<()> {
        self.0.publish_generated_file(receipt, destination)
    }
    #[doc(hidden)]
    pub fn exchange_generated_files(
        &self,
        new: &mut GeneratedFileReceipt,
        old: &mut GeneratedFileReceipt,
    ) -> std::io::Result<()> {
        self.0.exchange_generated_files(new, old)
    }
    #[doc(hidden)]
    pub fn delete_generated_file(&self, receipt: &mut GeneratedFileReceipt) -> std::io::Result<()> {
        self.0.delete_generated_file(receipt)
    }

    #[doc(hidden)]
    pub fn open(path: &Path) -> std::io::Result<Self> {
        secure_fs::Directory::open(path).map(Self)
    }

    #[doc(hidden)]
    pub fn io_path(&self) -> std::io::Result<PathBuf> {
        Ok(self.0.path()?.join("."))
    }

    #[doc(hidden)]
    pub fn sync(&self) -> std::io::Result<()> {
        self.0.sync()
    }

    #[doc(hidden)]
    pub fn sync_tree(&self) -> std::io::Result<()> {
        self.0.sync_tree()
    }

    #[doc(hidden)]
    pub fn matches_path(&self, path: &Path) -> std::io::Result<bool> {
        self.0.matches_path(path)
    }

    #[doc(hidden)]
    pub fn copy_file_from_path(
        &self,
        source_root: &Path,
        source_name: &std::ffi::OsStr,
        destination_name: &std::ffi::OsStr,
        expected_bytes: u64,
        expected_sha256: &str,
        max_bytes: u64,
    ) -> std::io::Result<()> {
        let source = secure_fs::Directory::open(source_root)?;
        secure_fs::copy_verified_into_directory(
            &source,
            source_name,
            &self.0,
            destination_name,
            &(expected_bytes, expected_sha256.to_owned()),
            max_bytes,
        )?;
        Ok(())
    }

    #[doc(hidden)]
    pub fn copy_file_from_directory(
        &self,
        source: &GuardedDirectory,
        source_name: &std::ffi::OsStr,
        destination_name: &std::ffi::OsStr,
        expected_bytes: u64,
        expected_sha256: &str,
        max_bytes: u64,
    ) -> std::io::Result<()> {
        secure_fs::copy_verified_into_directory(
            &source.0,
            source_name,
            &self.0,
            destination_name,
            &(expected_bytes, expected_sha256.to_owned()),
            max_bytes,
        )?;
        Ok(())
    }

    #[doc(hidden)]
    pub fn copy_tree_from_path(
        &self,
        source_root: &Path,
        max_file_bytes: u64,
    ) -> std::io::Result<()> {
        self.0.copy_tree_from_path(source_root, max_file_bytes)
    }

    #[doc(hidden)]
    pub fn copy_tree_from_directory(
        &self,
        source: &GuardedDirectory,
        max_file_bytes: u64,
    ) -> std::io::Result<()> {
        self.0.copy_tree_from_directory(&source.0, max_file_bytes)
    }

    #[doc(hidden)]
    pub fn create_new_relative_file(&self, relative: &Path) -> std::io::Result<File> {
        self.0.create_new_relative_file(relative)
    }

    #[doc(hidden)]
    pub fn hash_file(
        &self,
        name: &std::ffi::OsStr,
        max_bytes: u64,
    ) -> std::io::Result<(u64, String)> {
        secure_fs::hash_file(&mut self.0.open_file(name)?, max_bytes)
    }

    #[doc(hidden)]
    pub fn open_file_handle(&self, name: &std::ffi::OsStr) -> std::io::Result<File> {
        self.0.open_file(name)
    }

    #[doc(hidden)]
    pub fn write_new_file(&self, name: &std::ffi::OsStr, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = self.0.create_new_file(name)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        self.0.sync()
    }

    #[doc(hidden)]
    pub fn hash_relative_file(
        &self,
        relative: &Path,
        max_bytes: u64,
    ) -> std::io::Result<(u64, String)> {
        self.0.hash_relative_file(relative, max_bytes)
    }

    #[doc(hidden)]
    pub fn relative_file_exists(&self, relative: &Path) -> std::io::Result<bool> {
        self.0.relative_file_exists(relative)
    }

    /// Permanently delete a hash-verified generated file relative to this
    /// pinned directory. Original model files must use recoverable holds.
    #[doc(hidden)]
    pub fn delete_verified_file(
        &self,
        relative: &Path,
        expected_bytes: u64,
        expected_sha256: &str,
        max_bytes: u64,
    ) -> std::io::Result<()> {
        self.0.delete_verified_relative_file(
            relative,
            &(expected_bytes, expected_sha256.to_owned()),
            max_bytes,
        )
    }

    /// Move a hash-verified entry into a durable sibling hold instead of
    /// permanently unlinking file bytes.
    #[doc(hidden)]
    pub fn remove_verified_file(
        &self,
        relative: &Path,
        expected_bytes: u64,
        expected_sha256: &str,
        max_bytes: u64,
    ) -> std::io::Result<()> {
        let _held = self.0.remove_verified_relative_file(
            relative,
            &(expected_bytes, expected_sha256.to_owned()),
            max_bytes,
        )?;
        Ok(())
    }

    /// Move a verified file into a durable, same-filesystem destination. The
    /// source journal remains beside the original name for crash recovery.
    #[doc(hidden)]
    pub fn move_verified_file_to(
        &self,
        source_name: &std::ffi::OsStr,
        destination: &GuardedDirectory,
        destination_name: &std::ffi::OsStr,
        expected_bytes: u64,
        expected_sha256: &str,
        max_bytes: u64,
    ) -> std::io::Result<MovedEntry> {
        self.0.move_verified_file_to(
            source_name,
            &destination.0,
            destination_name,
            &(expected_bytes, expected_sha256.to_owned()),
            max_bytes,
        )
    }

    /// Move a visible directory or file by stable entry identity, with a
    /// durable journal and exclusive destination creation.
    #[doc(hidden)]
    pub fn move_entry_to(
        &self,
        source_name: &std::ffi::OsStr,
        destination: &GuardedDirectory,
        destination_name: &std::ffi::OsStr,
    ) -> std::io::Result<MovedEntry> {
        self.0
            .move_entry_to(source_name, &destination.0, destination_name)
    }

    /// Preserve a file under a unique hidden name with a durable recovery
    /// journal instead of unlinking it.
    #[doc(hidden)]
    pub fn hold_verified_file(
        &self,
        name: &std::ffi::OsStr,
        expected_bytes: u64,
        expected_sha256: &str,
        max_bytes: u64,
    ) -> std::io::Result<HeldEntry> {
        self.0.hold_verified_file(
            name,
            &(expected_bytes, expected_sha256.to_owned()),
            max_bytes,
        )
    }

    /// Hold a verified file under deterministic sibling names so a higher
    /// level transaction can reconstruct its recovery path after restart.
    #[doc(hidden)]
    pub fn hold_verified_file_as(
        &self,
        source_name: &std::ffi::OsStr,
        expected_bytes: u64,
        expected_sha256: &str,
        max_bytes: u64,
        hold_name: &std::ffi::OsStr,
        journal_name: &std::ffi::OsStr,
    ) -> std::io::Result<HeldEntry> {
        self.0.hold_verified_file_as(
            source_name,
            &(expected_bytes, expected_sha256.to_owned()),
            max_bytes,
            hold_name,
            journal_name,
        )
    }

    /// Preserve an arbitrary file or directory entry by identity. This is for
    /// visible filesystem items where a content hash is not available.
    #[doc(hidden)]
    pub fn hold_entry(&self, name: &std::ffi::OsStr) -> std::io::Result<HeldEntry> {
        self.0.hold_entry(name)
    }

    /// Restore a held entry into its original pinned directory. The operation
    /// refuses to overwrite an occupied original name.
    #[doc(hidden)]
    pub fn restore_held_entry(
        &self,
        held: &HeldEntry,
        original_name: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        self.0.restore_held_entry(held, original_name)
    }

    /// Rebuild a verified hold receipt after a process restart using its
    /// durable journal in this same pinned directory.
    #[doc(hidden)]
    pub fn recover_held_entry(&self, journal_name: &std::ffi::OsStr) -> std::io::Result<HeldEntry> {
        self.0.recover_held_entry(journal_name)
    }

    #[doc(hidden)]
    pub fn remove_empty_directory(&self, relative: &Path) -> std::io::Result<()> {
        self.0.remove_empty_relative_directory(relative)
    }

    #[doc(hidden)]
    pub fn modified_nanos(&self, name: &std::ffi::OsStr) -> std::io::Result<u64> {
        let modified = self.0.open_file(name)?.metadata()?.modified()?;
        Ok(modified
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |value| {
                u64::try_from(value.as_nanos()).unwrap_or(u64::MAX)
            }))
    }

    #[doc(hidden)]
    pub fn create_child_directory(
        &self,
        name: &std::ffi::OsStr,
    ) -> std::io::Result<GuardedDirectory> {
        self.0.create_child_directory(name)?;
        self.0.open_child(name).map(GuardedDirectory)
    }
}

impl library_backup::RestoreTarget for GuardedDirectory {
    fn create_new_file(&mut self, relative_path: &Path) -> std::io::Result<File> {
        self.create_new_relative_file(relative_path)
    }

    fn staging_path(&self) -> std::io::Result<PathBuf> {
        self.io_path()
    }

    fn sync_file_parent(&self, relative_path: &Path) -> std::io::Result<()> {
        self.0.sync_relative_parent(relative_path)
    }

    fn sync_root(&self) -> std::io::Result<()> {
        self.0.sync_tree()
    }
}

/// Opaque parent-directory handle for sibling catalog swaps.
#[doc(hidden)]
pub struct GuardedDirectoryParent(secure_fs::Directory);

impl GuardedDirectoryParent {
    #[doc(hidden)]
    pub fn admit_created_generated(
        &self,
        namespace: &GeneratedControlNamespace,
        nonce: &str,
        role: GeneratedControlRole,
        file: File,
        bytes: &[u8],
    ) -> std::io::Result<GeneratedFileReceipt> {
        self.0
            .admit_created_generated(namespace, nonce, role, file, bytes)
    }
    #[doc(hidden)]
    pub fn recover_generated_file<T>(
        &self,
        namespace: &GeneratedControlNamespace,
        role: GeneratedControlRole,
        maximum: u64,
        validate: impl FnOnce(&[u8], Option<&str>) -> std::io::Result<(String, T)>,
    ) -> std::io::Result<Option<(GeneratedFileReceipt, T)>> {
        self.0
            .recover_generated_file(namespace, role, maximum, validate)
    }
    #[doc(hidden)]
    pub fn publish_generated_file(
        &self,
        receipt: &mut GeneratedFileReceipt,
        destination: GeneratedControlRole,
    ) -> std::io::Result<()> {
        self.0.publish_generated_file(receipt, destination)
    }
    #[doc(hidden)]
    pub fn exchange_generated_files(
        &self,
        new: &mut GeneratedFileReceipt,
        old: &mut GeneratedFileReceipt,
    ) -> std::io::Result<()> {
        self.0.exchange_generated_files(new, old)
    }
    #[doc(hidden)]
    pub fn delete_generated_file(&self, receipt: &mut GeneratedFileReceipt) -> std::io::Result<()> {
        self.0.delete_generated_file(receipt)
    }

    #[doc(hidden)]
    pub fn open(path: &Path) -> std::io::Result<Self> {
        secure_fs::Directory::open(path).map(Self)
    }

    #[doc(hidden)]
    pub fn matches_path(&self, path: &Path) -> std::io::Result<bool> {
        self.0.matches_path(path)
    }

    #[doc(hidden)]
    pub fn open_child(&self, name: &std::ffi::OsStr) -> std::io::Result<GuardedDirectory> {
        self.0.open_child(name).map(GuardedDirectory)
    }

    #[doc(hidden)]
    pub fn open_file_handle(&self, name: &std::ffi::OsStr) -> std::io::Result<File> {
        self.0.open_file(name)
    }

    #[doc(hidden)]
    pub fn create_new_file_read_write(&self, name: &std::ffi::OsStr) -> std::io::Result<File> {
        self.0.create_new_file_read_write(name)
    }

    #[doc(hidden)]
    pub fn hash_file(
        &self,
        name: &std::ffi::OsStr,
        max_bytes: u64,
    ) -> std::io::Result<(u64, String)> {
        self.0.hash_file(name, max_bytes)
    }

    #[doc(hidden)]
    pub fn create_child_directory(
        &self,
        name: &std::ffi::OsStr,
    ) -> std::io::Result<GuardedDirectory> {
        self.0.create_child_directory(name)?;
        self.0.open_child(name).map(GuardedDirectory)
    }

    #[doc(hidden)]
    pub fn remove_child_tree(&self, name: &std::ffi::OsStr) -> std::io::Result<()> {
        self.0.remove_child_tree(name)
    }

    /// Move a verified file into a durable, same-filesystem destination.
    #[doc(hidden)]
    pub fn move_verified_file_to(
        &self,
        source_name: &std::ffi::OsStr,
        destination: &GuardedDirectoryParent,
        destination_name: &std::ffi::OsStr,
        expected_bytes: u64,
        expected_sha256: &str,
        max_bytes: u64,
    ) -> std::io::Result<MovedEntry> {
        self.0.move_verified_file_to(
            source_name,
            &destination.0,
            destination_name,
            &(expected_bytes, expected_sha256.to_owned()),
            max_bytes,
        )
    }

    /// Move a visible directory or file by stable entry identity.
    #[doc(hidden)]
    pub fn move_entry_to(
        &self,
        source_name: &std::ffi::OsStr,
        destination: &GuardedDirectoryParent,
        destination_name: &std::ffi::OsStr,
    ) -> std::io::Result<MovedEntry> {
        self.0
            .move_entry_to(source_name, &destination.0, destination_name)
    }

    /// Preserve an arbitrary file or directory entry by identity.
    #[doc(hidden)]
    pub fn hold_entry(&self, name: &std::ffi::OsStr) -> std::io::Result<HeldEntry> {
        self.0.hold_entry(name)
    }

    /// Preserve a verified file under a unique hidden name.
    #[doc(hidden)]
    pub fn hold_verified_file(
        &self,
        name: &std::ffi::OsStr,
        expected_bytes: u64,
        expected_sha256: &str,
        max_bytes: u64,
    ) -> std::io::Result<HeldEntry> {
        self.0.hold_verified_file(
            name,
            &(expected_bytes, expected_sha256.to_owned()),
            max_bytes,
        )
    }

    /// Permanently delete a hash-verified generated file after checking that
    /// its directory entry still names the opened file. Original model files
    /// must use recoverable holds instead.
    #[doc(hidden)]
    pub fn delete_verified_file(
        &self,
        name: &std::ffi::OsStr,
        expected_bytes: u64,
        expected_sha256: &str,
        max_bytes: u64,
    ) -> std::io::Result<()> {
        self.0.delete_verified_file(
            name,
            &(expected_bytes, expected_sha256.to_owned()),
            max_bytes,
        )
    }

    /// Hold a verified file under deterministic sibling names.
    #[doc(hidden)]
    pub fn hold_verified_file_as(
        &self,
        source_name: &std::ffi::OsStr,
        expected_bytes: u64,
        expected_sha256: &str,
        max_bytes: u64,
        hold_name: &std::ffi::OsStr,
        journal_name: &std::ffi::OsStr,
    ) -> std::io::Result<HeldEntry> {
        self.0.hold_verified_file_as(
            source_name,
            &(expected_bytes, expected_sha256.to_owned()),
            max_bytes,
            hold_name,
            journal_name,
        )
    }

    /// Restore a held file or directory into its original pinned directory.
    #[doc(hidden)]
    pub fn restore_held_entry(
        &self,
        held: &HeldEntry,
        original_name: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        self.0.restore_held_entry(held, original_name)
    }

    /// Rebuild a verified hold receipt after a process restart.
    #[doc(hidden)]
    pub fn recover_held_entry(&self, journal_name: &std::ffi::OsStr) -> std::io::Result<HeldEntry> {
        self.0.recover_held_entry(journal_name)
    }

    #[doc(hidden)]
    pub fn entry_exists(&self, name: &std::ffi::OsStr) -> std::io::Result<bool> {
        self.0.entry_exists(name)
    }

    #[doc(hidden)]
    pub fn write_new_file(&self, name: &std::ffi::OsStr, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = self.0.create_new_file(name)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        self.0.sync()
    }

    #[doc(hidden)]
    pub fn link_child(
        &self,
        source: &std::ffi::OsStr,
        destination: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        self.0.link_child(source, destination)
    }

    #[doc(hidden)]
    pub fn replace_child(
        &self,
        source: &std::ffi::OsStr,
        destination: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        self.0.replace_child(source, destination)
    }

    /// Remove an entry from its active name by moving it into a recoverable
    /// sibling hold. This does not permanently delete bytes.
    #[doc(hidden)]
    pub fn remove_file(&self, name: &std::ffi::OsStr) -> std::io::Result<()> {
        self.0.unlink_file(name)?;
        self.0.sync()
    }

    /// Move a hash-verified entry into a durable sibling hold.
    #[doc(hidden)]
    pub fn remove_verified_file(
        &self,
        name: &std::ffi::OsStr,
        expected_bytes: u64,
        expected_sha256: &str,
        max_bytes: u64,
    ) -> std::io::Result<()> {
        let _held = self.0.unlink_verified_file(
            name,
            &(expected_bytes, expected_sha256.to_owned()),
            max_bytes,
        )?;
        self.0.sync()
    }

    #[doc(hidden)]
    pub fn remove_file_if_same(
        &self,
        name: &std::ffi::OsStr,
        expected_file: &File,
    ) -> std::io::Result<()> {
        let _held = self.0.unlink_file_if_same(name, expected_file)?;
        self.0.sync()
    }

    #[doc(hidden)]
    pub fn create_empty_library_child(
        &self,
        name: &std::ffi::OsStr,
    ) -> std::io::Result<GuardedDirectory> {
        self.0.create_child_directory(name)?;
        let result = (|| {
            let stage = self.0.open_child(name)?;
            let template = create_scratch_directory("empty-library")
                .map_err(|error| std::io::Error::other(error.message))?;
            let template_root = template.path.clone();
            drop(LibraryDb::open(&template_root).map_err(|error| {
                std::io::Error::other(format!("could not initialize empty Library: {error}"))
            })?);
            for child in ["originals", "manuals", "thumbnails", "staging", "trash"] {
                stage.create_child_directory(std::ffi::OsStr::new(child))?;
            }
            let source = secure_fs::Directory::open(&template_root)?;
            let mut database = source.open_file(std::ffi::OsStr::new("library.sqlite3"))?;
            let expected = secure_fs::hash_file(&mut database, 64 * 1024 * 1024)?;
            secure_fs::copy_verified_into_directory(
                &source,
                std::ffi::OsStr::new("library.sqlite3"),
                &stage,
                std::ffi::OsStr::new("library.sqlite3"),
                &expected,
                64 * 1024 * 1024,
            )?;
            stage.sync()?;
            Ok(GuardedDirectory(stage))
        })();
        if result.is_err() {
            let _ = self.0.remove_child_tree(name);
        }
        result
    }

    #[doc(hidden)]
    pub fn sync(&self) -> std::io::Result<()> {
        self.0.sync()
    }

    #[doc(hidden)]
    pub fn rename_child_exclusive(
        &self,
        source: &std::ffi::OsStr,
        destination: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        self.0.rename_child_exclusive(source, destination)
    }

    #[doc(hidden)]
    pub fn swap_library_children(
        &self,
        root_name: &std::ffi::OsStr,
        staged_name: &std::ffi::OsStr,
        previous_name: &std::ffi::OsStr,
    ) -> Result<(), (String, bool)> {
        if let Err(error) = self.0.rename_child_exclusive(root_name, previous_name) {
            return Err((error.to_string(), true));
        }
        if let Err(error) = self.0.sync() {
            return Err((
                format!("Could not sync the saved Library directory: {error}"),
                false,
            ));
        }
        if let Err(error) = self.0.rename_child_exclusive(staged_name, root_name) {
            let rollback = self.0.rename_child_exclusive(previous_name, root_name);
            let recovered = rollback.is_ok() && self.0.sync().is_ok();
            return Err((
                format!("Could not install the empty Library: {error}; rollback: {rollback:?}"),
                recovered,
            ));
        }
        if let Err(error) = self.0.sync() {
            return Err((
                format!("Could not sync the new Library directory: {error}"),
                false,
            ));
        }
        Ok(())
    }

    #[doc(hidden)]
    pub fn rollback_library_children(
        &self,
        root_name: &std::ffi::OsStr,
        previous_name: &std::ffi::OsStr,
        failed_name: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        self.0.rename_child_exclusive(root_name, failed_name)?;
        self.0.sync()?;
        self.0.rename_child_exclusive(previous_name, root_name)?;
        self.0.sync()
    }
}

const MAX_ADMITTED_ROOTS: usize = 256;
const MAX_CURSORS: usize = 256;
const MAX_EVENTS: usize = 1024;
const MAX_LEASES: usize = 256;
const LEASE_TTL: Duration = Duration::from_secs(5 * 60);
const DEFAULT_OWNED_TEMP_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const DEFAULT_UPLOAD_TEMP_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PHYSICAL_MUTATION_TARGETS: usize = 100;
const MAX_PHYSICAL_MUTATION_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ORPHAN_CLEANUP_FILES: usize = 10_000;
const MAX_ORPHAN_CLEANUP_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;

type EpochInvalidator = Arc<dyn Fn(u64) + Send + Sync>;
type ScanCompletedHook = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct RuntimeRootPolicy {
    pub id: String,
    pub catalog_root_id: Option<String>,
    pub label: String,
    pub path: PathBuf,
    pub access: RootAccess,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub library_root: PathBuf,
    pub integration_settings_path: PathBuf,
    pub roots: Vec<RuntimeRootPolicy>,
    pub capabilities: BTreeSet<Capability>,
    pub local_catalog_root_access: bool,
}

impl RuntimeConfig {
    pub fn desktop(library_root: PathBuf, integration_settings_path: PathBuf) -> Self {
        let mut capabilities = BTreeSet::from([
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
        ]);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        capabilities.insert(Capability::PhysicalSourceWrite);
        Self {
            library_root,
            integration_settings_path,
            roots: Vec::new(),
            capabilities,
            local_catalog_root_access: true,
        }
    }
}

#[derive(Debug, Clone)]
struct AdmittedRoot {
    handle: RootHandle,
    catalog_root_id: Option<String>,
    path: PathBuf,
}

struct CatalogLock {
    file: File,
    identity: String,
}

impl CatalogLock {
    fn acquire(library_root: &Path) -> Result<Self, RuntimeError> {
        let parent = library_root
            .parent()
            .ok_or_else(|| invalid("Library root must have a stable parent directory"))?;
        let parent_metadata = match fs::symlink_metadata(parent) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(parent).map_err(|_| io_error())?;
                fs::symlink_metadata(parent).map_err(|_| io_error())?
            }
            Err(_) => return Err(io_error()),
        };
        if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
            return Err(invalid("Library parent must be a real directory"));
        }
        let canonical_parent = parent.canonicalize().map_err(|_| io_error())?;
        let leaf = library_root
            .file_name()
            .ok_or_else(|| invalid("Library root is invalid"))?;
        let mut digest = Sha256::new();
        digest.update(canonical_parent.as_os_str().as_encoded_bytes());
        digest.update([0]);
        digest.update(leaf.as_encoded_bytes());
        let identity = format!("{:x}", digest.finalize());
        let lock_path = canonical_parent.join(format!(".gogglelab-{}.lock", &identity[..24]));
        match fs::symlink_metadata(&lock_path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Conflict,
                    message: "Library ownership lock path is unsafe".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(io_error()),
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&lock_path).map_err(|_| io_error())?;
        let metadata = file.metadata().map_err(|_| io_error())?;
        if !metadata.is_file() {
            return Err(invalid("Library ownership lock is not a regular file"));
        }
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => RuntimeError {
                code: RuntimeErrorCode::Busy,
                message: "Library is already owned by another process".into(),
                retryable: true,
                current_version: None,
                source_holds: Vec::new(),
            },
            std::fs::TryLockError::Error(_) => RuntimeError {
                code: RuntimeErrorCode::Unsupported,
                message: "Library filesystem does not support exclusive ownership locking".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            },
        })?;
        Ok(Self { file, identity })
    }
}

impl Drop for CatalogLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[derive(Default)]
struct LifecycleState {
    maintenance: bool,
    active_io: usize,
}

#[derive(Clone)]
struct ModelCursor {
    query: ModelQuery,
    sort_value: String,
    model_id: String,
    revision: u64,
}

#[derive(Clone)]
struct CaptureCursor {
    created_at: String,
    item_id: String,
    revision: u64,
}

#[derive(Clone)]
enum ContentBacking {
    ExistingPath { path: PathBuf, remove_on_drop: bool },
    Geometry(Arc<geometry::GeneratedGeometryPacket>),
}
impl ContentBacking {
    fn path(&self) -> Option<&Path> {
        match self {
            Self::ExistingPath { path, .. } => Some(path),
            Self::Geometry(_) => None,
        }
    }
    fn verify(&self, descriptor: &ContentLease) -> Result<(), RuntimeError> {
        match self {
            Self::ExistingPath { path, .. } => verify_leased_file(
                path,
                descriptor.bytes,
                &descriptor.sha256,
                content_limit(descriptor.kind),
            ),
            Self::Geometry(packet) => {
                if descriptor.kind != ContentKind::Geometry
                    || descriptor.bytes != packet.bytes()
                    || descriptor.sha256 != packet.sha256()
                    || descriptor.bytes > content_limit(ContentKind::Geometry)
                {
                    return Err(stale("Geometry backing changed"));
                }
                packet
                    .verify()
                    .map_err(|_| stale("Geometry backing failed verification"))
            }
        }
    }
    fn stream_to(
        &self,
        descriptor: &ContentLease,
        sink: &mut dyn Write,
    ) -> Result<(), RuntimeError> {
        match self {
            Self::ExistingPath { path, .. } => stream_verified_file(
                path,
                descriptor.bytes,
                &descriptor.sha256,
                content_limit(descriptor.kind),
                sink,
            ),
            Self::Geometry(packet) => {
                self.verify(descriptor)?;
                packet.stream_to(sink).map_err(|_| io_error())
            }
        }
    }
}
#[derive(Clone)]
struct ContentLeaseEntry {
    descriptor: SourceLease,
    backing: ContentBacking,
    expires: SystemTime,
    principal_id: String,
    source_token: Option<String>,
}

#[derive(Clone)]
struct UploadEntry {
    token: UploadToken,
    path: PathBuf,
    expires: SystemTime,
    sha256: String,
    bytes: u64,
    principal_id: String,
}

#[derive(Clone, Copy)]
struct TempResourceLimits {
    owned_bytes: u64,
    upload_bytes: u64,
    bundle_builds: usize,
}

impl Default for TempResourceLimits {
    fn default() -> Self {
        Self {
            owned_bytes: DEFAULT_OWNED_TEMP_BYTES,
            upload_bytes: DEFAULT_UPLOAD_TEMP_BYTES,
            bundle_builds: 1,
        }
    }
}

#[derive(Default)]
struct TempResourceState {
    owned_bytes: u64,
    reserved_owned_bytes: u64,
    staged_upload_bytes: u64,
    reserved_upload_bytes: u64,
    active_bundle_builds: usize,
    retained_geometry_bytes: u64,
    geometry_inventory_error: Option<RuntimeError>,
}

struct PlannedAction {
    action: ManagementAction,
    principal_id: String,
    expires: SystemTime,
    library_epoch: u64,
    catalog_revision: u64,
    targets: Vec<ActionTargetSnapshot>,
    input_fingerprint: Option<(u64, String)>,
    physical_sources: Vec<PhysicalSourceSnapshot>,
    managed_assets: Vec<ManagedAssetSnapshot>,
    thumbnail_cleanup: Option<thumbnail_cleanup::Plan>,
}

#[derive(Clone)]
struct PhysicalSourceSnapshot {
    model_id: String,
    metadata_version: u64,
    root_handle_id: String,
    catalog_root_id: String,
    root_path: PathBuf,
    relative_path: PathBuf,
    bytes: u64,
    sha256: String,
}

fn source_hold_receipt(
    snapshot: &PhysicalSourceSnapshot,
    held: &secure_fs::HeldEntry,
) -> Result<SourceHoldReceipt, RuntimeError> {
    let (bytes, sha256) = held
        .expected
        .as_ref()
        .cloned()
        .ok_or_else(|| internal("Verified source hold is missing its fingerprint"))?;
    Ok(SourceHoldReceipt {
        model_id: snapshot.model_id.clone(),
        root_handle_id: snapshot.root_handle_id.clone(),
        relative_path: snapshot.relative_path.to_string_lossy().replace('\\', "/"),
        hold_name: held.hold_name.to_string_lossy().into_owned(),
        journal_name: held.journal_name.to_string_lossy().into_owned(),
        bytes,
        sha256,
    })
}

fn deterministic_source_hold_names(
    operation_key: &str,
) -> Result<(std::ffi::OsString, std::ffi::OsString), RuntimeError> {
    if operation_key.is_empty()
        || operation_key.len() > 175
        || operation_key.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        })
    {
        return Err(invalid("Source hold operation key is invalid"));
    }
    let base = format!(".gogglelab-held-{operation_key}");
    Ok((
        std::ffi::OsString::from(format!("{base}.data")),
        std::ffi::OsString::from(format!("{base}.journal")),
    ))
}

fn recover_source_hold_receipt(
    directory: &secure_fs::Directory,
    snapshot: &PhysicalSourceSnapshot,
    journal_name: &std::ffi::OsStr,
) -> Option<SourceHoldReceipt> {
    let held = directory.recover_held_entry(journal_name).ok()?;
    source_hold_receipt(snapshot, &held).ok()
}

fn source_hold_summary(holds: &[SourceHoldReceipt]) -> String {
    holds
        .iter()
        .map(|hold| {
            format!(
                "model {} at {}:{} retained as {} ({})",
                hold.model_id,
                hold.root_handle_id,
                hold.relative_path,
                hold.hold_name,
                hold.journal_name
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ManagedAssetSnapshot {
    relative_path: PathBuf,
    bytes: u64,
    sha256: String,
}

use std::io;

/// Opaque authority for one generated purge/restore journal and its scratch files.
#[doc(hidden)]
pub struct GeneratedTransactionJournal {
    namespace: GeneratedControlNamespace,
    kind: GeneratedControlKind,
    document: GeneratedJournalDocument,
    main: GeneratedFileReceipt,
    scratch: Vec<GeneratedFileReceipt>,
    public_name: std::ffi::OsString,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeneratedJournalDocument {
    version: u32,
    root_path: String,
    nonce: String,
    staged_name: String,
    previous_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failed_name: Option<String>,
    #[serde(default)]
    committed: bool,
}
fn generated_metadata_error(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn validate_generated_journal(
    namespace: &GeneratedControlNamespace,
    kind: GeneratedControlKind,
    bytes: &[u8],
    expected_nonce: Option<&str>,
) -> io::Result<(String, GeneratedJournalDocument)> {
    if bytes.len() > 8192 {
        return Err(generated_metadata_error("Generated journal is oversized"));
    }
    let value: GeneratedJournalDocument = serde_json::from_slice(bytes)
        .map_err(|_| generated_metadata_error("Generated journal is invalid"))?;
    let root = namespace.root();
    let root_name = root.file_name().unwrap_or_default().to_string_lossy();
    let nonce = &value.nonce;
    let valid_nonce = nonce.len() == 32 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit());
    let (staged, previous, failed) = match kind {
        GeneratedControlKind::Purge => (
            format!(".gogglelab-empty-{nonce}"),
            format!("{root_name}.previous-purge-{nonce}"),
            None,
        ),
        GeneratedControlKind::Restore => (
            format!(".gogglelab-restore-{nonce}"),
            format!("{root_name}.previous-{nonce}"),
            Some(format!(".gogglelab-failed-restore-{nonce}")),
        ),
    };
    if value.version != 1
        || !valid_nonce
        || value.root_path != root.to_string_lossy()
        || expected_nonce.is_some_and(|expected| expected != nonce)
        || value.staged_name != staged
        || value.previous_name != previous
        || value.failed_name != failed
    {
        return Err(generated_metadata_error(
            "Generated journal does not match its transaction",
        ));
    }
    Ok((nonce.clone(), value))
}
impl AsRef<Path> for GeneratedTransactionJournal {
    fn as_ref(&self) -> &Path {
        Path::new(&self.public_name)
    }
}
impl GeneratedTransactionJournal {
    pub fn create(
        parent: &GuardedDirectoryParent,
        root: &Path,
        kind: GeneratedControlKind,
        bytes: &[u8],
    ) -> io::Result<Self> {
        let namespace = GeneratedControlNamespace::new(root, kind)?;
        let (nonce, document) = validate_generated_journal(&namespace, kind, bytes, None)?;
        if document.committed {
            return Err(generated_metadata_error("New journal must be prepared"));
        }
        for role in [
            GeneratedControlRole::Main,
            GeneratedControlRole::Temporary,
            GeneratedControlRole::Commit,
        ] {
            if parent
                .recover_generated_file(&namespace, role, 8192, |bytes, nonce| {
                    validate_generated_journal(&namespace, kind, bytes, nonce)
                })?
                .is_some()
            {
                return Err(generated_metadata_error(
                    "Transaction metadata already exists",
                ));
            }
        }
        let public_name = namespace.public_name(GeneratedControlRole::Main)?;
        if parent.entry_exists(&public_name)? {
            return Err(generated_metadata_error(
                "A recovery journal already exists",
            ));
        }
        let name = namespace.public_name(GeneratedControlRole::Temporary)?;
        let mut file = parent.create_new_file_read_write(&name)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        let mut main = parent.admit_created_generated(
            &namespace,
            &nonce,
            GeneratedControlRole::Temporary,
            file,
            bytes,
        )?;
        parent.publish_generated_file(&mut main, GeneratedControlRole::Main)?;
        Ok(Self {
            namespace,
            kind,
            document,
            main,
            scratch: Vec::new(),
            public_name,
        })
    }
    pub fn recover(
        parent: &GuardedDirectoryParent,
        root: &Path,
        kind: GeneratedControlKind,
    ) -> io::Result<Option<Self>> {
        let namespace = GeneratedControlNamespace::new(root, kind)?;
        let recovered = parent.recover_generated_file(
            &namespace,
            GeneratedControlRole::Main,
            8192,
            |bytes, nonce| validate_generated_journal(&namespace, kind, bytes, nonce),
        )?;
        let Some((main, document)) = recovered else {
            let mut temporary = parent.recover_generated_file(
                &namespace,
                GeneratedControlRole::Temporary,
                8192,
                |bytes, nonce| validate_generated_journal(&namespace, kind, bytes, nonce),
            )?;
            let commit = parent.recover_generated_file(
                &namespace,
                GeneratedControlRole::Commit,
                8192,
                |bytes, nonce| validate_generated_journal(&namespace, kind, bytes, nonce),
            )?;
            if commit.is_some() {
                return Err(generated_metadata_error(
                    "Commit scratch exists without an authoritative journal",
                ));
            }
            if let Some((receipt, document)) = temporary.as_mut() {
                if document.committed {
                    return Err(generated_metadata_error(
                        "Unpublished journal must be prepared",
                    ));
                }
                parent.delete_generated_file(receipt)?;
            }
            parent.sync()?;
            return Ok(None);
        };
        let public_name = namespace.public_name(GeneratedControlRole::Main)?;
        let mut result = Self {
            namespace,
            kind,
            document,
            main,
            scratch: Vec::new(),
            public_name,
        };
        for role in [
            GeneratedControlRole::Temporary,
            GeneratedControlRole::Commit,
        ] {
            if let Some((receipt, _)) = result.recover_scratch(parent, role)? {
                result.scratch.push(receipt);
            }
        }
        Ok(Some(result))
    }
    fn recover_scratch(
        &self,
        parent: &GuardedDirectoryParent,
        role: GeneratedControlRole,
    ) -> io::Result<Option<(GeneratedFileReceipt, GeneratedJournalDocument)>> {
        parent.recover_generated_file(&self.namespace, role, 8192, |bytes, claim_nonce| {
            let (nonce, mut document) =
                validate_generated_journal(&self.namespace, self.kind, bytes, claim_nonce)?;
            let mut expected = self.document.clone();
            if nonce != expected.nonce
                || (role == GeneratedControlRole::Temporary && document.committed)
            {
                return Err(generated_metadata_error(
                    "Scratch journal does not match the active transaction",
                ));
            }
            expected.committed = false;
            let committed = document.committed;
            document.committed = false;
            if document != expected {
                return Err(generated_metadata_error("Scratch journal fields changed"));
            }
            document.committed = committed;
            Ok((nonce, document))
        })
    }
    pub fn root(&self) -> &Path {
        self.namespace.root()
    }
    pub fn kind(&self) -> GeneratedControlKind {
        self.kind
    }
    pub fn nonce(&self) -> &str {
        &self.document.nonce
    }
    pub fn committed(&self) -> bool {
        self.document.committed
    }
    pub fn bytes(&self) -> &[u8] {
        self.main.bytes()
    }
    pub fn public_name(&self) -> &std::ffi::OsStr {
        &self.public_name
    }
    pub fn mark_committed(&mut self, parent: &GuardedDirectoryParent) -> io::Result<()> {
        if self.document.committed {
            return Err(generated_metadata_error("Journal is already committed"));
        }
        let mut document = self.document.clone();
        document.committed = true;
        let bytes = serde_json::to_vec(&document).map_err(io::Error::other)?;
        let name = self.namespace.public_name(GeneratedControlRole::Commit)?;
        let mut file = parent.create_new_file_read_write(&name)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        let replacement = parent.admit_created_generated(
            &self.namespace,
            self.nonce(),
            GeneratedControlRole::Commit,
            file,
            &bytes,
        )?;
        self.scratch.push(replacement);
        let replacement = self.scratch.last_mut().unwrap();
        let exchanged = parent.exchange_generated_files(replacement, &mut self.main);
        // The syscall can succeed before a later synchronization error. Keep
        // receipt roles aligned without reopening or reauthorizing either name.
        if replacement.role() == GeneratedControlRole::Main {
            std::mem::swap(replacement, &mut self.main);
            self.document = document;
        }
        exchanged
    }
    pub fn clear(&mut self, parent: &GuardedDirectoryParent) -> io::Result<()> {
        let observed_main = parent.recover_generated_file(
            &self.namespace,
            GeneratedControlRole::Main,
            8192,
            |bytes, nonce| validate_generated_journal(&self.namespace, self.kind, bytes, nonce),
        )?;
        match (observed_main, self.main.location()) {
            (None, GeneratedFileLocation::Unlinked) => {}
            (Some((observed, _)), location)
                if !matches!(location, GeneratedFileLocation::Unlinked)
                    && observed.current_name() == self.main.current_name()
                    && observed.bytes() == self.main.bytes() => {}
            _ => {
                return Err(generated_metadata_error(
                    "Unexpected or changed main journal was preserved",
                ));
            }
        }
        for role in [
            GeneratedControlRole::Temporary,
            GeneratedControlRole::Commit,
        ] {
            let observed = self.recover_scratch(parent, role)?;
            let owned = self.scratch.iter().find(|entry| {
                entry.role() == role && !matches!(entry.location(), GeneratedFileLocation::Unlinked)
            });
            match (observed, owned) {
                (None, None) => {}
                (Some((observed, _)), Some(owned))
                    if observed.current_name() == owned.current_name()
                        && observed.bytes() == owned.bytes() => {}
                _ => {
                    return Err(generated_metadata_error(
                        "Unexpected or changed transaction scratch was preserved",
                    ))
                }
            }
        }
        for scratch in &mut self.scratch {
            if !matches!(scratch.location(), GeneratedFileLocation::Unlinked) {
                parent.delete_generated_file(scratch)?;
            }
        }
        parent.sync()?;
        if !matches!(self.main.location(), GeneratedFileLocation::Unlinked) {
            parent.delete_generated_file(&mut self.main)?;
        }
        parent.sync()
    }
}
#[doc(hidden)]
pub fn create_generated_purge_marker(
    directory: &GuardedDirectory,
    root: &Path,
    nonce: &str,
) -> io::Result<GeneratedFileReceipt> {
    if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(generated_metadata_error("Invalid purge nonce"));
    }
    let namespace = GeneratedControlNamespace::new(root, GeneratedControlKind::Purge)?;
    let name = namespace.public_name(GeneratedControlRole::PurgeMarker)?;
    let mut file = directory.create_new_file_read_write(&name)?;
    file.write_all(nonce.as_bytes())?;
    file.sync_all()?;
    let receipt = directory.admit_created_generated(
        &namespace,
        nonce,
        GeneratedControlRole::PurgeMarker,
        file,
        nonce.as_bytes(),
    )?;
    directory.sync()?;
    Ok(receipt)
}
#[doc(hidden)]
pub fn recover_generated_purge_marker(
    directory: &GuardedDirectory,
    root: &Path,
    nonce: &str,
) -> io::Result<Option<GeneratedFileReceipt>> {
    let namespace = GeneratedControlNamespace::new(root, GeneratedControlKind::Purge)?;
    directory
        .recover_generated_file(
            &namespace,
            GeneratedControlRole::PurgeMarker,
            64,
            |bytes, claimed| {
                if nonce.len() != 32
                    || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
                    || bytes != nonce.as_bytes()
                    || claimed.is_some_and(|value| value != nonce)
                {
                    return Err(generated_metadata_error("Purge staging marker changed"));
                }
                Ok((nonce.to_owned(), ()))
            },
        )
        .map(|value| value.map(|(receipt, ())| receipt))
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PurgeJournal {
    version: u32,
    root_path: String,
    nonce: String,
    staged_name: String,
    previous_name: String,
    #[serde(default)]
    committed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreJournal {
    version: u32,
    root_path: String,
    nonce: String,
    staged_name: String,
    previous_name: String,
    failed_name: String,
    #[serde(default)]
    committed: bool,
}

struct OwnedDirectory {
    path: PathBuf,
    directory: secure_fs::Directory,
    cleanup_on_drop: bool,
    parent_directory: Option<secure_fs::Directory>,
    child_name: Option<std::ffi::OsString>,
}

struct PurgeJournalGuard<'a> {
    parent_directory: secure_fs::Directory,
    journal: &'a mut GeneratedTransactionJournal,
    cleanup_on_drop: bool,
}

impl<'a> PurgeJournalGuard<'a> {
    fn new(
        parent_directory: secure_fs::Directory,
        journal: &'a mut GeneratedTransactionJournal,
    ) -> Self {
        Self {
            parent_directory,
            journal,
            cleanup_on_drop: true,
        }
    }

    fn disarm(mut self) {
        self.cleanup_on_drop = false;
    }
}

impl Drop for PurgeJournalGuard<'_> {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            let _ = clear_purge_journal_at(&self.parent_directory, self.journal);
        }
    }
}

impl OwnedDirectory {
    fn create_at(parent: &secure_fs::Directory, name: &str) -> Result<Self, RuntimeError> {
        let child_name = std::ffi::OsString::from(name);
        parent
            .create_child_directory(&child_name)
            .map_err(|_| io_error())?;
        let directory = parent.open_child(&child_name).map_err(|_| io_error())?;
        let path = parent.child_path(&child_name).map_err(|_| io_error())?;
        Ok(Self {
            path,
            directory,
            cleanup_on_drop: true,
            parent_directory: Some(parent.try_clone().map_err(|_| io_error())?),
            child_name: Some(child_name),
        })
    }

    fn disarm(&mut self) {
        self.cleanup_on_drop = false;
    }
}

fn create_scratch_directory(purpose: &str) -> Result<OwnedDirectory, RuntimeError> {
    let parent_path = std::env::temp_dir()
        .canonicalize()
        .map_err(|_| io_error())?;
    let parent = secure_fs::Directory::open(&parent_path).map_err(|_| io_error())?;
    if !parent.matches_path(&parent_path).map_err(|_| io_error())? {
        return Err(stale("Scratch directory parent identity changed"));
    }
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    OwnedDirectory::create_at(&parent, &format!(".gogglelab-{purpose}-{nonce}"))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
struct GuardedBackupAssetSource {
    directory: secure_fs::Directory,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
const MAX_BACKUP_TREE_ENTRIES: usize = library_backup::MAX_BACKUP_TREE_ENTRY_COUNT;
#[cfg(any(target_os = "macos", target_os = "linux"))]
const MAX_BACKUP_TREE_DEPTH: usize = library_backup::MAX_BACKUP_DIRECTORY_DEPTH;

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl GuardedBackupAssetSource {
    fn asset_paths_with_limits(
        &self,
        maximum_entries: usize,
        maximum_depth: usize,
        maximum_path_bytes: usize,
    ) -> Result<Vec<PathBuf>, String> {
        let mut paths = Vec::new();
        let mut visited_entries = 0;
        let mut visited_path_bytes = 0;
        for namespace in ["originals", "manuals", "thumbnails", "trash"] {
            match self.directory.open_child(std::ffi::OsStr::new(namespace)) {
                Ok(_) => collect_guarded_backup_files(
                    &self.directory,
                    Path::new(namespace),
                    &mut paths,
                    &mut visited_entries,
                    &mut visited_path_bytes,
                    maximum_entries,
                    maximum_depth,
                    maximum_path_bytes,
                )?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(paths)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl library_backup::BackupAssetSource for GuardedBackupAssetSource {
    fn asset_paths(&self) -> Result<Vec<PathBuf>, String> {
        self.asset_paths_with_limits(
            MAX_BACKUP_TREE_ENTRIES,
            MAX_BACKUP_TREE_DEPTH,
            library_backup::MAX_BACKUP_PATH_BYTES,
        )
    }

    fn open_asset(&self, relative_path: &Path) -> Result<File, String> {
        self.directory
            .open_relative_file(relative_path)
            .map_err(|error| error.to_string())
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn collect_guarded_backup_files(
    root: &secure_fs::Directory,
    initial_directory: &Path,
    output: &mut Vec<PathBuf>,
    visited_entries: &mut usize,
    visited_path_bytes: &mut usize,
    maximum_entries: usize,
    maximum_depth: usize,
    maximum_path_bytes: usize,
) -> Result<(), String> {
    let mut pending = vec![(initial_directory.to_path_buf(), 0usize)];
    while let Some((relative_directory, depth)) = pending.pop() {
        if depth > maximum_depth {
            return Err("Backup asset directory tree is too deep".into());
        }
        let directory = root
            .open_relative(&relative_directory)
            .map_err(|error| error.to_string())?;
        let remaining_entries = maximum_entries.saturating_sub(*visited_entries);
        let mut names = directory
            .read_entry_names(remaining_entries)
            .map_err(|error| error.to_string())?;
        *visited_entries = visited_entries
            .checked_add(names.len())
            .ok_or("Backup asset entry count overflow")?;
        names.sort();
        for name in names {
            let relative_path = relative_directory.join(&name);
            let path_bytes = relative_path
                .as_os_str()
                .len()
                .checked_mul(2)
                .ok_or("Backup path size overflow")?;
            *visited_path_bytes = visited_path_bytes
                .checked_add(path_bytes)
                .ok_or("Backup path size overflow")?;
            if *visited_path_bytes > maximum_path_bytes {
                return Err("Backup paths exceed total size limit".into());
            }
            match directory.open_child(&name) {
                Ok(child) => {
                    drop(child);
                    if depth >= maximum_depth {
                        return Err("Backup asset directory tree is too deep".into());
                    }
                    pending.push((relative_path, depth + 1));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => {
                    let file = directory
                        .open_file(&name)
                        .map_err(|error| error.to_string())?;
                    if !file
                        .metadata()
                        .map_err(|error| error.to_string())?
                        .is_file()
                    {
                        return Err("Backup asset is not a regular file".into());
                    }
                    if output.len() >= library_backup::MAX_BACKUP_FILE_COUNT - 1 {
                        return Err("Backup contains too many files".into());
                    }
                    output.push(relative_path);
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }
    Ok(())
}

/// Creates a backup with a platform-appropriate no-clobber publication path.
/// macOS/Linux pin the destination directory and atomically rename a verified
/// temporary archive; other targets use library-backup's path-based wrapper.
#[doc(hidden)]
pub fn create_backup_file(
    library_root: &Path,
    destination: &Path,
) -> Result<library_backup::BackupResult, String> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        create_backup_file_with_limit(
            library_root,
            destination,
            library_backup::MAX_BACKUP_ARCHIVE_BYTES,
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        library_backup::create_backup(library_root, destination)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn create_backup_file_with_limit(
    library_root: &Path,
    destination: &Path,
    archive_limit: u64,
) -> Result<library_backup::BackupResult, String> {
    create_backup_file_with_limit_and_hook(library_root, destination, archive_limit, || {})
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn create_backup_file_with_limit_and_hook<F>(
    library_root: &Path,
    destination: &Path,
    archive_limit: u64,
    before_rename: F,
) -> Result<library_backup::BackupResult, String>
where
    F: FnOnce(),
{
    if !destination.is_absolute() {
        return Err("Backup destination must be an absolute path".into());
    }
    if !matches!(
        std::fs::symlink_metadata(destination),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    ) {
        return Err("Backup destination must be a new file".into());
    }
    let root_metadata =
        std::fs::symlink_metadata(library_root).map_err(|error| error.to_string())?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err("Library root is not a real directory".into());
    }
    let root = library_root
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let destination_parent_path = destination
        .parent()
        .ok_or("Backup destination has no parent")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if destination_parent_path.starts_with(&root) {
        return Err("Backup destination must be outside the Library".into());
    }
    let source_directory = secure_fs::Directory::open(&root).map_err(|error| error.to_string())?;
    if !source_directory
        .matches_path(&root)
        .map_err(|error| error.to_string())?
    {
        return Err("Library source root changed before backup".into());
    }
    let asset_source = GuardedBackupAssetSource {
        directory: source_directory
            .try_clone()
            .map_err(|error| error.to_string())?,
    };
    let destination_name = destination
        .file_name()
        .ok_or("Backup destination has no file name")?;
    let destination_parent = GuardedDirectoryParent::open(&destination_parent_path)
        .map_err(|error| error.to_string())?;
    if !destination_parent
        .matches_path(&destination_parent_path)
        .map_err(|error| error.to_string())?
    {
        return Err("Backup destination parent changed".into());
    }

    let temporary_name = std::ffi::OsString::from(format!(
        ".gogglelab-backup-{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let mut archive_file = destination_parent
        .create_new_file_read_write(&temporary_name)
        .map_err(|error| error.to_string())?;
    let info = match library_backup::write_backup_archive_from_source_with_limit(
        &root,
        &asset_source,
        &mut archive_file,
        archive_limit,
    ) {
        Ok(info) => info,
        Err(error) => {
            cleanup_unpublished_backup_file(&destination_parent, &temporary_name, archive_file);
            return Err(error);
        }
    };
    let source_root_matches = match source_directory.matches_path(&root) {
        Ok(matches) => matches,
        Err(error) => {
            cleanup_unpublished_backup_file(&destination_parent, &temporary_name, archive_file);
            return Err(error.to_string());
        }
    };
    if !source_root_matches {
        cleanup_unpublished_backup_file(&destination_parent, &temporary_name, archive_file);
        return Err("Library source root changed during backup".into());
    }
    if let Err(error) = archive_file.sync_all() {
        cleanup_unpublished_backup_file(&destination_parent, &temporary_name, archive_file);
        return Err(error.to_string());
    }
    if let Err(error) = archive_file.seek(std::io::SeekFrom::Start(0)) {
        cleanup_unpublished_backup_file(&destination_parent, &temporary_name, archive_file);
        return Err(error.to_string());
    }
    let (archive_bytes, archive_sha256) =
        match secure_fs::hash_file(&mut archive_file, archive_limit) {
            Ok(hash) if hash.1 == info.sha256 => hash,
            Ok(_) => {
                cleanup_unpublished_backup_file(&destination_parent, &temporary_name, archive_file);
                return Err("Backup archive failed integrity verification".into());
            }
            Err(error) => {
                cleanup_unpublished_backup_file(&destination_parent, &temporary_name, archive_file);
                return Err(error.to_string());
            }
        };
    drop(archive_file);
    let parent_matches = match destination_parent.matches_path(&destination_parent_path) {
        Ok(matches) => matches,
        Err(error) => {
            let _ = destination_parent.remove_verified_file(
                &temporary_name,
                archive_bytes,
                &archive_sha256,
                archive_limit,
            );
            return Err(error.to_string());
        }
    };
    if !parent_matches {
        let _ = destination_parent.remove_verified_file(
            &temporary_name,
            archive_bytes,
            &archive_sha256,
            archive_limit,
        );
        return Err("Backup destination parent changed during creation".into());
    }
    before_rename();
    if let Err(error) = destination_parent.rename_child_exclusive(&temporary_name, destination_name)
    {
        let _ = destination_parent.remove_verified_file(
            &temporary_name,
            archive_bytes,
            &archive_sha256,
            archive_limit,
        );
        return Err(error.to_string());
    }
    destination_parent
        .sync()
        .map_err(|error| error.to_string())?;
    let parent_matches = match destination_parent.matches_path(&destination_parent_path) {
        Ok(matches) => matches,
        Err(error) => {
            let _ = destination_parent.remove_verified_file(
                destination_name,
                archive_bytes,
                &archive_sha256,
                archive_limit,
            );
            return Err(error.to_string());
        }
    };
    if !parent_matches {
        let _ = destination_parent.remove_verified_file(
            destination_name,
            archive_bytes,
            &archive_sha256,
            archive_limit,
        );
        return Err("Backup destination parent changed during publication".into());
    }
    let published_hash = match destination_parent.hash_file(destination_name, archive_limit) {
        Ok(hash) => hash,
        Err(error) => {
            let _ = destination_parent.remove_verified_file(
                destination_name,
                archive_bytes,
                &archive_sha256,
                archive_limit,
            );
            return Err(error.to_string());
        }
    };
    if published_hash != (archive_bytes, archive_sha256.clone()) {
        let _ = destination_parent.remove_verified_file(
            destination_name,
            archive_bytes,
            &archive_sha256,
            archive_limit,
        );
        return Err("Published backup failed integrity verification".into());
    }

    Ok(library_backup::BackupResult {
        path: destination.to_path_buf(),
        files: info.files,
        bytes: info.bytes,
        sha256: archive_sha256,
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn cleanup_unpublished_backup_file(
    parent: &GuardedDirectoryParent,
    name: &std::ffi::OsStr,
    file: File,
) {
    let _ = parent.remove_file_if_same(name, &file);
}

impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            if let (Some(parent), Some(name)) = (&self.parent_directory, &self.child_name) {
                let _ = parent.remove_child_tree(name);
            }
        }
    }
}

enum ReservationKind {
    Bundle,
    Owned,
    Geometry,
    Upload,
}

struct TempReservation<'a> {
    runtime: &'a LibraryRuntime,
    kind: ReservationKind,
    bytes: u64,
    committed: bool,
}

pub struct LibraryRuntime {
    db: Mutex<Option<LibraryDb>>,
    root: PathBuf,
    integration_settings_path: PathBuf,
    roots: Vec<AdmittedRoot>,
    capabilities: BTreeSet<Capability>,
    local_catalog_root_access: bool,
    _ownership: CatalogLock,
    maintenance: AtomicBool,
    lifecycle: Mutex<LifecycleState>,
    library_epoch: AtomicU64,
    catalog_revision: AtomicU64,
    invalidators: Mutex<Vec<EpochInvalidator>>,
    scan_completed_hooks: Mutex<Vec<ScanCompletedHook>>,
    cursors: Mutex<HashMap<String, ModelCursor>>,
    capture_cursors: Mutex<HashMap<String, CaptureCursor>>,
    events: Mutex<VecDeque<RuntimeEvent>>,
    shared_states: Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    leases: Mutex<HashMap<String, ContentLeaseEntry>>,
    uploads: Mutex<HashMap<String, UploadEntry>>,
    scan: Arc<scanner::ScanControl>,
    bundle_previews: bundles::BundlePreviewStore,
    temp_limits: Mutex<TempResourceLimits>,
    temp_resources: Arc<Mutex<TempResourceState>>,
    actions: Mutex<HashMap<String, PlannedAction>>,
    thumbnail_cleanup: thumbnail_cleanup::Store,
    #[cfg(test)]
    before_action_dispatch: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    after_guarded_catalog_open: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    self_weak: OnceLock<Weak<LibraryRuntime>>,
}

impl LibraryRuntime {
    pub fn open(config: RuntimeConfig) -> Result<Arc<Self>, RuntimeError> {
        validate_identifier(&config.library_root, "Library root")?;
        validate_identifier(
            &config.integration_settings_path,
            "Integration settings path",
        )?;
        if config.roots.len() > MAX_ADMITTED_ROOTS {
            return Err(invalid("Too many admitted roots"));
        }
        let mut roots = admit_roots(config.roots)?;
        if roots
            .iter()
            .any(|root| root.handle.access == RootAccess::BackupOnly)
        {
            validate_backup_root_separation(&roots, &config.library_root)?;
        }
        let ownership = CatalogLock::acquire(&config.library_root)?;
        recover_interrupted_catalog_purge(&config.library_root)?;
        recover_interrupted_library_restore(&config.library_root)?;
        let (db, root) = if config.local_catalog_root_access {
            let root = prepare_lazy_desktop_root(&config.library_root)?;
            validate_backup_root_separation(&roots, &root)?;
            (None, root)
        } else {
            let db = LibraryDb::open(&config.library_root).map_err(runtime_library_error)?;
            let root = db.root().to_path_buf();
            validate_backup_root_separation(&roots, &root)?;
            cleanup_runtime_orphans(&root)?;
            map_operator_catalog_roots(
                &db,
                &mut roots,
                config.capabilities.contains(&Capability::Scan),
            )?;
            (Some(db), root)
        };
        let runtime = Arc::new(Self {
            db: Mutex::new(db),
            root,
            integration_settings_path: config.integration_settings_path,
            roots,
            capabilities: config.capabilities,
            local_catalog_root_access: config.local_catalog_root_access,
            _ownership: ownership,
            maintenance: AtomicBool::new(false),
            lifecycle: Mutex::new(LifecycleState::default()),
            library_epoch: AtomicU64::new(1),
            catalog_revision: AtomicU64::new(1),
            invalidators: Mutex::new(Vec::new()),
            scan_completed_hooks: Mutex::new(Vec::new()),
            cursors: Mutex::new(HashMap::new()),
            capture_cursors: Mutex::new(HashMap::new()),
            events: Mutex::new(VecDeque::new()),
            shared_states: Mutex::new(HashMap::new()),
            leases: Mutex::new(HashMap::new()),
            uploads: Mutex::new(HashMap::new()),
            scan: Arc::new(scanner::ScanControl::new()),
            bundle_previews: bundles::BundlePreviewStore::default(),
            temp_limits: Mutex::new(TempResourceLimits::default()),
            temp_resources: Arc::new(Mutex::new(TempResourceState::default())),
            actions: Mutex::new(HashMap::new()),
            thumbnail_cleanup: thumbnail_cleanup::Store::default(),
            #[cfg(test)]
            before_action_dispatch: Mutex::new(None),
            #[cfg(test)]
            after_guarded_catalog_open: Mutex::new(None),
            self_weak: OnceLock::new(),
        });
        runtime.refresh_geometry_retained_accounting();
        runtime
            .self_weak
            .set(Arc::downgrade(&runtime))
            .map_err(|_| internal("Runtime self reference could not be initialized"))?;
        if !runtime.local_catalog_root_access
            && runtime.capability_enabled(Capability::Scan)
            && runtime.capability_enabled(Capability::MediaWrite)
        {
            let weak = Arc::downgrade(&runtime);
            runtime.register_scan_completed_hook(Arc::new(move |root_id| {
                if let Some(runtime) = weak.upgrade() {
                    render_default_thumbnails_for_root(&runtime, root_id);
                }
            }));
        }
        Ok(runtime)
    }

    pub fn library_epoch(&self) -> u64 {
        self.library_epoch.load(Ordering::Acquire)
    }

    pub fn catalog_revision(&self) -> u64 {
        self.catalog_revision.load(Ordering::Acquire)
    }

    pub fn maintenance_active(&self) -> bool {
        self.maintenance.load(Ordering::Acquire)
    }

    pub fn ownership_mode(&self) -> RuntimeOwnership {
        RuntimeOwnership {
            lock_identity: self._ownership.identity.clone(),
            exclusive: true,
        }
    }

    pub fn root_handles(&self, context: &RequestContext) -> Result<Vec<RootHandle>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        Ok(self.roots.iter().map(|root| root.handle.clone()).collect())
    }

    pub(crate) fn capability_enabled(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    pub fn register_epoch_invalidator(&self, callback: Arc<dyn Fn(u64) + Send + Sync>) {
        self.invalidators.lock().unwrap().push(callback);
    }

    pub fn register_scan_completed_hook(&self, callback: Arc<dyn Fn(&str) + Send + Sync>) {
        self.scan_completed_hooks.lock().unwrap().push(callback);
    }

    #[cfg(test)]
    fn install_before_action_dispatch_hook(&self, hook: impl FnOnce() + Send + 'static) {
        *self.before_action_dispatch.lock().unwrap() = Some(Box::new(hook));
    }

    #[cfg(test)]
    fn install_after_guarded_catalog_open_hook(&self, hook: impl FnOnce() + Send + 'static) {
        *self.after_guarded_catalog_open.lock().unwrap() = Some(Box::new(hook));
    }

    #[cfg(test)]
    fn run_after_guarded_catalog_open_hook(&self) {
        if let Some(hook) = self.after_guarded_catalog_open.lock().unwrap().take() {
            hook();
        }
    }

    pub(crate) fn notify_scan_completed(&self, root_id: &str) {
        for callback in self.scan_completed_hooks.lock().unwrap().iter() {
            callback(root_id);
        }
    }

    pub fn scan_control(&self) -> &Arc<scanner::ScanControl> {
        &self.scan
    }

    pub fn install_shared_state<T>(&self, state: Arc<T>) -> Result<(), RuntimeError>
    where
        T: Any + Send + Sync,
    {
        let mut states = self.shared_states.lock().unwrap();
        match states.entry(TypeId::of::<T>()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(state);
                Ok(())
            }
            std::collections::hash_map::Entry::Occupied(_) => Err(RuntimeError {
                code: RuntimeErrorCode::Conflict,
                message: "Runtime shared state is already installed".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            }),
        }
    }

    pub fn shared_state<T>(&self) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        self.shared_states
            .lock()
            .unwrap()
            .get(&TypeId::of::<T>())
            .cloned()
            .and_then(|state| state.downcast::<T>().ok())
    }

    pub fn begin_io(self: &Arc<Self>) -> Result<RuntimeIoGuard, RuntimeError> {
        let mut lifecycle = self.lifecycle.lock().unwrap();
        if lifecycle.maintenance {
            return Err(maintenance_error());
        }
        lifecycle.active_io = lifecycle
            .active_io
            .checked_add(1)
            .ok_or_else(|| internal("Runtime I/O admission overflow"))?;
        Ok(RuntimeIoGuard {
            runtime: Arc::clone(self),
        })
    }

    fn begin_borrowed_io(&self) -> Result<BorrowedIoGuard<'_>, RuntimeError> {
        let mut lifecycle = self.lifecycle.lock().unwrap();
        if lifecycle.maintenance {
            return Err(maintenance_error());
        }
        lifecycle.active_io = lifecycle
            .active_io
            .checked_add(1)
            .ok_or_else(|| internal("Runtime I/O admission overflow"))?;
        Ok(BorrowedIoGuard { runtime: self })
    }

    pub fn begin_maintenance(self: &Arc<Self>) -> Result<RuntimeMaintenanceGuard, RuntimeError> {
        self.begin_maintenance_mode(false)
    }

    #[doc(hidden)]
    pub fn begin_source_mutation_maintenance(
        self: &Arc<Self>,
    ) -> Result<RuntimeMaintenanceGuard, RuntimeError> {
        self.begin_maintenance_mode(true)
    }

    fn begin_maintenance_mode(
        self: &Arc<Self>,
        preserve_catalog: bool,
    ) -> Result<RuntimeMaintenanceGuard, RuntimeError> {
        if self.scan_control().status().running {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Busy,
                message: "Library scan is running".into(),
                retryable: true,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        if preserve_catalog {
            self.read_db(|_| Ok::<(), LibraryError>(()))?;
        }
        {
            let mut lifecycle = self.lifecycle.lock().unwrap();
            if lifecycle.maintenance || lifecycle.active_io != 0 {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Busy,
                    message: "Library I/O or maintenance is already running".into(),
                    retryable: true,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            lifecycle.maintenance = true;
            self.maintenance.store(true, Ordering::Release);
        }
        let catalog = {
            let mut db = self.db.lock().unwrap();
            if preserve_catalog {
                db.take()
            } else {
                *db = None;
                None
            }
        };
        Ok(RuntimeMaintenanceGuard {
            runtime: Arc::clone(self),
            catalog,
            release_on_drop: Cell::new(true),
        })
    }

    pub fn read_db<T>(
        &self,
        operation: impl FnOnce(&LibraryDb) -> Result<T, LibraryError>,
    ) -> Result<T, RuntimeError> {
        if self.maintenance.load(Ordering::Acquire) {
            return Err(maintenance_error());
        }
        let mut guard = self.db.lock().unwrap();
        if self.maintenance.load(Ordering::Acquire) {
            return Err(maintenance_error());
        }
        if guard.is_none() {
            let db = LibraryDb::open(&self.root).map_err(runtime_library_error)?;
            cleanup_runtime_orphans(&self.root)?;
            *guard = Some(db);
        }
        let db = guard.as_ref().unwrap();
        if !db.catalog_path_matches() {
            return Err(stale("Library catalog file identity changed"));
        }
        let result = operation(db).map_err(runtime_library_error);
        if !db.catalog_path_matches() {
            return Err(stale(
                "Library catalog file identity changed during the operation",
            ));
        }
        result
    }

    pub fn with_db<T, E>(&self, operation: impl FnOnce(&LibraryDb) -> Result<T, E>) -> Result<T, E>
    where
        E: From<RuntimeError>,
    {
        if self.maintenance.load(Ordering::Acquire) {
            return Err(E::from(maintenance_error()));
        }
        let mut guard = self.db.lock().unwrap();
        if self.maintenance.load(Ordering::Acquire) {
            return Err(E::from(maintenance_error()));
        }
        if guard.is_none() {
            let db = LibraryDb::open(&self.root)
                .map_err(runtime_library_error)
                .map_err(E::from)?;
            cleanup_runtime_orphans(&self.root).map_err(E::from)?;
            *guard = Some(db);
        }
        let db = guard.as_ref().unwrap();
        if !db.catalog_path_matches() {
            return Err(E::from(stale("Library catalog file identity changed")));
        }
        let before = db.total_changes();
        let result = operation(db);
        if !db.catalog_path_matches() {
            return Err(E::from(stale(
                "Library catalog file identity changed during the operation",
            )));
        }
        let changed = db.total_changes() > before;
        drop(guard);
        if changed {
            self.record_mutation("catalogChanged", Vec::new());
        }
        result
    }

    pub fn mutate_db<T>(
        &self,
        operation: impl FnOnce(&LibraryDb) -> Result<T, LibraryError>,
    ) -> Result<T, RuntimeError> {
        let value = self.read_db(operation)?;
        self.record_mutation("catalogChanged", Vec::new());
        Ok(value)
    }

    pub fn host_library_root(&self) -> &Path {
        &self.root
    }

    pub fn host_integration_settings_path(&self) -> &Path {
        &self.integration_settings_path
    }

    pub fn admitted_root_path(
        &self,
        context: &RequestContext,
        root_id: &str,
        required: RootAccess,
    ) -> Result<PathBuf, RuntimeError> {
        let root = self
            .roots
            .iter()
            .find(|root| root.handle.id == root_id && access_allows(root.handle.access, required))
            .ok_or_else(|| RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Root handle is not admitted for this operation".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })?;
        match required {
            RootAccess::ReadOnly | RootAccess::Inbox => {
                self.require(context, Capability::CatalogRead)?;
            }
            RootAccess::Writable => self.require(context, Capability::PhysicalSourceWrite)?,
            RootAccess::BackupOnly if root.handle.access == RootAccess::Writable => {
                self.require(context, Capability::PhysicalSourceWrite)?;
            }
            RootAccess::BackupOnly => self.require_backup_write(context)?,
        }
        Ok(root.path.clone())
    }

    pub(crate) fn validate_scan_root(
        &self,
        root: &library_core::IndexRoot,
    ) -> Result<(), RuntimeError> {
        let canonical = root.path.canonicalize().map_err(|_| RuntimeError {
            code: RuntimeErrorCode::Io,
            message: "Index root is unavailable".into(),
            retryable: true,
            current_version: None,
            source_holds: Vec::new(),
        })?;
        if canonical != root.path {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Index root identity changed".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        if self.local_catalog_root_access {
            return Ok(());
        }
        if self.roots.iter().any(|admitted| {
            admitted.catalog_root_id.as_deref() == Some(root.id.as_str())
                && admitted.path == canonical
                && access_allows(admitted.handle.access, RootAccess::ReadOnly)
        }) {
            Ok(())
        } else {
            Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Index root is not admitted by operator policy".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })
        }
    }

    pub(crate) fn validate_indexed_source_admission(
        &self,
        model: &Model,
        source: &library_core::ResolvedModelSource,
    ) -> Result<(), RuntimeError> {
        if model.source_kind != "indexed" || self.local_catalog_root_access {
            return Ok(());
        }
        let root_id = model
            .source_root_id
            .as_deref()
            .ok_or_else(|| internal("Indexed source has no root identity"))?;
        if self.roots.iter().any(|root| {
            root.catalog_root_id.as_deref() == Some(root_id)
                && source.outer_path.starts_with(&root.path)
                && access_allows(root.handle.access, RootAccess::ReadOnly)
        }) {
            Ok(())
        } else {
            Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Indexed source root is not admitted".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })
        }
    }

    pub(crate) fn verified_model_source_bytes(
        &self,
        model: &Model,
    ) -> Result<Vec<u8>, RuntimeError> {
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        self.verified_model_source_bytes_in_context(model, &mut context)
    }

    pub(crate) fn verified_model_source_bytes_in_context(
        &self,
        model: &Model,
        context: &mut PackageProcessingContext,
    ) -> Result<Vec<u8>, RuntimeError> {
        let observer = context.observer();
        observer
            .check()
            .map_err(|e| ProcessingError::from(e).into_runtime(io_error()))?;
        let result = (|| {
            let (source, root_limit) = self.read_db(|db| {
                let root_limit = if model.source_kind == "indexed" {
                    let root_id = model.source_root_id.as_deref().ok_or_else(|| {
                        LibraryError::InvalidInput("Indexed model has no source root".into())
                    })?;
                    Some(
                        db.list_index_roots()?
                            .into_iter()
                            .find(|root| root.id == root_id)
                            .ok_or_else(|| LibraryError::IndexRootNotFound(root_id.into()))?
                            .max_file_bytes,
                    )
                } else {
                    None
                };
                Ok((db.resolve_model_source_ref(&model.id)?, root_limit))
            })?;
            self.validate_indexed_source_admission(model, &source)?;
            if root_limit.is_some_and(|limit| source.bytes > limit)
                || source.bytes > library_core::MAX_INDEXED_FILE_BYTES
            {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Unsupported,
                    message: "Model source exceeds its admitted file limit".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            if let Some(member) = source.archive_member.as_deref() {
                let temporary = archives::extract_member_to_temporary_in_context_typed(
                    &self.root,
                    &source.outer_path,
                    member,
                    source
                        .outer_fingerprint
                        .as_deref()
                        .ok_or_else(|| internal("Archive model has no source fingerprint"))?,
                    &source.content_hash,
                    source.bytes,
                    context,
                )
                .map_err(|error| error.into_runtime(stale("Archive model source changed")))?;
                read_verified_content_in_context(
                    temporary.path(),
                    source.bytes,
                    &source.content_hash,
                    context,
                )
            } else {
                read_verified_content_in_context(
                    &source.outer_path,
                    source.bytes,
                    &source.content_hash,
                    context,
                )
            }
        })();
        observer
            .check()
            .map_err(|e| ProcessingError::from(e).into_runtime(io_error()))?;
        result
    }

    fn require(
        &self,
        context: &RequestContext,
        capability: Capability,
    ) -> Result<(), RuntimeError> {
        if !self.capabilities.contains(&capability) || !context.capabilities.contains(&capability) {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Principal lacks the required capability".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        Ok(())
    }

    fn require_backup_write(&self, context: &RequestContext) -> Result<(), RuntimeError> {
        let allowed = [Capability::BackupWrite, Capability::PhysicalSourceWrite]
            .into_iter()
            .any(|capability| {
                self.capabilities.contains(&capability)
                    && context.capabilities.contains(&capability)
            });
        if allowed {
            Ok(())
        } else {
            Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Principal lacks backup write capability".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })
        }
    }

    fn require_upload_write(&self, context: &RequestContext) -> Result<(), RuntimeError> {
        let allowed = [Capability::CatalogWrite, Capability::MediaWrite]
            .into_iter()
            .any(|capability| {
                self.capabilities.contains(&capability)
                    && context.capabilities.contains(&capability)
            });
        if allowed {
            Ok(())
        } else {
            Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Principal lacks upload capability".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })
        }
    }

    fn reserve_temp(
        &self,
        kind: ReservationKind,
        bytes: u64,
    ) -> Result<TempReservation<'_>, RuntimeError> {
        if matches!(kind, ReservationKind::Owned | ReservationKind::Geometry) {
            self.refresh_geometry_retained_accounting();
        }
        let limits = *self.temp_limits.lock().unwrap();
        let mut state = self.temp_resources.lock().unwrap();
        if matches!(kind, ReservationKind::Geometry) {
            if let Some(error) = &state.geometry_inventory_error {
                return Err(error.clone());
            }
        }
        match kind {
            ReservationKind::Bundle => {
                if state.active_bundle_builds >= limits.bundle_builds
                    || state
                        .owned_bytes
                        .checked_add(state.reserved_owned_bytes)
                        .and_then(|value| value.checked_add(bytes))
                        .is_none_or(|value| value > limits.owned_bytes)
                {
                    return Err(RuntimeError {
                        code: RuntimeErrorCode::Busy,
                        message: "Bundle archive temporary storage is full".into(),
                        retryable: true,
                        current_version: None,
                        source_holds: Vec::new(),
                    });
                }
                state.reserved_owned_bytes += bytes;
                state.active_bundle_builds += 1;
            }
            ReservationKind::Owned | ReservationKind::Geometry => {
                if state
                    .owned_bytes
                    .checked_add(state.reserved_owned_bytes)
                    .and_then(|value| value.checked_add(bytes))
                    .is_none_or(|value| value > limits.owned_bytes)
                {
                    return Err(RuntimeError {
                        code: RuntimeErrorCode::Busy,
                        message: "Generated content temporary storage is full".into(),
                        retryable: true,
                        current_version: None,
                        source_holds: Vec::new(),
                    });
                }
                state.reserved_owned_bytes += bytes;
            }
            ReservationKind::Upload => {
                if state
                    .staged_upload_bytes
                    .checked_add(state.reserved_upload_bytes)
                    .and_then(|value| value.checked_add(bytes))
                    .is_none_or(|value| value > limits.upload_bytes)
                {
                    return Err(RuntimeError {
                        code: RuntimeErrorCode::Busy,
                        message: "Upload temporary storage is full".into(),
                        retryable: true,
                        current_version: None,
                        source_holds: Vec::new(),
                    });
                }
                state.reserved_upload_bytes += bytes;
            }
        }
        Ok(TempReservation {
            runtime: self,
            kind,
            bytes,
            committed: false,
        })
    }

    fn refresh_geometry_retained_accounting(&self) {
        let (bytes, mut error) = retained_geometry_inventory(&self.root);
        let mut state = self.temp_resources.lock().unwrap();
        // A failed inventory cannot authorize releasing previously observed
        // storage. Its uncertainty blocks geometry, not ordinary catalog use.
        let retained = if error.is_some() {
            bytes.max(state.retained_geometry_bytes)
        } else {
            bytes
        };
        let owned = state
            .owned_bytes
            .checked_sub(state.retained_geometry_bytes)
            .and_then(|active| active.checked_add(retained));
        if owned.is_none() {
            error = Some(geometry_inventory_error(
                "Retained geometry byte accounting overflowed",
            ));
        }
        state.owned_bytes = owned.unwrap_or(u64::MAX);
        state.retained_geometry_bytes = retained;
        state.geometry_inventory_error = error;
    }

    fn release_owned_bytes(&self, bytes: u64) {
        let mut state = self.temp_resources.lock().unwrap();
        state.owned_bytes = state.owned_bytes.saturating_sub(bytes);
    }

    fn release_upload_bytes(&self, bytes: u64) {
        let mut state = self.temp_resources.lock().unwrap();
        state.staged_upload_bytes = state.staged_upload_bytes.saturating_sub(bytes);
    }

    fn cleanup_upload(&self, upload: &UploadEntry) {
        let _ = hold_runtime_path(&upload.path);
        self.release_upload_bytes(upload.bytes);
    }

    fn prune_uploads(
        &self,
        uploads: &mut HashMap<String, UploadEntry>,
        now: SystemTime,
        epoch: u64,
    ) {
        uploads.retain(|_, upload| {
            let keep = upload.expires > now && upload.token.library_epoch == epoch;
            if !keep {
                self.cleanup_upload(upload);
            }
            keep
        });
    }

    fn cleanup_content_lease(&self, lease: &ContentLeaseEntry) {
        if let ContentBacking::ExistingPath {
            path,
            remove_on_drop: true,
        } = &lease.backing
        {
            let _ = hold_runtime_path(path);
            self.release_owned_bytes(lease.descriptor.bytes);
        }
    }

    fn prune_content_leases(
        &self,
        leases: &mut HashMap<String, ContentLeaseEntry>,
        now: SystemTime,
        epoch: u64,
    ) {
        leases.retain(|_, lease| {
            let keep = lease.expires > now && lease.descriptor.library_epoch == epoch;
            if !keep {
                self.cleanup_content_lease(lease);
            }
            keep
        });
    }

    /// Paths whose bytes are still backing valid content leases.
    ///
    /// Call while runtime maintenance is active before deleting files, so no new
    /// lease can be admitted between this snapshot and cleanup.
    pub fn active_content_lease_paths(&self) -> Vec<PathBuf> {
        let mut leases = self.leases.lock().unwrap();
        self.prune_content_leases(&mut leases, SystemTime::now(), self.library_epoch());
        leases
            .values()
            .filter_map(|lease| lease.backing.path().map(Path::to_path_buf))
            .collect()
    }

    #[cfg(test)]
    fn set_temp_limits_for_test(&self, owned_bytes: u64, upload_bytes: u64) {
        let state = self.temp_resources.lock().unwrap();
        assert_eq!(state.owned_bytes, 0);
        assert_eq!(state.reserved_owned_bytes, 0);
        assert_eq!(state.staged_upload_bytes, 0);
        assert_eq!(state.reserved_upload_bytes, 0);
        drop(state);
        *self.temp_limits.lock().unwrap() = TempResourceLimits {
            owned_bytes,
            upload_bytes,
            bundle_builds: 1,
        };
    }

    fn advance_epoch(&self) -> u64 {
        let epoch = self.library_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.record_mutation("libraryReplaced", Vec::new());
        self.cursors.lock().unwrap().clear();
        self.capture_cursors.lock().unwrap().clear();
        self.actions.lock().unwrap().clear();
        let leases = std::mem::take(&mut *self.leases.lock().unwrap());
        for lease in leases.into_values() {
            self.cleanup_content_lease(&lease);
        }
        self.bundle_previews.invalidate_all();
        let uploads = std::mem::take(&mut *self.uploads.lock().unwrap());
        for upload in uploads.into_values() {
            let _ = hold_runtime_path(&upload.path);
        }
        self.temp_resources.lock().unwrap().staged_upload_bytes = 0;
        for callback in self.invalidators.lock().unwrap().iter() {
            callback(epoch);
        }
        epoch
    }

    fn record_mutation(&self, kind: &str, entity_ids: Vec<String>) -> u64 {
        let revision = self.catalog_revision.fetch_add(1, Ordering::AcqRel) + 1;
        let mut events = self.events.lock().unwrap();
        events.push_back(RuntimeEvent {
            revision,
            library_epoch: self.library_epoch(),
            kind: kind.into(),
            entity_ids,
        });
        while events.len() > MAX_EVENTS {
            events.pop_front();
        }
        revision
    }
}

/// Unwired producer foundation: owns an existing Owned reservation without
/// retaining Runtime or changing its configured accounting category. Payloads
/// must drop their buffers/descriptors before this owner releases the charge.
#[allow(dead_code)]
pub(crate) struct OwnedTempReservation {
    state: Arc<Mutex<TempResourceState>>,
    bytes: u64,
}
impl std::fmt::Debug for OwnedTempReservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedTempReservation")
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}
impl Drop for OwnedTempReservation {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.reserved_owned_bytes = state
            .reserved_owned_bytes
            .checked_sub(self.bytes)
            .expect("Owned reservation accounting underflow");
    }
}

pub(crate) struct GeometryByteCharge {
    state: Arc<Mutex<TempResourceState>>,
    bytes: u64,
}
impl Drop for GeometryByteCharge {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.owned_bytes = state.owned_bytes.saturating_sub(self.bytes);
    }
}
impl TempReservation<'_> {
    /// Consume exactly this already-admitted reservation. No bytes are added,
    /// committed as generated files, or transferred from another category.
    #[allow(dead_code)]
    pub(crate) fn into_owned_reservation(mut self) -> Result<OwnedTempReservation, RuntimeError> {
        if !matches!(self.kind, ReservationKind::Owned) {
            return Err(internal("Only an Owned reservation can transfer ownership"));
        }
        let limits = *self.runtime.temp_limits.lock().unwrap();
        {
            let state = self.runtime.temp_resources.lock().unwrap();
            if state.reserved_owned_bytes < self.bytes {
                return Err(internal("Owned reservation accounting changed"));
            }
            if state
                .owned_bytes
                .checked_add(state.reserved_owned_bytes)
                .is_none_or(|bytes| bytes > limits.owned_bytes)
            {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Busy,
                    message: "Generated content temporary storage is full".into(),
                    retryable: true,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
        }
        self.committed = true;
        Ok(OwnedTempReservation {
            state: Arc::clone(&self.runtime.temp_resources),
            bytes: self.bytes,
        })
    }

    pub(crate) fn into_geometry_charge(
        mut self,
        actual_bytes: u64,
    ) -> Result<GeometryByteCharge, RuntimeError> {
        if !matches!(self.kind, ReservationKind::Geometry) || actual_bytes != self.bytes {
            return Err(internal(
                "Geometry reservation must transfer its exact byte count",
            ));
        }
        let limits = *self.runtime.temp_limits.lock().unwrap();
        let mut state = self.runtime.temp_resources.lock().unwrap();
        let owned = state
            .owned_bytes
            .checked_add(actual_bytes)
            .ok_or_else(|| internal("Geometry byte accounting overflow"))?;
        let other_reserved = state
            .reserved_owned_bytes
            .checked_sub(self.bytes)
            .ok_or_else(|| internal("Geometry reservation accounting changed"))?;
        if owned
            .checked_add(other_reserved)
            .is_none_or(|value| value > limits.owned_bytes)
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Busy,
                message: "Generated content temporary storage is full".into(),
                retryable: true,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        state.reserved_owned_bytes = other_reserved;
        state.owned_bytes = owned;
        self.committed = true;
        Ok(GeometryByteCharge {
            state: Arc::clone(&self.runtime.temp_resources),
            bytes: actual_bytes,
        })
    }

    fn commit(mut self, actual_bytes: u64) -> Result<(), RuntimeError> {
        let limits = *self.runtime.temp_limits.lock().unwrap();
        let mut state = self.runtime.temp_resources.lock().unwrap();
        match self.kind {
            ReservationKind::Bundle => {
                state.reserved_owned_bytes = state.reserved_owned_bytes.saturating_sub(self.bytes);
                state.active_bundle_builds = state.active_bundle_builds.saturating_sub(1);
                self.committed = true;
                if state
                    .owned_bytes
                    .checked_add(actual_bytes)
                    .is_none_or(|value| value > limits.owned_bytes)
                {
                    return Err(RuntimeError {
                        code: RuntimeErrorCode::Busy,
                        message: "Bundle archive temporary storage is full".into(),
                        retryable: true,
                        current_version: None,
                        source_holds: Vec::new(),
                    });
                }
                state.owned_bytes += actual_bytes;
            }
            ReservationKind::Owned => {
                state.reserved_owned_bytes = state.reserved_owned_bytes.saturating_sub(self.bytes);
                self.committed = true;
                if state
                    .owned_bytes
                    .checked_add(actual_bytes)
                    .is_none_or(|value| value > limits.owned_bytes)
                {
                    return Err(RuntimeError {
                        code: RuntimeErrorCode::Busy,
                        message: "Generated content temporary storage is full".into(),
                        retryable: true,
                        current_version: None,
                        source_holds: Vec::new(),
                    });
                }
                state.owned_bytes += actual_bytes;
            }
            ReservationKind::Geometry => {
                return Err(internal(
                    "Geometry must transfer its reservation into a byte charge",
                ));
            }
            ReservationKind::Upload => {
                state.reserved_upload_bytes =
                    state.reserved_upload_bytes.saturating_sub(self.bytes);
                self.committed = true;
                if state
                    .staged_upload_bytes
                    .checked_add(actual_bytes)
                    .is_none_or(|value| value > limits.upload_bytes)
                {
                    return Err(RuntimeError {
                        code: RuntimeErrorCode::Busy,
                        message: "Upload temporary storage is full".into(),
                        retryable: true,
                        current_version: None,
                        source_holds: Vec::new(),
                    });
                }
                state.staged_upload_bytes += actual_bytes;
            }
        }
        Ok(())
    }
}

impl Drop for TempReservation<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut state = self.runtime.temp_resources.lock().unwrap();
        match self.kind {
            ReservationKind::Bundle => {
                state.reserved_owned_bytes = state.reserved_owned_bytes.saturating_sub(self.bytes);
                state.active_bundle_builds = state.active_bundle_builds.saturating_sub(1);
            }
            ReservationKind::Owned | ReservationKind::Geometry => {
                state.reserved_owned_bytes = state.reserved_owned_bytes.saturating_sub(self.bytes);
            }
            ReservationKind::Upload => {
                state.reserved_upload_bytes =
                    state.reserved_upload_bytes.saturating_sub(self.bytes);
            }
        }
    }
}

fn prepare_lazy_desktop_root(path: &Path) -> Result<PathBuf, RuntimeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(invalid("Library root must be a real directory"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|_| io_error())?;
        }
        Err(_) => return Err(io_error()),
    }
    path.canonicalize().map_err(|_| io_error())
}

fn geometry_inventory_error(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Busy,
        message: message.into(),
        retryable: true,
        current_version: None,
        source_holds: Vec::new(),
    }
}
/// Retained legacy/unknown files are never adopted as generated ownership.
/// Count regular bytes without following names; uncertainty disables only new geometry.
fn retained_geometry_inventory(root: &Path) -> (u64, Option<RuntimeError>) {
    // The producer holds this same gate only while its empty named inode is
    // being captured/unlinked. Release it before taking temporary-resource state.
    let _creation = match secure_fs::geometry_inventory_guard() {
        Ok(guard) => guard,
        Err(_) => {
            return (
                0,
                Some(geometry_inventory_error(
                    "Geometry storage synchronization is unavailable",
                )),
            )
        }
    };
    retained_geometry_inventory_under_gate(root)
}
fn retained_geometry_inventory_under_gate(root: &Path) -> (u64, Option<RuntimeError>) {
    let namespace = root.join("staging/geometry");
    let metadata = match fs::symlink_metadata(&namespace) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return (0, None),
        Err(_) => {
            return (
                0,
                Some(geometry_inventory_error(
                    "Geometry storage could not be inspected",
                )),
            )
        }
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || namespace.canonicalize().ok().as_deref() != Some(namespace.as_path())
    {
        return (
            0,
            Some(geometry_inventory_error(
                "Retained geometry namespace is unsafe; its files were preserved",
            )),
        );
    }
    let directory = match secure_fs::Directory::open(&namespace) {
        Ok(directory) => directory,
        Err(_) => {
            return (
                0,
                Some(geometry_inventory_error(
                    "Geometry storage could not be admitted",
                )),
            )
        }
    };
    let names = match directory.read_entry_names(256) {
        Ok(names) => names,
        Err(_) => {
            return (
                0,
                Some(geometry_inventory_error(
                    "Retained geometry storage exceeds its entry limit or cannot be enumerated",
                )),
            )
        }
    };
    let mut bytes = 0u64;
    let mut issue = None;
    for name in names {
        match directory.open_file(&name).and_then(|file|file.metadata()) {
            Ok(metadata)=>match bytes.checked_add(metadata.len()) {
                Some(total)=>bytes=total,
                None=>return (u64::MAX,Some(geometry_inventory_error("Retained geometry storage exceeds its byte range"))),
            },
            Err(_)=>issue=Some(geometry_inventory_error("Retained geometry contains an unsafe or nonregular entry; all files were preserved")),
        }
    }
    if !directory
        .matches_path(&namespace)
        .is_ok_and(|matches| matches)
    {
        issue = Some(geometry_inventory_error(
            "Retained geometry namespace changed during inspection",
        ));
    }
    (bytes, issue)
}

fn cleanup_runtime_orphans(root: &Path) -> Result<(), RuntimeError> {
    for (relative, prefix, suffix) in [("staging/bundle-downloads", "bundle-", ".zip")] {
        let directory = root.join(relative);
        let metadata = match fs::symlink_metadata(&directory) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(io_error()),
        };
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || directory.canonicalize().map_err(|_| io_error())? != directory
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Runtime temporary namespace is unsafe".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        let directory_handle = secure_fs::Directory::open(&directory).map_err(|_| io_error())?;
        for (index, entry) in fs::read_dir(&directory)
            .map_err(|_| io_error())?
            .enumerate()
        {
            if index >= 10_000 {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Busy,
                    message: "Runtime temporary namespace has too many entries".into(),
                    retryable: true,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            let entry = entry.map_err(|_| io_error())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with(prefix) || !name.ends_with(suffix) {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path()).map_err(|_| io_error())?;
            if metadata.is_file() && !metadata.file_type().is_symlink() {
                directory_handle
                    .hold_entry(&entry.file_name())
                    .map_err(|_| io_error())?;
            }
        }
    }
    Ok(())
}

impl RuntimeInfoEngine for LibraryRuntime {
    fn runtime_info(&self, context: &RequestContext) -> Result<RuntimeInfo, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        Ok(RuntimeInfo {
            library_epoch: self.library_epoch(),
            catalog_revision: self.catalog_revision(),
            ownership: self.ownership_mode(),
            capabilities: self
                .capabilities
                .intersection(&context.capabilities)
                .filter(|capability| {
                    **capability != Capability::PrinterNetwork || self.printer_network_enabled()
                })
                .cloned()
                .collect(),
            roots: self.roots.iter().map(|root| root.handle.clone()).collect(),
        })
    }
}

impl SettingsEngine for LibraryRuntime {
    fn list_index_root_settings(
        &self,
        context: &RequestContext,
    ) -> Result<Vec<IndexRootSettings>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        let catalog = self.read_db(|db| db.list_index_roots())?;
        Ok(self
            .roots
            .iter()
            .filter_map(|admitted| {
                let catalog_root_id = admitted.catalog_root_id.as_ref()?;
                let root = catalog.iter().find(|root| &root.id == catalog_root_id)?;
                (root.path == admitted.path).then(|| IndexRootSettings {
                    handle_id: admitted.handle.id.clone(),
                    catalog_root_id: root.id.clone(),
                    label: admitted.handle.label.clone(),
                    access: admitted.handle.access,
                    enabled: root.enabled,
                    last_scanned_at: root.last_scanned_at.clone(),
                    scan_interval_minutes: root.scan_interval_minutes,
                    max_file_bytes: root.max_file_bytes,
                    scan_zip: root.scan_zip,
                })
            })
            .collect())
    }

    fn update_index_root_settings(
        &self,
        context: &RequestContext,
        handle_id: &str,
        patch: IndexRootSettingsPatch,
    ) -> Result<IndexRootSettings, RuntimeError> {
        self.require(context, Capability::Scan)?;
        let admitted = self
            .roots
            .iter()
            .find(|root| root.handle.id == handle_id)
            .ok_or_else(|| RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Root handle is not admitted".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })?;
        let catalog_root_id = admitted
            .catalog_root_id
            .as_deref()
            .ok_or_else(|| RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Root handle is not a catalog root".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })?;
        let updated = self.mutate_db(|db| {
            let existing = db
                .list_index_roots()?
                .into_iter()
                .find(|root| root.id == catalog_root_id && root.path == admitted.path)
                .ok_or_else(|| LibraryError::IndexRootNotFound(catalog_root_id.into()))?;
            db.update_index_root(&library_core::IndexRoot {
                enabled: patch.enabled,
                scan_interval_minutes: patch.scan_interval_minutes,
                max_file_bytes: patch.max_file_bytes,
                scan_zip: patch.scan_zip,
                ..existing
            })
        })?;
        Ok(IndexRootSettings {
            handle_id: admitted.handle.id.clone(),
            catalog_root_id: updated.id,
            label: admitted.handle.label.clone(),
            access: admitted.handle.access,
            enabled: updated.enabled,
            last_scanned_at: updated.last_scanned_at,
            scan_interval_minutes: updated.scan_interval_minutes,
            max_file_bytes: updated.max_file_bytes,
            scan_zip: updated.scan_zip,
        })
    }

    fn get_catalog_settings(
        &self,
        context: &RequestContext,
    ) -> Result<CatalogSettings, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.get_catalog_settings())
            .map(catalog_settings)
    }

    fn update_catalog_settings(
        &self,
        context: &RequestContext,
        settings: CatalogSettings,
    ) -> Result<CatalogSettings, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let settings = CoreCatalogSettings {
            thumbnail_size: settings.thumbnail_size,
            thumbnail_background: settings.thumbnail_background,
            thumbnail_concurrency: settings.thumbnail_concurrency,
            scan_batch_size: settings.scan_batch_size,
        };
        self.mutate_db(|db| db.update_catalog_settings(&settings))
            .map(catalog_settings)
    }
}

impl ManagementEngine for LibraryRuntime {
    fn discard_action(&self, context: &RequestContext, token: &str) -> Result<(), RuntimeError> {
        thumbnail_cleanup::prune(self);
        let mut actions = self.actions.lock().unwrap();
        let planned = actions
            .get(token)
            .ok_or_else(|| stale("Management plan is unknown or already used"))?;
        if planned.principal_id != context.principal_id {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Management plan belongs to another principal".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        let planned = actions.remove(token);
        drop(actions);
        drop(planned);
        Ok(())
    }
    fn plan_action(
        &self,
        context: &RequestContext,
        action: ManagementAction,
    ) -> Result<ActionPlan, RuntimeError> {
        match &action {
            ManagementAction::CleanupOrphans { continuation } => {
                return thumbnail_cleanup::review(
                    self,
                    context,
                    library_protocol::ThumbnailCleanupRequest {
                        mode: library_protocol::ThumbnailCleanupMode::Orphans,
                        continuation: continuation.clone(),
                    },
                )
            }
            ManagementAction::ResetDefaultThumbnails => {
                return thumbnail_cleanup::review(
                    self,
                    context,
                    library_protocol::ThumbnailCleanupRequest {
                        mode: library_protocol::ThumbnailCleanupMode::ResetDefaults,
                        continuation: None,
                    },
                )
            }
            _ => {}
        }
        thumbnail_cleanup::prune(self);
        let action_kind = match &action {
            ManagementAction::RecoverableRemove { .. } => {
                self.require(context, Capability::CatalogWrite)?;
                "recoverableRemove"
            }
            ManagementAction::RestoreRemoval { .. } => {
                self.require(context, Capability::CatalogWrite)?;
                "restoreRemoval"
            }
            ManagementAction::CreateBackup { .. } => {
                self.require(context, Capability::Maintenance)?;
                "createBackup"
            }
            ManagementAction::RestoreBackup { .. } => {
                self.require(context, Capability::Maintenance)?;
                "restoreBackup"
            }
            ManagementAction::ExportSelection { .. } => {
                self.require(context, Capability::Maintenance)?;
                "exportSelection"
            }
            ManagementAction::ExportLibraryJson { .. } => {
                self.require(context, Capability::Maintenance)?;
                self.require(context, Capability::BackupWrite)?;
                "exportLibrary"
            }
            ManagementAction::QuarantineLooseSource { .. } => {
                require_durable_source_mutations()?;
                self.require(context, Capability::CatalogWrite)?;
                self.require(context, Capability::PhysicalSourceWrite)?;
                "quarantineLooseSource"
            }
            ManagementAction::QuarantineLooseSources { .. } => {
                require_durable_source_mutations()?;
                self.require(context, Capability::CatalogWrite)?;
                self.require(context, Capability::PhysicalSourceWrite)?;
                "quarantineLooseSources"
            }
            ManagementAction::UndoQuarantine { .. } => {
                require_durable_source_mutations()?;
                self.require(context, Capability::CatalogWrite)?;
                self.require(context, Capability::PhysicalSourceWrite)?;
                "undoQuarantine"
            }
            ManagementAction::MoveLooseSource { .. } => {
                require_durable_source_mutations()?;
                self.require(context, Capability::CatalogWrite)?;
                self.require(context, Capability::PhysicalSourceWrite)?;
                "moveLooseSource"
            }
            ManagementAction::MoveLooseSources { .. } => {
                require_durable_source_mutations()?;
                self.require(context, Capability::CatalogWrite)?;
                self.require(context, Capability::PhysicalSourceWrite)?;
                "moveLooseSources"
            }
            ManagementAction::PurgeCatalog { .. } => {
                require_durable_source_mutations()?;
                self.require(context, Capability::CatalogWrite)?;
                self.require(context, Capability::Maintenance)?;
                self.require(context, Capability::BackupWrite)?;
                "purgeCatalog"
            }
            ManagementAction::CleanupOrphans { .. } | ManagementAction::ResetDefaultThumbnails => {
                require_durable_source_mutations()?;
                self.require(context, Capability::Maintenance)?;
                self.require(context, Capability::MediaWrite)?;
                "cleanupOrphans"
            }
        };
        let target_requests: Vec<VersionedTarget> = match &action {
            ManagementAction::RecoverableRemove { targets }
            | ManagementAction::ExportSelection { targets, .. }
            | ManagementAction::QuarantineLooseSources { targets } => targets.clone(),
            ManagementAction::QuarantineLooseSource { target }
            | ManagementAction::MoveLooseSource { target, .. } => vec![target.clone()],
            ManagementAction::MoveLooseSources { targets, .. } => targets
                .iter()
                .map(|target| VersionedTarget {
                    model_id: target.model_id.clone(),
                    expected_version: target.expected_version,
                })
                .collect(),
            _ => Vec::new(),
        };
        let is_physical_batch = matches!(
            &action,
            ManagementAction::QuarantineLooseSources { .. }
                | ManagementAction::MoveLooseSources { .. }
        );
        if is_physical_batch && target_requests.len() > MAX_PHYSICAL_MUTATION_TARGETS {
            return Err(invalid("Physical source target count exceeds its limit"));
        }
        if target_requests.len() > 50_000 {
            return Err(invalid("Management target count exceeds its limit"));
        }
        let targets = self.read_db(|db| {
            target_requests
                .iter()
                .map(|target| {
                    let model = db
                        .get_model(&target.model_id)?
                        .ok_or_else(|| LibraryError::ModelNotFound(target.model_id.clone()))?;
                    if model.metadata_version != target.expected_version {
                        return Err(LibraryError::MetadataConflict {
                            model_id: target.model_id.clone(),
                            expected: target.expected_version,
                            actual: model.metadata_version,
                        });
                    }
                    Ok(ActionTargetSnapshot {
                        model_id: model.id,
                        metadata_version: model.metadata_version,
                        content_hash: model.content_hash,
                    })
                })
                .collect::<Result<Vec<_>, LibraryError>>()
        })?;
        validate_management_action(self, context, &action)?;
        let physical_sources = match &action {
            ManagementAction::QuarantineLooseSource { target }
            | ManagementAction::MoveLooseSource { target, .. } => {
                vec![snapshot_active_source(self, context, target)?]
            }
            ManagementAction::QuarantineLooseSources { targets } => targets
                .iter()
                .map(|target| snapshot_active_source(self, context, target))
                .collect::<Result<Vec<_>, _>>()?,
            ManagementAction::MoveLooseSources { targets, .. } => targets
                .iter()
                .map(|target| {
                    snapshot_active_source(
                        self,
                        context,
                        &VersionedTarget {
                            model_id: target.model_id.clone(),
                            expected_version: target.expected_version,
                        },
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
            ManagementAction::UndoQuarantine { snapshot_id } => {
                snapshot_quarantine_sources(self, context, snapshot_id)?
            }
            _ => Vec::new(),
        };
        let managed_assets = if matches!(&action, ManagementAction::PurgeCatalog { .. }) {
            snapshot_managed_library_assets(self)?
        } else {
            Vec::new()
        };
        if is_physical_batch {
            validate_physical_source_batch(&physical_sources)?;
        }
        let input_fingerprint = if let ManagementAction::RestoreBackup {
            source_handle_id,
            file_name,
            ..
        } = &action
        {
            let (_, source) = admitted_file(
                self,
                context,
                source_handle_id,
                RootAccess::Inbox,
                file_name,
                true,
            )
            .or_else(|_| {
                admitted_file(
                    self,
                    context,
                    source_handle_id,
                    RootAccess::ReadOnly,
                    file_name,
                    true,
                )
            })?;
            Some(hash_management_file(
                &source,
                8 * 1024 * 1024 * 1024 + 16 * 1024 * 1024,
            )?)
        } else {
            None
        };
        let expires = SystemTime::now() + Duration::from_secs(5 * 60);
        let token = uuid::Uuid::new_v4().simple().to_string();
        let library_epoch = self.library_epoch();
        let catalog_revision = self.catalog_revision();
        let mut actions = self.actions.lock().unwrap();
        actions.retain(|_, action| action.expires > SystemTime::now());
        if actions.len() >= 64 {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Busy,
                message: "Management plan limit reached".into(),
                retryable: true,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        let impact = match &action {
            ManagementAction::PurgeCatalog { .. } => vec![
                "A verified backup is written before the active catalog is replaced".into(),
                "Configured index roots and their original model files are preserved".into(),
                "The previous managed Library directory is retained for recovery".into(),
            ],
            _ => vec!["The action will be rejected if the catalog changes before execution".into()],
        };
        actions.insert(
            token.clone(),
            PlannedAction {
                action,
                principal_id: context.principal_id.clone(),
                expires,
                library_epoch,
                catalog_revision,
                targets: targets.clone(),
                input_fingerprint: input_fingerprint.clone(),
                physical_sources,
                managed_assets: managed_assets.clone(),
                thumbnail_cleanup: None,
            },
        );
        Ok(ActionPlan {
            token,
            action_kind: action_kind.into(),
            expires_at: format_system_time(expires)?,
            library_epoch,
            catalog_revision,
            targets,
            impact,
            input_bytes: input_fingerprint.as_ref().map(|value| value.0),
            input_sha256: input_fingerprint.map(|value| value.1),
            thumbnail_review: None,
        })
    }

    fn execute_action(
        &self,
        context: &RequestContext,
        token: &str,
    ) -> Result<ManagementResult, RuntimeError> {
        thumbnail_cleanup::prune(self);
        let mut actions = self.actions.lock().unwrap();
        let planned = actions
            .get(token)
            .ok_or_else(|| stale("Management plan is unknown or already used"))?;
        if planned.principal_id != context.principal_id {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Management plan belongs to another principal".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        let mut planned = actions.remove(token).unwrap();
        drop(actions);
        if planned.expires <= SystemTime::now()
            || planned.library_epoch != self.library_epoch()
            || planned.catalog_revision != self.catalog_revision()
        {
            return Err(stale("Management plan is stale"));
        }
        if let Some(cleanup) = planned.thumbnail_cleanup.take() {
            return thumbnail_cleanup::execute(
                self,
                context,
                cleanup,
                planned.library_epoch,
                planned.catalog_revision,
            );
        }
        revalidate_action_targets(self, &planned.targets)?;
        #[cfg(test)]
        if let Some(hook) = self.before_action_dispatch.lock().unwrap().take() {
            hook();
        }
        match planned.action {
            ManagementAction::RecoverableRemove { targets } => {
                self.require(context, Capability::CatalogWrite)?;
                let targets = targets
                    .into_iter()
                    .map(|target| CoreVersionedTarget {
                        model_id: target.model_id,
                        expected_version: target.expected_version,
                    })
                    .collect::<Vec<_>>();
                let result = self.mutate_db(|db| db.remove_models_recoverably(&targets))?;
                Ok(ManagementResult::Removed {
                    snapshot_id: result.snapshot_id,
                    removed_ids: result.removed_ids,
                })
            }
            ManagementAction::RestoreRemoval { snapshot_id } => {
                self.require(context, Capability::CatalogWrite)?;
                let models = self.mutate_db(|db| db.restore_removal(&snapshot_id))?;
                Ok(ManagementResult::RestoredRemoval {
                    models: models.into_iter().map(mutation_version).collect(),
                })
            }
            ManagementAction::CreateBackup {
                destination_handle_id,
                file_name,
            } => execute_backup(
                self,
                context,
                &destination_handle_id,
                &file_name,
                planned.library_epoch,
                planned.catalog_revision,
            ),
            ManagementAction::ExportSelection {
                targets,
                destination_handle_id,
                file_name,
            } => execute_selection_export(
                self,
                context,
                &targets,
                &destination_handle_id,
                &file_name,
                planned.library_epoch,
                planned.catalog_revision,
            ),
            ManagementAction::ExportLibraryJson {
                destination_handle_id,
                file_name,
            } => execute_library_export(
                self,
                context,
                &destination_handle_id,
                &file_name,
                planned.library_epoch,
                planned.catalog_revision,
            ),
            ManagementAction::RestoreBackup {
                source_handle_id,
                file_name,
                root_mappings,
            } => execute_restore(
                self,
                context,
                RestoreExecution {
                    source_handle_id: &source_handle_id,
                    file_name: &file_name,
                    root_mappings: &root_mappings,
                    expected_input: planned
                        .input_fingerprint
                        .as_ref()
                        .ok_or_else(|| internal("Restore plan lost its input fingerprint"))?,
                    expected_epoch: planned.library_epoch,
                    expected_revision: planned.catalog_revision,
                },
            ),
            ManagementAction::QuarantineLooseSource { target } => execute_quarantine_batch(
                self,
                context,
                std::slice::from_ref(&target),
                &planned.physical_sources,
                planned.library_epoch,
                planned.catalog_revision,
            ),
            ManagementAction::QuarantineLooseSources { targets } => execute_quarantine_batch(
                self,
                context,
                &targets,
                &planned.physical_sources,
                planned.library_epoch,
                planned.catalog_revision,
            ),
            ManagementAction::UndoQuarantine { snapshot_id } => execute_undo_quarantine_batch(
                self,
                context,
                &snapshot_id,
                &planned.physical_sources,
                planned.library_epoch,
                planned.catalog_revision,
            ),
            ManagementAction::MoveLooseSource {
                target,
                destination_handle_id,
                file_name,
            } => execute_move_batch(
                self,
                context,
                &[MoveLooseSourceTarget {
                    model_id: target.model_id,
                    expected_version: target.expected_version,
                    file_name,
                }],
                &destination_handle_id,
                &planned.physical_sources,
                planned.library_epoch,
                planned.catalog_revision,
            ),
            ManagementAction::MoveLooseSources {
                targets,
                destination_handle_id,
            } => execute_move_batch(
                self,
                context,
                &targets,
                &destination_handle_id,
                &planned.physical_sources,
                planned.library_epoch,
                planned.catalog_revision,
            ),
            ManagementAction::PurgeCatalog {
                backup_destination_handle_id,
                backup_file_name,
            } => execute_purge_catalog(
                self,
                context,
                &backup_destination_handle_id,
                &backup_file_name,
                &planned.managed_assets,
                planned.library_epoch,
                planned.catalog_revision,
            ),
            ManagementAction::CleanupOrphans { .. } | ManagementAction::ResetDefaultThumbnails => {
                Err(internal("Preview cleanup plan is missing"))
            }
        }
    }

    fn plan_library_export(
        &self,
        context: &RequestContext,
        dest_path: Option<&str>,
    ) -> Result<ActionPlan, RuntimeError> {
        self.require(context, Capability::Maintenance)?;
        self.require(context, Capability::BackupWrite)?;
        let (destination_handle_id, file_name) =
            resolve_library_export_destination(self, context, dest_path)?;
        self.plan_action(
            context,
            ManagementAction::ExportLibraryJson {
                destination_handle_id,
                file_name,
            },
        )
    }
}

impl CatalogEngine for LibraryRuntime {
    fn get_model(
        &self,
        context: &RequestContext,
        model_id: &str,
    ) -> Result<CatalogModel, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| {
            let model = db
                .get_model(model_id)?
                .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))?;
            let thumbnail_id = db
                .list_thumbnail_gallery(model_id)?
                .into_iter()
                .find(|thumbnail| thumbnail.is_active)
                .map(|thumbnail| thumbnail.id);
            Ok(catalog_model(model, thumbnail_id))
        })
    }

    fn list_models(
        &self,
        context: &RequestContext,
        query: ModelQuery,
        page: PageRequest,
    ) -> Result<Page<CatalogModel>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        if page.limit == 0 || page.limit > MAX_PAGE_LIMIT {
            return Err(invalid("Page limit is outside the supported range"));
        }
        if query
            .offset
            .is_some_and(|offset| offset > MAX_MODEL_PAGE_OFFSET)
            || (page.cursor.is_some() && query.offset.is_some())
        {
            return Err(invalid("Model page offset is invalid for this request"));
        }
        let offset = query.offset.unwrap_or(0);
        let query = ModelQuery {
            offset: None,
            ..query
        };
        let revision = self.catalog_revision();
        let cursor = if let Some(token) = page.cursor.as_deref() {
            let cursor = self
                .cursors
                .lock()
                .unwrap()
                .get(token)
                .cloned()
                .ok_or_else(|| stale("Catalog cursor is unknown or expired"))?;
            if cursor.revision != revision || cursor.query != query {
                return Err(stale("Catalog cursor became stale"));
            }
            Some(cursor)
        } else {
            None
        };
        let core_query = core_model_query(&query);
        let (models, has_more) = self.read_db(|db| {
            let (models, has_more) = db.list_models_page_with_offset(
                &core_query,
                cursor.as_ref().map(|cursor| cursor.sort_value.as_str()),
                cursor.as_ref().map(|cursor| cursor.model_id.as_str()),
                page.limit,
                offset,
            )?;
            let models = models
                .into_iter()
                .map(|model| {
                    let thumbnail_id = db
                        .list_thumbnail_gallery(&model.id)?
                        .into_iter()
                        .find(|thumbnail| thumbnail.is_active)
                        .map(|thumbnail| thumbnail.id);
                    Ok((model, thumbnail_id))
                })
                .collect::<Result<Vec<_>, LibraryError>>()?;
            Ok((models, has_more))
        })?;
        if self.catalog_revision() != revision {
            return Err(stale("Catalog changed while the page was being read"));
        }
        let next_cursor = if has_more {
            let last = &models
                .last()
                .ok_or_else(|| internal("Paged model query lost its final item"))?
                .0;
            let token = uuid::Uuid::new_v4().simple().to_string();
            let mut cursors = self.cursors.lock().unwrap();
            cursors.retain(|_, cursor| cursor.revision == revision);
            if cursors.len() >= MAX_CURSORS {
                if let Some(oldest) = cursors.keys().next().cloned() {
                    cursors.remove(&oldest);
                }
            }
            cursors.insert(
                token.clone(),
                ModelCursor {
                    sort_value: model_sort_value(last, query.sort),
                    model_id: last.id.clone(),
                    revision,
                    query,
                },
            );
            Some(token)
        } else {
            None
        };
        Ok(Page {
            items: models
                .into_iter()
                .map(|(model, thumbnail_id)| catalog_model(model, thumbnail_id))
                .collect(),
            next_cursor,
            snapshot_revision: revision,
        })
    }

    fn catalog_stats(&self, context: &RequestContext) -> Result<CatalogStats, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.catalog_stats()).map(catalog_stats)
    }

    fn catalog_facets(&self, context: &RequestContext) -> Result<CatalogFacets, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.catalog_facets()).map(catalog_facets)
    }
}

impl MetadataEngine for LibraryRuntime {
    fn rename_metadata_value(
        &self,
        context: &RequestContext,
        kind: MetadataValueKind,
        old_name: &str,
        new_name: &str,
    ) -> Result<u64, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| {
            db.rename_metadata_value(metadata_value_kind_name(kind), old_name, new_name)
        })
    }

    fn delete_metadata_value(
        &self,
        context: &RequestContext,
        kind: MetadataValueKind,
        name: &str,
    ) -> Result<u64, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.delete_metadata_value(metadata_value_kind_name(kind), name))
    }

    fn update_metadata(
        &self,
        context: &RequestContext,
        target: VersionedTarget,
        patch: MetadataPatch,
    ) -> Result<MutationVersion, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let patch = core_metadata_patch(patch);
        let model = self.mutate_db(|db| {
            db.update_model_metadata(&target.model_id, target.expected_version, &patch)
        })?;
        Ok(mutation_version(model))
    }

    fn bulk_update_metadata(
        &self,
        context: &RequestContext,
        targets: Vec<VersionedTarget>,
        patch: MetadataPatch,
    ) -> Result<Vec<MutationVersion>, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let targets = targets
            .into_iter()
            .map(|target| CoreVersionedTarget {
                model_id: target.model_id,
                expected_version: target.expected_version,
            })
            .collect::<Vec<_>>();
        let patch = core_metadata_patch(patch);
        self.mutate_db(|db| db.bulk_update_metadata(&targets, &patch))
            .map(|models| models.into_iter().map(mutation_version).collect())
    }

    fn update_metadata_batch(
        &self,
        context: &RequestContext,
        updates: Vec<VersionedMetadataUpdate>,
    ) -> Result<Vec<MutationVersion>, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        if updates.is_empty() || updates.len() > 50_000 {
            return Err(invalid(
                "Metadata update batch must contain 1 to 50000 targets",
            ));
        }
        let updates = updates
            .into_iter()
            .map(|update| CoreVersionedModelUpdate {
                model_id: update.model_id,
                expected_version: update.expected_version,
                patch: core_metadata_patch(update.patch),
                tags: update.tags,
                filament_ids: update.filament_ids,
            })
            .collect::<Vec<_>>();
        self.mutate_db(|db| db.update_models_metadata(&updates))
            .map(|models| models.into_iter().map(mutation_version).collect())
    }
}

fn metadata_value_kind_name(kind: MetadataValueKind) -> &'static str {
    match kind {
        MetadataValueKind::Designer => "designer",
        MetadataValueKind::ParentModel => "parentModel",
        MetadataValueKind::License => "license",
    }
}

impl TagEngine for LibraryRuntime {
    fn list_tags(&self, context: &RequestContext) -> Result<Vec<Tag>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.list_tags()).map(|tags| {
            tags.into_iter()
                .map(|tag| Tag {
                    id: tag.id,
                    name: tag.name,
                    category: tag.category,
                })
                .collect()
        })
    }

    fn add_model_tags_by_name(
        &self,
        context: &RequestContext,
        target: VersionedTarget,
        names: Vec<String>,
    ) -> Result<MutationVersion, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| {
            db.add_model_tags_by_name(&target.model_id, target.expected_version, &names)
        })
        .map(|model| MutationVersion {
            model_id: model.id,
            metadata_version: model.metadata_version,
        })
    }

    fn remove_model_tags_by_name(
        &self,
        context: &RequestContext,
        target: VersionedTarget,
        names: Vec<String>,
    ) -> Result<MutationVersion, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| {
            db.remove_model_tags_by_name(&target.model_id, target.expected_version, &names)
        })
        .map(|model| MutationVersion {
            model_id: model.id,
            metadata_version: model.metadata_version,
        })
    }

    fn create_tag(
        &self,
        context: &RequestContext,
        name: String,
        category: Option<String>,
    ) -> Result<Tag, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.create_tag(&name, category.as_deref()))
            .map(|tag| Tag {
                id: tag.id,
                name: tag.name,
                category: tag.category,
            })
    }

    fn update_tag(
        &self,
        context: &RequestContext,
        tag_id: &str,
        name: String,
        category: Option<String>,
    ) -> Result<Tag, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.rename_tag(tag_id, &name, category.as_deref()))
            .map(|tag| Tag {
                id: tag.id,
                name: tag.name,
                category: tag.category,
            })
    }

    fn delete_tag(&self, context: &RequestContext, tag_id: &str) -> Result<(), RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.delete_tag(tag_id))
    }
}

impl FolderEngine for LibraryRuntime {
    fn list_folders(&self, context: &RequestContext) -> Result<Vec<Folder>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.list_folders())
            .map(|folders| folders.into_iter().map(folder).collect())
    }

    fn create_folder(
        &self,
        context: &RequestContext,
        name: String,
        parent_id: Option<String>,
    ) -> Result<Folder, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.create_folder(&name, parent_id.as_deref()))
            .map(folder)
    }

    fn rename_folder(
        &self,
        context: &RequestContext,
        folder_id: &str,
        name: String,
    ) -> Result<Folder, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.rename_folder(folder_id, &name))
            .map(folder)
    }

    fn move_folder(
        &self,
        context: &RequestContext,
        folder_id: &str,
        parent_id: &str,
    ) -> Result<Folder, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.move_folder(folder_id, parent_id))
            .map(folder)
    }

    fn delete_folder(&self, context: &RequestContext, folder_id: &str) -> Result<(), RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.delete_folder(folder_id))
    }
}

impl FilamentEngine for LibraryRuntime {
    fn list_filaments(
        &self,
        context: &RequestContext,
        include_unavailable: bool,
    ) -> Result<Vec<Filament>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.list_filaments(include_unavailable))
            .map(|filaments| filaments.into_iter().map(filament).collect())
    }

    fn create_filament(
        &self,
        context: &RequestContext,
        input: FilamentInput,
    ) -> Result<Filament, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = core_filament_input(input);
        self.mutate_db(|db| db.create_filament(&input))
            .map(filament)
    }

    fn update_filament(
        &self,
        context: &RequestContext,
        filament_id: &str,
        expected_version: u64,
        input: FilamentInput,
    ) -> Result<Filament, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = core_filament_input(input);
        self.mutate_db(|db| db.update_filament(filament_id, expected_version, &input))
            .map(filament)
    }

    fn delete_filament(
        &self,
        context: &RequestContext,
        filament_id: &str,
        expected_version: u64,
    ) -> Result<Option<Filament>, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.delete_filament(filament_id, expected_version))
            .map(|value| value.map(filament))
    }

    fn bulk_assign_model_filaments(
        &self,
        context: &RequestContext,
        assignments: Vec<ModelFilamentAssignment>,
    ) -> Result<Vec<MutationVersion>, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let assignments = assignments
            .into_iter()
            .map(|assignment| CoreModelFilamentAssignment {
                target: CoreVersionedTarget {
                    model_id: assignment.target.model_id,
                    expected_version: assignment.target.expected_version,
                },
                filament_ids: assignment.filament_ids,
            })
            .collect::<Vec<_>>();
        self.mutate_db(|db| db.bulk_assign_model_filaments(&assignments))
            .map(|models| models.into_iter().map(mutation_version).collect())
    }
}

impl PartStockEngine for LibraryRuntime {
    fn list_part_stock(&self, context: &RequestContext) -> Result<Vec<PartStock>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.list_part_stock())
            .map(|values| values.into_iter().map(part_stock).collect())
    }

    fn create_part_stock(
        &self,
        context: &RequestContext,
        input: PartStockInput,
    ) -> Result<PartStock, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = core_part_stock_input(input);
        self.mutate_db(|db| db.create_part_stock(&input))
            .map(part_stock)
    }

    fn update_part_stock(
        &self,
        context: &RequestContext,
        part_id: &str,
        expected_version: u64,
        input: PartStockInput,
    ) -> Result<PartStock, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = core_part_stock_input(input);
        self.mutate_db(|db| db.update_part_stock(part_id, expected_version, &input))
            .map(part_stock)
    }

    fn delete_part_stock(
        &self,
        context: &RequestContext,
        part_id: &str,
        expected_version: u64,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.delete_part_stock(part_id, expected_version))
    }
}

impl PrintHistoryEngine for LibraryRuntime {
    fn create_print_attempts(
        &self,
        context: &RequestContext,
        idempotency_key: &str,
        items: Vec<PrintAttemptBatchItem>,
    ) -> Result<Vec<PrintAttempt>, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let items = items
            .into_iter()
            .map(|item| CorePrintAttemptBatchItem {
                model_id: item.model_id,
                input: core_print_attempt_input(item.input),
            })
            .collect::<Vec<_>>();
        self.mutate_db(|db| db.create_print_attempts_idempotent(idempotency_key, &items))
            .map(|attempts| attempts.into_iter().map(print_attempt).collect())
    }

    fn list_print_attempts(
        &self,
        context: &RequestContext,
        model_id: &str,
    ) -> Result<Vec<PrintAttempt>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.list_print_attempts(model_id))
            .map(|attempts| attempts.into_iter().map(print_attempt).collect())
    }

    fn create_print_attempt(
        &self,
        context: &RequestContext,
        model_id: &str,
        input: PrintAttemptInput,
    ) -> Result<PrintAttempt, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = core_print_attempt_input(input);
        self.mutate_db(|db| db.create_print_attempt(model_id, &input))
            .map(print_attempt)
    }

    fn create_print_attempt_with_printer(
        &self,
        context: &RequestContext,
        model_id: &str,
        input: PrintAttemptInputWithPrinter,
    ) -> Result<PrintAttempt, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = core_print_attempt_input_with_printer(input);
        self.mutate_db(|db| db.create_print_attempt_with_printer(model_id, &input))
            .map(print_attempt)
    }

    fn update_print_attempt(
        &self,
        context: &RequestContext,
        attempt_id: &str,
        input: PrintAttemptInput,
    ) -> Result<PrintAttempt, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = core_print_attempt_input(input);
        self.mutate_db(|db| db.update_print_attempt(attempt_id, &input))
            .map(print_attempt)
    }

    fn update_print_attempt_with_printer(
        &self,
        context: &RequestContext,
        attempt_id: &str,
        input: PrintAttemptInputWithPrinter,
    ) -> Result<PrintAttempt, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = core_print_attempt_input_with_printer(input);
        self.mutate_db(|db| db.update_print_attempt_with_printer(attempt_id, &input))
            .map(print_attempt)
    }

    fn delete_print_attempt(
        &self,
        context: &RequestContext,
        attempt_id: &str,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.delete_print_attempt(attempt_id))
    }
}

impl RuntimeStatusEngine for LibraryRuntime {
    fn runtime_status(&self, context: &RequestContext) -> Result<RuntimeStatus, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        Ok(RuntimeStatus {
            library_epoch: self.library_epoch(),
            catalog_revision: self.catalog_revision(),
            maintenance: self.maintenance.load(Ordering::Acquire),
        })
    }

    fn events_since(
        &self,
        context: &RequestContext,
        after_revision: u64,
        page: PageRequest,
    ) -> Result<Page<RuntimeEvent>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        if page.cursor.is_some() || page.limit == 0 || page.limit > MAX_PAGE_LIMIT {
            return Err(invalid("Event page request is invalid"));
        }
        let revision = self.catalog_revision();
        let items = self
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.revision > after_revision)
            .take(usize::from(page.limit))
            .cloned()
            .collect();
        Ok(Page {
            items,
            next_cursor: None,
            snapshot_revision: revision,
        })
    }
}

impl ScanEngine for LibraryRuntime {
    fn start_scan(
        &self,
        context: &RequestContext,
        catalog_root_id: Option<String>,
    ) -> Result<ScanStatus, RuntimeError> {
        self.require(context, Capability::Scan)?;
        let runtime = self
            .self_weak
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| internal("Runtime self reference is unavailable"))?;
        if self.local_catalog_root_access {
            scanner::start_manual_scan(runtime, catalog_root_id).map_err(scan_error)?;
        } else {
            let catalog_roots = self.read_db(|db| db.list_index_roots())?;
            let admitted_ids = self
                .roots
                .iter()
                .filter_map(|policy| {
                    let catalog_id = policy.catalog_root_id.as_ref()?;
                    catalog_roots
                        .iter()
                        .find(|root| &root.id == catalog_id && root.path == policy.path)
                        .map(|_| catalog_id.clone())
                })
                .collect::<BTreeSet<_>>();
            let selected = if let Some(root_id) = catalog_root_id {
                if !admitted_ids.contains(&root_id) {
                    return Err(RuntimeError {
                        code: RuntimeErrorCode::Forbidden,
                        message: "Catalog root is not admitted for scanning".into(),
                        retryable: false,
                        current_version: None,
                        source_holds: Vec::new(),
                    });
                }
                vec![root_id]
            } else {
                admitted_ids.into_iter().collect::<Vec<_>>()
            };
            if selected.is_empty() {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Forbidden,
                    message: "No catalog roots are admitted for scanning".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            scanner::start_selected_scan(runtime, selected).map_err(scan_error)?;
        }
        Ok(scan_status(
            self.scan_control().status(),
            self.capabilities.contains(&Capability::HostPaths)
                && context.capabilities.contains(&Capability::HostPaths),
        ))
    }

    fn start_scan_subdirectory(
        &self,
        context: &RequestContext,
        catalog_root_id: String,
        directory: String,
    ) -> Result<ScanStatus, RuntimeError> {
        self.require(context, Capability::Scan)?;
        let runtime = self
            .self_weak
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| internal("Runtime self reference is unavailable"))?;
        if !self.local_catalog_root_access {
            let admitted = self.roots.iter().any(|root| {
                root.catalog_root_id.as_deref() == Some(catalog_root_id.as_str())
                    && root.handle.access != RootAccess::BackupOnly
            });
            if !admitted {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Forbidden,
                    message: "Catalog root is not admitted for scanning".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
        }
        scanner::start_manual_scan_subdirectory(runtime, catalog_root_id, directory)
            .map_err(scan_error)?;
        Ok(scan_status(
            self.scan_control().status(),
            self.capabilities.contains(&Capability::HostPaths)
                && context.capabilities.contains(&Capability::HostPaths),
        ))
    }

    fn cancel_scan(&self, context: &RequestContext) -> Result<ScanStatus, RuntimeError> {
        self.require(context, Capability::Scan)?;
        self.scan_control().cancel();
        Ok(scan_status(
            self.scan_control().status(),
            self.capabilities.contains(&Capability::HostPaths)
                && context.capabilities.contains(&Capability::HostPaths),
        ))
    }

    fn scan_status(&self, context: &RequestContext) -> Result<ScanStatus, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        Ok(scan_status(
            self.scan_control().status(),
            self.capabilities.contains(&Capability::HostPaths)
                && context.capabilities.contains(&Capability::HostPaths),
        ))
    }
}

impl BundleEngine for LibraryRuntime {
    fn prepare_bundle_preview(
        &self,
        context: &RequestContext,
        bundle_id: &str,
    ) -> Result<BundlePreviewSummary, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        self.bundle_previews.prepare(self, context, bundle_id)
    }

    fn describe_bundle_geometry(
        &self,
        context: &RequestContext,
        token: &str,
    ) -> Result<ContentLease, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        self.bundle_previews
            .describe(self, &context.principal_id, token)
    }

    fn stream_bundle_geometry(
        &self,
        context: &RequestContext,
        token: &str,
        sink: &mut dyn Write,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        self.bundle_previews
            .stream(self, &context.principal_id, token, sink)
    }

    fn close_bundle_preview(
        &self,
        context: &RequestContext,
        token: &str,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        self.bundle_previews.close(&context.principal_id, token)
    }

    fn issue_bundle_download(
        &self,
        context: &RequestContext,
        preview_token: &str,
    ) -> Result<ContentLease, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        let bundle_id = self.bundle_previews.bundle_id_for_download(
            self,
            &context.principal_id,
            preview_token,
        )?;
        {
            let mut leases = self.leases.lock().unwrap();
            self.prune_content_leases(&mut leases, SystemTime::now(), self.library_epoch());
            if let Some(existing) = leases.values().find(|lease| {
                lease.principal_id == context.principal_id
                    && lease.source_token.as_deref() == Some(preview_token)
                    && lease.descriptor.kind == ContentKind::BundleArchive
            }) {
                return Ok(existing.descriptor.clone());
            }
        }
        let estimate = bundles::bundle_archive_estimated_bytes(self, &bundle_id)?;
        let reservation = self.reserve_temp(ReservationKind::Bundle, estimate)?;
        let archive = bundles::create_bundle_archive(self, &bundle_id)?;
        let expires = SystemTime::now() + LEASE_TTL;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let descriptor = ContentLease {
            id: id.clone(),
            kind: ContentKind::BundleArchive,
            file_name: format!("bundle-{bundle_id}.zip"),
            mime: "application/zip".into(),
            bytes: archive.bytes,
            sha256: archive.sha256,
            library_epoch: self.library_epoch(),
            expires_at: format_system_time(expires)?,
        };
        if let Err(error) = reservation.commit(archive.bytes) {
            let _ = hold_runtime_path(&archive.path);
            return Err(error);
        }
        let mut leases = self.leases.lock().unwrap();
        self.prune_content_leases(&mut leases, SystemTime::now(), self.library_epoch());
        if leases.len() >= MAX_LEASES {
            let _ = hold_runtime_path(&archive.path);
            self.release_owned_bytes(archive.bytes);
            return Err(RuntimeError {
                code: RuntimeErrorCode::Busy,
                message: "Content lease limit reached".into(),
                retryable: true,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        leases.insert(
            id,
            ContentLeaseEntry {
                descriptor: descriptor.clone(),
                backing: ContentBacking::ExistingPath {
                    path: archive.path,
                    remove_on_drop: true,
                },
                expires,
                principal_id: context.principal_id.clone(),
                source_token: Some(preview_token.to_owned()),
            },
        );
        Ok(descriptor)
    }
}

impl GeometryEngine for LibraryRuntime {
    fn issue_model_geometry(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_hash: Option<&str>,
        plate_id: Option<u32>,
    ) -> Result<GeometryLease, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        let model = self
            .read_db(|db| db.get_model(model_id))?
            .ok_or_else(|| RuntimeError {
                code: RuntimeErrorCode::NotFound,
                message: "Model was not found".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })?;
        if expected_hash.is_some_and(|hash| model.content_hash.as_deref() != Some(hash)) {
            return Err(stale("Model source hash changed"));
        }
        // Expired tokens must release their map-owned backing before quota is
        // reserved. Existing readers retain their Arc and byte charge.
        {
            let mut leases = self.leases.lock().unwrap();
            self.prune_content_leases(&mut leases, SystemTime::now(), self.library_epoch());
        }
        let prepared = geometry::prepare_model_geometry(self, &model, plate_id)?;
        let expires = SystemTime::now() + LEASE_TTL;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let content = ContentLease {
            id: id.clone(),
            kind: ContentKind::Geometry,
            file_name: format!("model-{model_id}.stl2"),
            mime: "application/vnd.gogglelab.geometry".into(),
            bytes: prepared.packet.bytes(),
            sha256: prepared.packet.sha256().to_owned(),
            library_epoch: self.library_epoch(),
            expires_at: format_system_time(expires)?,
        };
        let mut leases = self.leases.lock().unwrap();
        self.prune_content_leases(&mut leases, SystemTime::now(), self.library_epoch());
        if leases.len() >= MAX_LEASES {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Busy,
                message: "Content lease limit reached".into(),
                retryable: true,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        leases.insert(
            id,
            ContentLeaseEntry {
                descriptor: content.clone(),
                backing: ContentBacking::Geometry(prepared.packet),
                expires,
                principal_id: context.principal_id.clone(),
                source_token: None,
            },
        );
        Ok(GeometryLease {
            content,
            packet_format: "stl2".into(),
            packet_version: 2,
            model_format: prepared.model_format,
            triangle_count: prepared.triangle_count,
            plates: prepared.plates,
            parts: prepared.parts,
            materials: prepared.materials,
            embedded_filaments: prepared.embedded_filaments,
            part_warning: prepared.part_warning,
            plate_warning: prepared.plate_warning,
        })
    }
}

impl DuplicateEngine for LibraryRuntime {
    fn duplicate_groups(
        &self,
        context: &RequestContext,
        model_ids: Vec<String>,
    ) -> Result<Vec<DuplicateGroup>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| {
            db.duplicate_groups(&model_ids)?
                .into_iter()
                .map(|group| {
                    let models = group
                        .models
                        .into_iter()
                        .map(|model| {
                            let thumbnail_id = db
                                .list_thumbnail_gallery(&model.id)?
                                .into_iter()
                                .find(|thumbnail| thumbnail.is_active)
                                .map(|thumbnail| thumbnail.id);
                            Ok(catalog_model(model, thumbnail_id))
                        })
                        .collect::<Result<Vec<_>, LibraryError>>()?;
                    Ok(DuplicateGroup {
                        content_hash: group.content_hash,
                        models,
                    })
                })
                .collect::<Result<Vec<_>, LibraryError>>()
        })
    }
}

impl EmbeddedMetadataEngine for LibraryRuntime {
    fn read_embedded_metadata(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_hash: Option<&str>,
    ) -> Result<EmbeddedMetadataProposal, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        let _io = self.begin_borrowed_io()?;
        let model = self
            .read_db(|db| db.get_model(model_id))?
            .ok_or_else(|| RuntimeError {
                code: RuntimeErrorCode::NotFound,
                message: "Model was not found".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })?;
        let source_hash = model
            .content_hash
            .clone()
            .ok_or_else(|| invalid("Model has no source content"))?;
        if expected_hash.is_some_and(|hash| hash != source_hash) {
            return Err(stale("Model source hash changed"));
        }
        let parsed = embedded::read_metadata(self, &model)?;
        let current_parent_model = match model.metadata.parent_model_id.as_deref() {
            Some(parent_id) => Some(
                self.read_db(|db| {
                    db.get_model(parent_id)
                        .map(|parent| parent.map(|parent| parent.display_name))
                })?
                .unwrap_or_else(|| parent_id.to_owned()),
            ),
            None => None,
        };
        Ok(EmbeddedMetadataProposal {
            model_id: model.id,
            source_hash,
            current: EmbeddedMetadataFields {
                designer: model.metadata.designer,
                source_url: model.metadata.source_url,
                notes: model.metadata.notes,
                model_license: model.metadata.model_license,
                parent_model: current_parent_model,
            },
            embedded: parsed.fields,
            embedded_filaments: parsed.filaments,
        })
    }

    fn apply_embedded_metadata(
        &self,
        context: &RequestContext,
        target: VersionedTarget,
        expected_source_hash: &str,
        selection: EmbeddedMetadataSelection,
    ) -> Result<MutationVersion, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        if !selection.designer
            && !selection.source_url
            && !selection.notes
            && !selection.model_license
            && !selection.parent_model
        {
            return Err(invalid("Embedded metadata selection is empty"));
        }
        let _io = self.begin_borrowed_io()?;
        let model = self
            .read_db(|db| db.get_model(&target.model_id))?
            .ok_or_else(|| RuntimeError {
                code: RuntimeErrorCode::NotFound,
                message: "Model was not found".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })?;
        if model.content_hash.as_deref() != Some(expected_source_hash) {
            return Err(stale("Model source hash changed"));
        }
        let embedded = embedded::read_metadata(self, &model)?.fields;
        let parent_model_id = if selection.parent_model {
            Some(match embedded.parent_model.as_deref() {
                Some(value) if !value.trim().is_empty() => {
                    self.read_db(|db| db.resolve_parent_model_id(value))?
                }
                _ => String::new(),
            })
        } else {
            None
        };
        let patch = CoreMetadataPatch {
            designer: selection
                .designer
                .then(|| embedded.designer.unwrap_or_default()),
            source_url: selection
                .source_url
                .then(|| embedded.source_url.unwrap_or_default()),
            notes: selection.notes.then(|| embedded.notes.unwrap_or_default()),
            model_license: selection
                .model_license
                .then(|| embedded.model_license.unwrap_or_default()),
            parent_model_id,
            ..CoreMetadataPatch::default()
        };
        self.mutate_db(|db| {
            db.update_model_metadata(&target.model_id, target.expected_version, &patch)
        })
        .map(mutation_version)
    }
}

impl GalleryEngine for LibraryRuntime {
    fn list_thumbnails(
        &self,
        context: &RequestContext,
        model_id: &str,
    ) -> Result<Vec<ThumbnailDescriptor>, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        let root = self.root.clone();
        self.read_db(|db| db.list_thumbnail_gallery(model_id))?
            .into_iter()
            .map(|item| {
                let path = root.join(&item.relative_path);
                let metadata = fs::symlink_metadata(&path).map_err(|_| io_error())?;
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.len() > 4 * 1024 * 1024
                {
                    return Err(RuntimeError {
                        code: RuntimeErrorCode::Corrupt,
                        message: "Thumbnail asset is missing or unsafe".into(),
                        retryable: false,
                        current_version: None,
                        source_holds: Vec::new(),
                    });
                }
                Ok(ThumbnailDescriptor {
                    thumbnail_id: item.id,
                    model_id: item.model_id,
                    ordinal: item.ordinal,
                    active: item.is_active,
                    custom: item.is_custom,
                    mime: "image/png".into(),
                    bytes: metadata.len(),
                    sha256: item.content_hash,
                })
            })
            .collect()
    }

    fn set_active_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<u64, RuntimeError> {
        self.require(context, Capability::MediaWrite)?;
        let model =
            self.mutate_db(|db| db.set_active_thumbnail(model_id, expected_version, thumbnail_id))?;
        Ok(model.metadata_version)
    }

    fn set_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        upload_token: &str,
    ) -> Result<u64, RuntimeError> {
        self.require(context, Capability::MediaWrite)?;
        let _io = self.begin_borrowed_io()?;
        let upload = self
            .uploads
            .lock()
            .unwrap()
            .get(upload_token)
            .cloned()
            .ok_or_else(|| stale("Thumbnail upload token is unknown or expired"))?;
        if upload.expires <= SystemTime::now()
            || upload.token.library_epoch != self.library_epoch()
            || upload.principal_id != context.principal_id
        {
            return Err(stale("Thumbnail upload token is stale"));
        }
        let (relative, hash) = images::import_custom_image_file(&self.root, &upload.path)
            .map_err(|_| invalid("Thumbnail upload is not a valid bounded PNG or JPEG"))?;
        let model = self.mutate_db(|db| {
            db.set_custom_thumbnail(model_id, expected_version, &hash, &relative)
        })?;
        let mut uploads = self.uploads.lock().unwrap();
        if uploads
            .get(upload_token)
            .is_some_and(|upload| upload.principal_id != context.principal_id)
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Upload token belongs to another principal".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        if let Some(upload) = uploads.remove(upload_token) {
            self.cleanup_upload(&upload);
        }
        Ok(model.metadata_version)
    }

    fn add_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        upload_token: &str,
    ) -> Result<u64, RuntimeError> {
        self.require(context, Capability::MediaWrite)?;
        let _io = self.begin_borrowed_io()?;
        let upload = self
            .uploads
            .lock()
            .unwrap()
            .get(upload_token)
            .cloned()
            .ok_or_else(|| stale("Thumbnail upload token is unknown or expired"))?;
        if upload.expires <= SystemTime::now()
            || upload.token.library_epoch != self.library_epoch()
            || upload.principal_id != context.principal_id
        {
            return Err(stale("Thumbnail upload token is stale"));
        }
        let (relative, hash) = images::import_custom_image_file(&self.root, &upload.path)
            .map_err(|_| invalid("Thumbnail upload is not a valid bounded PNG or JPEG"))?;
        let (model, _) = self.mutate_db(|db| {
            db.add_custom_thumbnail(model_id, expected_version, &hash, &relative)
        })?;
        let mut uploads = self.uploads.lock().unwrap();
        if uploads
            .get(upload_token)
            .is_some_and(|upload| upload.principal_id != context.principal_id)
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Upload token belongs to another principal".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        if let Some(upload) = uploads.remove(upload_token) {
            self.cleanup_upload(&upload);
        }
        Ok(model.metadata_version)
    }

    fn delete_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<u64, RuntimeError> {
        self.require(context, Capability::MediaWrite)?;
        let _io = self.begin_borrowed_io()?;
        let (model, path) = self.mutate_db(|db| {
            db.delete_thumbnail_gallery_item(model_id, expected_version, thumbnail_id)
        })?;
        match hold_runtime_path(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(io_error()),
        }
        Ok(model.metadata_version)
    }

    fn issue_thumbnail_media(
        &self,
        context: &RequestContext,
        model_id: &str,
        thumbnail_id: &str,
    ) -> Result<MediaLease, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        let item = self
            .read_db(|db| db.list_thumbnail_gallery(model_id))?
            .into_iter()
            .find(|item| item.id == thumbnail_id)
            .ok_or_else(|| RuntimeError {
                code: RuntimeErrorCode::NotFound,
                message: "Thumbnail was not found".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })?;
        let path = self.root.join(&item.relative_path);
        let metadata = fs::symlink_metadata(&path).map_err(|_| io_error())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() > 4 * 1024 * 1024
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Thumbnail asset is missing or unsafe".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        verify_content_file(&path, metadata.len(), &item.content_hash)?;
        let expires = SystemTime::now() + LEASE_TTL;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let descriptor = MediaLease {
            id: id.clone(),
            kind: ContentKind::Thumbnail,
            file_name: format!("thumbnail-{thumbnail_id}.png"),
            mime: "image/png".into(),
            bytes: metadata.len(),
            sha256: item.content_hash,
            library_epoch: self.library_epoch(),
            expires_at: format_system_time(expires)?,
        };
        let mut leases = self.leases.lock().unwrap();
        let epoch = self.library_epoch();
        let now = SystemTime::now();
        self.prune_content_leases(&mut leases, now, epoch);
        if leases.len() >= MAX_LEASES {
            if let Some(oldest) = leases.keys().next().cloned() {
                if let Some(removed) = leases.remove(&oldest) {
                    self.cleanup_content_lease(&removed);
                }
            }
        }
        leases.insert(
            id,
            ContentLeaseEntry {
                descriptor: descriptor.clone(),
                backing: ContentBacking::ExistingPath {
                    path,
                    remove_on_drop: false,
                },
                expires,
                principal_id: context.principal_id.clone(),
                source_token: None,
            },
        );
        Ok(descriptor)
    }
}

impl UploadEngine for LibraryRuntime {
    fn stage_upload(
        &self,
        context: &RequestContext,
        intent: UploadIntent,
        source: &mut dyn Read,
    ) -> Result<UploadToken, RuntimeError> {
        self.require_upload_write(context)?;
        validate_upload_intent(&intent)?;
        let _io = self.begin_borrowed_io()?;
        {
            let mut uploads = self.uploads.lock().unwrap();
            self.prune_uploads(&mut uploads, SystemTime::now(), self.library_epoch());
            if uploads.len() >= MAX_LEASES {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Busy,
                    message: "Upload staging limit reached".into(),
                    retryable: true,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
        }
        let reservation = self.reserve_temp(ReservationKind::Upload, intent.bytes)?;
        self.read_db(|_| Ok(()))?;
        let staging = self.root.join("staging");
        let metadata = fs::symlink_metadata(&staging).map_err(|_| io_error())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || staging.canonicalize().map_err(|_| io_error())? != staging
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Runtime staging directory is unsafe".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let path = staging.join(format!("upload-{id}.part"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| io_error())?;
        let mut digest = Sha256::new();
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        let result = (|| {
            loop {
                let count = source.read(&mut buffer).map_err(|_| io_error())?;
                if count == 0 {
                    break;
                }
                copied = copied
                    .checked_add(count as u64)
                    .ok_or_else(|| invalid("Upload size overflow"))?;
                if copied > intent.bytes {
                    return Err(invalid("Upload exceeds its declared size"));
                }
                digest.update(&buffer[..count]);
                file.write_all(&buffer[..count]).map_err(|_| io_error())?;
            }
            if copied != intent.bytes || format!("{:x}", digest.finalize()) != intent.sha256 {
                return Err(stale("Upload bytes do not match their declared digest"));
            }
            file.sync_all().map_err(|_| io_error())?;
            Ok(())
        })();
        if let Err(error) = result {
            drop(file);
            let _ = hold_runtime_path(&path);
            return Err(error);
        }
        let expires = SystemTime::now() + LEASE_TTL;
        let token = UploadToken {
            id: id.clone(),
            item_id: intent.item_id,
            capture_id: intent.capture_id,
            expires_at: format_system_time(expires)?,
            library_epoch: self.library_epoch(),
        };
        let mut uploads = self.uploads.lock().unwrap();
        let now = SystemTime::now();
        let epoch = self.library_epoch();
        self.prune_uploads(&mut uploads, now, epoch);
        if uploads.len() >= MAX_LEASES {
            let _ = hold_runtime_path(&path);
            return Err(RuntimeError {
                code: RuntimeErrorCode::Busy,
                message: "Upload staging limit reached".into(),
                retryable: true,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        if let Err(error) = reservation.commit(intent.bytes) {
            let _ = hold_runtime_path(&path);
            return Err(error);
        }
        uploads.insert(
            id,
            UploadEntry {
                token: token.clone(),
                path,
                expires,
                sha256: intent.sha256,
                bytes: intent.bytes,
                principal_id: context.principal_id.clone(),
            },
        );
        Ok(token)
    }

    fn discard_upload(
        &self,
        context: &RequestContext,
        upload_token: &str,
    ) -> Result<(), RuntimeError> {
        self.require_upload_write(context)?;
        let mut uploads = self.uploads.lock().unwrap();
        if uploads
            .get(upload_token)
            .is_some_and(|upload| upload.principal_id != context.principal_id)
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Upload token belongs to another principal".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        if let Some(upload) = uploads.remove(upload_token) {
            self.cleanup_upload(&upload);
        }
        Ok(())
    }
}

impl DownloadEngine for LibraryRuntime {
    fn issue_model_download(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_hash: Option<&str>,
    ) -> Result<SourceLease, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        let (model, source) = self.read_db(|db| {
            let model = db
                .get_model(model_id)?
                .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))?;
            let source = db.resolve_model_source_ref(model_id)?;
            Ok((model, source))
        })?;
        self.validate_indexed_source_admission(&model, &source)?;
        if expected_hash.is_some_and(|hash| hash != source.content_hash) {
            return Err(stale("Model source hash changed"));
        }
        let path = if let Some(member) = source.archive_member.as_deref() {
            archives::extract_member_to_cache_typed(
                &self.root,
                &source.outer_path,
                member,
                source
                    .outer_fingerprint
                    .as_deref()
                    .ok_or_else(|| RuntimeError {
                        code: RuntimeErrorCode::Corrupt,
                        message: "Archive model has no outer fingerprint".into(),
                        retryable: false,
                        current_version: None,
                        source_holds: Vec::new(),
                    })?,
                &source.content_hash,
                source.bytes,
            )
            .map_err(|error| {
                error.into_runtime(RuntimeError {
                    code: RuntimeErrorCode::Corrupt,
                    message: "Archive member failed source verification".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                })
            })?
        } else {
            source.outer_path
        };
        verify_content_file(&path, source.bytes, &source.content_hash)?;
        let file_name = safe_download_name(
            &model.display_name,
            source
                .archive_member
                .as_deref()
                .map(Path::new)
                .unwrap_or(path.as_path()),
        );
        let expires = SystemTime::now() + LEASE_TTL;
        let expires_at = time::OffsetDateTime::from(expires)
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| internal("Lease expiry could not be formatted"))?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let descriptor = SourceLease {
            id: id.clone(),
            kind: ContentKind::Model,
            file_name,
            mime: if model
                .format
                .as_deref()
                .is_some_and(|value| value.contains("3mf"))
            {
                "model/3mf".into()
            } else {
                "model/stl".into()
            },
            bytes: source.bytes,
            sha256: source.content_hash,
            library_epoch: self.library_epoch(),
            expires_at,
        };
        let mut leases = self.leases.lock().unwrap();
        let epoch = self.library_epoch();
        let now = SystemTime::now();
        self.prune_content_leases(&mut leases, now, epoch);
        if leases.len() >= MAX_LEASES {
            if let Some(oldest) = leases.keys().next().cloned() {
                if let Some(removed) = leases.remove(&oldest) {
                    self.cleanup_content_lease(&removed);
                }
            }
        }
        leases.insert(
            id,
            ContentLeaseEntry {
                descriptor: descriptor.clone(),
                backing: ContentBacking::ExistingPath {
                    path,
                    remove_on_drop: false,
                },
                expires,
                principal_id: context.principal_id.clone(),
                source_token: None,
            },
        );
        Ok(descriptor)
    }
}

impl ContentEngine for LibraryRuntime {
    fn describe_content(
        &self,
        context: &RequestContext,
        lease_id: &str,
    ) -> Result<ContentLease, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        let entry = self.admitted_content_entry(context, lease_id)?;
        entry.backing.verify(&entry.descriptor)?;
        Ok(entry.descriptor)
    }

    fn stream_content(
        &self,
        context: &RequestContext,
        lease_id: &str,
        sink: &mut dyn Write,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        let entry = self.admitted_content_entry(context, lease_id)?;
        entry.backing.stream_to(&entry.descriptor, sink)?;
        Ok(())
    }
}

impl LibraryRuntime {
    fn admitted_content_entry(
        &self,
        context: &RequestContext,
        lease_id: &str,
    ) -> Result<ContentLeaseEntry, RuntimeError> {
        let entry = self
            .leases
            .lock()
            .unwrap()
            .get(lease_id)
            .cloned()
            .ok_or_else(|| stale("Content lease is unknown or expired"))?;
        if entry.principal_id != context.principal_id {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Content lease belongs to another principal".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        if entry.expires <= SystemTime::now()
            || entry.descriptor.library_epoch != self.library_epoch()
        {
            if let Some(removed) = self.leases.lock().unwrap().remove(lease_id) {
                self.cleanup_content_lease(&removed);
            }
            return Err(stale("Content lease is stale"));
        }
        Ok(entry)
    }
}

impl CaptureEngine for LibraryRuntime {
    fn list_capture_items(
        &self,
        context: &RequestContext,
        page: PageRequest,
    ) -> Result<Page<CaptureSummary>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        if page.limit == 0 || page.limit > MAX_PAGE_LIMIT {
            return Err(invalid("Page limit is outside the supported range"));
        }
        let revision = self.catalog_revision();
        let cursor = if let Some(token) = page.cursor.as_deref() {
            let cursor = self
                .capture_cursors
                .lock()
                .unwrap()
                .get(token)
                .cloned()
                .ok_or_else(|| stale("Capture cursor is unknown or expired"))?;
            if cursor.revision != revision {
                return Err(stale("Capture cursor became stale"));
            }
            Some(cursor)
        } else {
            None
        };
        let (summaries, has_more) = self.read_db(|db| {
            db.list_capture_summaries_page(
                cursor.as_ref().map(|cursor| cursor.created_at.as_str()),
                cursor.as_ref().map(|cursor| cursor.item_id.as_str()),
                page.limit,
            )
        })?;
        if self.catalog_revision() != revision {
            return Err(stale("Catalog changed while capture inbox was being read"));
        }
        let next_cursor = if has_more {
            let last = summaries
                .last()
                .ok_or_else(|| internal("Capture page lost its final item"))?;
            let token = uuid::Uuid::new_v4().simple().to_string();
            let mut cursors = self.capture_cursors.lock().unwrap();
            cursors.retain(|_, cursor| cursor.revision == revision);
            if cursors.len() >= MAX_CURSORS {
                if let Some(oldest) = cursors.keys().next().cloned() {
                    cursors.remove(&oldest);
                }
            }
            cursors.insert(
                token.clone(),
                CaptureCursor {
                    created_at: last.created_at.clone(),
                    item_id: last.item_id.clone(),
                    revision,
                },
            );
            Some(token)
        } else {
            None
        };
        Ok(Page {
            items: summaries.into_iter().map(capture_summary).collect(),
            next_cursor,
            snapshot_revision: revision,
        })
    }

    fn import_capture(
        &self,
        context: &RequestContext,
        envelope: CaptureEnvelope,
        uploads: Vec<UploadToken>,
    ) -> Result<CaptureImportResult, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        if envelope.version != CAPTURE_ENVELOPE_VERSION {
            return Err(invalid("Capture envelope version is unsupported"));
        }
        if envelope
            .downloads
            .iter()
            .any(|download| download.relative_inbox_path.is_some())
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Browser capture cannot supply server filesystem paths".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        let canonical = serde_json::to_vec(&envelope)
            .map_err(|_| invalid("Capture envelope could not be canonicalized"))?;
        let payload_hash = format!("{:x}", Sha256::digest(&canonical));
        let existing = self.read_db(|db| db.get_capture_receipt(&envelope.item_id))?;
        let upload_entries = {
            let stored = self.uploads.lock().unwrap();
            let mut entries = HashMap::new();
            for token in &uploads {
                let entry = stored
                    .get(&token.id)
                    .filter(|entry| entry.token == *token)
                    .cloned()
                    .ok_or_else(|| stale("Capture upload token is unknown or expired"))?;
                if entry.expires <= SystemTime::now()
                    || entry.token.library_epoch != self.library_epoch()
                    || entry.principal_id != context.principal_id
                    || entry.token.item_id != envelope.item_id
                    || !envelope
                        .downloads
                        .iter()
                        .any(|download| download.capture_id == entry.token.capture_id)
                    || entries
                        .insert(entry.token.capture_id.clone(), entry)
                        .is_some()
                {
                    return Err(stale("Capture upload token does not match its envelope"));
                }
            }
            entries
        };
        let page = envelope.page.clone();
        let downloads = envelope.downloads.clone();
        let input = CoreCaptureReceiptInput {
            item_id: envelope.item_id.clone(),
            payload_hash,
            created_at: envelope.created_at.clone(),
            page: CoreCapturePage {
                url: page.url.clone(),
                title: page.title.clone(),
                designer: page.designer.clone(),
                model_license: page.license.clone(),
                notes: page.notes.clone(),
                parent_external_id: page.parent_external_id.clone(),
            },
            downloads: downloads
                .iter()
                .map(|download| CoreCaptureDownload {
                    capture_id: download.capture_id.clone(),
                    file_name: download.file_name.clone(),
                    sha256: download.sha256.clone(),
                    bytes: download.bytes,
                })
                .collect(),
        };
        let mut receipt = self.mutate_db(|db| db.create_or_get_capture_receipt(&input))?;
        if existing.is_some_and(|receipt| receipt.status == "imported") {
            return Ok(capture_result(receipt, true));
        }
        let bookmark_id = receipt
            .page_model_id
            .clone()
            .ok_or_else(|| internal("Capture bookmark is missing"))?;
        for download in downloads {
            if receipt.downloads.iter().any(|result| {
                result.capture_id == download.capture_id
                    && matches!(result.status.as_str(), "imported" | "imported_removed")
            }) {
                continue;
            }
            let Some(upload) = upload_entries.get(&download.capture_id) else {
                continue;
            };
            if download
                .sha256
                .as_deref()
                .is_some_and(|hash| hash != upload.sha256)
                || download.bytes.is_some_and(|bytes| bytes != upload.bytes)
            {
                return Err(stale("Capture upload metadata changed"));
            }
            let imported = self.mutate_db(|db| {
                let model = db
                    .import_file(&upload.path, None, Some(&download.file_name))?
                    .model;
                db.update_model_metadata(
                    &model.id,
                    model.metadata_version,
                    &CoreMetadataPatch {
                        designer: page.designer.clone(),
                        source_url: Some(page.url.clone()),
                        notes: page.notes.clone(),
                        model_license: page.license.clone(),
                        parent_model_id: Some(bookmark_id.clone()),
                        ..CoreMetadataPatch::default()
                    },
                )
            });
            match imported {
                Ok(model) => {
                    receipt = self.mutate_db(|db| {
                        db.update_capture_download_result(
                            &envelope.item_id,
                            &download.capture_id,
                            Some(&model.id),
                            None,
                        )
                    })?;
                    let removed = self.uploads.lock().unwrap().remove(&upload.token.id);
                    if let Some(upload) = removed {
                        self.cleanup_upload(&upload);
                    }
                }
                Err(_) => {
                    receipt = self.mutate_db(|db| {
                        db.update_capture_download_result(
                            &envelope.item_id,
                            &download.capture_id,
                            None,
                            Some("import_failed"),
                        )
                    })?;
                }
            }
        }
        Ok(capture_result(receipt, false))
    }

    fn get_capture_result(
        &self,
        context: &RequestContext,
        item_id: &str,
    ) -> Result<CaptureImportResult, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        let receipt = self
            .read_db(|db| db.get_capture_receipt(item_id))?
            .ok_or_else(|| RuntimeError {
                code: RuntimeErrorCode::NotFound,
                message: "Capture receipt was not found".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })?;
        Ok(capture_result(receipt, false))
    }
}

pub struct RuntimeIoGuard {
    runtime: Arc<LibraryRuntime>,
}

struct BorrowedIoGuard<'a> {
    runtime: &'a LibraryRuntime,
}

impl Drop for BorrowedIoGuard<'_> {
    fn drop(&mut self) {
        let mut lifecycle = self.runtime.lifecycle.lock().unwrap();
        lifecycle.active_io = lifecycle.active_io.saturating_sub(1);
    }
}

impl Drop for RuntimeIoGuard {
    fn drop(&mut self) {
        let mut lifecycle = self.runtime.lifecycle.lock().unwrap();
        lifecycle.active_io = lifecycle.active_io.saturating_sub(1);
    }
}

impl Drop for LibraryRuntime {
    fn drop(&mut self) {
        if let Ok(leases) = self.leases.get_mut() {
            for lease in leases.drain().map(|(_, lease)| lease) {
                if let ContentBacking::ExistingPath {
                    path,
                    remove_on_drop: true,
                } = &lease.backing
                {
                    let _ = hold_runtime_path(path);
                }
            }
        }
        if let Ok(uploads) = self.uploads.get_mut() {
            for upload in uploads.drain().map(|(_, upload)| upload) {
                let _ = hold_runtime_path(&upload.path);
            }
        }
    }
}

pub struct RuntimeMaintenanceGuard {
    runtime: Arc<LibraryRuntime>,
    catalog: Option<LibraryDb>,
    release_on_drop: Cell<bool>,
}

impl RuntimeMaintenanceGuard {
    pub fn root(&self) -> &Path {
        &self.runtime.root
    }

    #[doc(hidden)]
    pub fn catalog(&self) -> Result<&LibraryDb, RuntimeError> {
        let catalog = self
            .catalog
            .as_ref()
            .ok_or_else(|| internal("Maintenance guard has no pinned catalog connection"))?;
        if !catalog.catalog_path_matches() {
            return Err(stale("Library catalog file identity changed"));
        }
        Ok(catalog)
    }

    pub fn invalidate(&self) -> u64 {
        self.runtime.advance_epoch()
    }

    pub fn remain_armed(&self) {
        self.release_on_drop.set(false);
    }
}

impl Drop for RuntimeMaintenanceGuard {
    fn drop(&mut self) {
        if self.release_on_drop.get() {
            if let Some(catalog) = self.catalog.take() {
                *self.runtime.db.lock().unwrap() = Some(catalog);
            }
            let mut lifecycle = self.runtime.lifecycle.lock().unwrap();
            lifecycle.maintenance = false;
            self.runtime.maintenance.store(false, Ordering::Release);
        }
    }
}

fn admit_roots(policies: Vec<RuntimeRootPolicy>) -> Result<Vec<AdmittedRoot>, RuntimeError> {
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut roots = Vec::with_capacity(policies.len());
    for policy in policies {
        validate_plain_id(&policy.id, "Root handle")?;
        if let Some(catalog_root_id) = &policy.catalog_root_id {
            validate_plain_id(catalog_root_id, "Catalog root")?;
        }
        if policy.label.trim().is_empty() || policy.label.len() > 200 {
            return Err(invalid("Root label is invalid"));
        }
        if !ids.insert(policy.id.clone()) {
            return Err(invalid("Duplicate root handle"));
        }
        let metadata = fs::symlink_metadata(&policy.path).map_err(|_| io_error())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(invalid("Admitted root must be a real directory"));
        }
        let path = policy.path.canonicalize().map_err(|_| io_error())?;
        if !paths.insert(path.clone()) {
            return Err(invalid("Duplicate admitted root path"));
        }
        if (policy.access == RootAccess::BackupOnly
            || roots
                .iter()
                .any(|root: &AdmittedRoot| root.handle.access == RootAccess::BackupOnly))
            && roots.iter().any(|root| paths_overlap(&root.path, &path))
        {
            return Err(invalid(
                "Backup-only roots must not overlap other admitted roots",
            ));
        }
        roots.push(AdmittedRoot {
            handle: RootHandle {
                id: policy.id,
                catalog_root_id: policy.catalog_root_id.clone(),
                label: policy.label.trim().into(),
                access: policy.access,
            },
            catalog_root_id: policy.catalog_root_id,
            path,
        });
    }
    Ok(roots)
}

fn validate_backup_root_separation(
    roots: &[AdmittedRoot],
    library_root: &Path,
) -> Result<(), RuntimeError> {
    let library_root = canonical_path_allow_missing_tail(library_root)?;
    if roots.iter().any(|root| {
        root.handle.access == RootAccess::BackupOnly && paths_overlap(&root.path, &library_root)
    }) {
        return Err(invalid(
            "Backup-only roots must not overlap the library root",
        ));
    }
    Ok(())
}

fn canonical_path_allow_missing_tail(path: &Path) -> Result<PathBuf, RuntimeError> {
    if !path.is_absolute() {
        return Err(invalid(
            "Library root must be absolute when using a backup-only root",
        ));
    }

    let mut canonical = PathBuf::new();
    let mut missing_tail = false;
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => canonical.push(prefix.as_os_str()),
            std::path::Component::RootDir => canonical.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                canonical.pop();
                if missing_tail {
                    match fs::symlink_metadata(&canonical) {
                        Ok(_) => {
                            canonical = canonical.canonicalize().map_err(|_| io_error())?;
                            missing_tail = false;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => return Err(io_error()),
                    }
                }
            }
            std::path::Component::Normal(name) => {
                canonical.push(name);
                if !missing_tail {
                    match fs::symlink_metadata(&canonical) {
                        Ok(_) => canonical = canonical.canonicalize().map_err(|_| io_error())?,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            missing_tail = true;
                        }
                        Err(_) => return Err(io_error()),
                    }
                }
            }
        }
    }
    Ok(canonical)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn map_operator_catalog_roots(
    db: &LibraryDb,
    roots: &mut [AdmittedRoot],
    allow_create: bool,
) -> Result<(), RuntimeError> {
    if roots
        .iter()
        .any(|root| root.handle.access == RootAccess::BackupOnly && root.catalog_root_id.is_some())
    {
        return Err(invalid("Backup-only roots cannot map to catalog roots"));
    }
    let mut catalog_roots = db.list_index_roots().map_err(runtime_library_error)?;
    for root in roots.iter_mut().filter(|root| {
        !matches!(
            root.handle.access,
            RootAccess::Inbox | RootAccess::BackupOnly
        )
    }) {
        let mapped = if let Some(catalog_root_id) = root.catalog_root_id.as_deref() {
            catalog_roots
                .iter()
                .find(|catalog| catalog.id == catalog_root_id && catalog.path == root.path)
                .cloned()
                .ok_or_else(|| RuntimeError {
                    code: RuntimeErrorCode::Forbidden,
                    message: "Configured catalog root ID does not match its admitted path".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                })?
        } else if let Some(existing) = catalog_roots
            .iter()
            .find(|catalog| catalog.path == root.path)
            .cloned()
        {
            existing
        } else if allow_create {
            let created = db
                .add_index_root(&root.path, 60, 50 * 1024 * 1024)
                .map_err(runtime_library_error)?;
            catalog_roots.push(created.clone());
            created
        } else {
            continue;
        };
        root.catalog_root_id = Some(mapped.id.clone());
        root.handle.catalog_root_id = Some(mapped.id);
    }
    Ok(())
}

fn validate_management_action(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    action: &ManagementAction,
) -> Result<(), RuntimeError> {
    match action {
        ManagementAction::RecoverableRemove { targets } => {
            if targets.is_empty() {
                return Err(invalid("Removal requires at least one model"));
            }
        }
        ManagementAction::RestoreRemoval { snapshot_id } => {
            validate_plain_id_text(snapshot_id, "Removal snapshot")?;
        }
        ManagementAction::CreateBackup {
            destination_handle_id,
            file_name,
        }
        | ManagementAction::ExportSelection {
            destination_handle_id,
            file_name,
            ..
        } => {
            validate_output_name(file_name)?;
            runtime.admitted_root_path(context, destination_handle_id, RootAccess::BackupOnly)?;
        }
        ManagementAction::ExportLibraryJson {
            destination_handle_id,
            file_name,
        } => {
            validate_output_name(file_name)?;
            admitted_file(
                runtime,
                context,
                destination_handle_id,
                RootAccess::BackupOnly,
                file_name,
                false,
            )?;
        }
        ManagementAction::RestoreBackup {
            source_handle_id,
            file_name,
            root_mappings,
        } => {
            runtime.require(context, Capability::CatalogRead)?;
            validate_output_name(file_name)?;
            if runtime
                .admitted_root_path(context, source_handle_id, RootAccess::Inbox)
                .or_else(|_| {
                    runtime.admitted_root_path(context, source_handle_id, RootAccess::ReadOnly)
                })
                .is_err()
            {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Forbidden,
                    message: "Restore source handle is not admitted".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            let mut backup_ids = BTreeSet::new();
            let mut handles = BTreeSet::new();
            for mapping in root_mappings {
                validate_plain_id_text(&mapping.backup_catalog_root_id, "Backup catalog root")?;
                if !backup_ids.insert(&mapping.backup_catalog_root_id)
                    || !handles.insert(&mapping.operator_handle_id)
                {
                    return Err(invalid("Restore root mapping is duplicated"));
                }
                runtime.admitted_root_path(
                    context,
                    &mapping.operator_handle_id,
                    RootAccess::ReadOnly,
                )?;
            }
        }
        ManagementAction::QuarantineLooseSource { target } => {
            validate_physical_model_action(runtime, context, &target.model_id)?;
        }
        ManagementAction::QuarantineLooseSources { targets } => {
            validate_physical_targets(targets.iter().map(|target| target.model_id.as_str()))?;
            for target in targets {
                validate_physical_model_action(runtime, context, &target.model_id)?;
            }
        }
        ManagementAction::UndoQuarantine { snapshot_id } => {
            validate_plain_id_text(snapshot_id, "Quarantine snapshot")?;
        }
        ManagementAction::MoveLooseSource {
            target,
            destination_handle_id,
            file_name,
        } => {
            validate_output_name(file_name)?;
            validate_physical_model_action(runtime, context, &target.model_id)?;
            let destination_root = runtime
                .roots
                .iter()
                .find(|root| root.handle.id == *destination_handle_id)
                .ok_or_else(|| invalid("Move destination root is not admitted"))?;
            if destination_root.handle.access != RootAccess::Writable
                || destination_root.catalog_root_id.is_none()
            {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Forbidden,
                    message: "Move destination must be an admitted writable catalog root".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            admitted_file(
                runtime,
                context,
                destination_handle_id,
                RootAccess::Writable,
                file_name,
                false,
            )?;
        }
        ManagementAction::MoveLooseSources {
            targets,
            destination_handle_id,
        } => {
            validate_physical_targets(targets.iter().map(|target| target.model_id.as_str()))?;
            let destination_root = runtime
                .roots
                .iter()
                .find(|root| root.handle.id == *destination_handle_id)
                .ok_or_else(|| invalid("Move destination root is not admitted"))?;
            if destination_root.handle.access != RootAccess::Writable
                || destination_root.catalog_root_id.is_none()
            {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Forbidden,
                    message: "Move destination must be an admitted writable catalog root".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            let mut destination_names = BTreeSet::new();
            for target in targets {
                validate_output_name(&target.file_name)?;
                if !destination_names.insert(target.file_name.to_lowercase()) {
                    return Err(invalid("Move destination file names must be unique"));
                }
                validate_physical_model_action(runtime, context, &target.model_id)?;
                admitted_file(
                    runtime,
                    context,
                    destination_handle_id,
                    RootAccess::Writable,
                    &target.file_name,
                    false,
                )?;
            }
        }
        ManagementAction::PurgeCatalog {
            backup_destination_handle_id,
            backup_file_name,
        } => {
            validate_output_name(backup_file_name)?;
            admitted_file(
                runtime,
                context,
                backup_destination_handle_id,
                RootAccess::BackupOnly,
                backup_file_name,
                false,
            )?;
        }
        ManagementAction::CleanupOrphans { .. } | ManagementAction::ResetDefaultThumbnails => {}
    }
    Ok(())
}

fn snapshot_managed_library_assets(
    runtime: &LibraryRuntime,
) -> Result<Vec<ManagedAssetSnapshot>, RuntimeError> {
    let root = runtime.root.canonicalize().map_err(|_| io_error())?;
    let models_and_sources = runtime.read_db(|db| {
        db.list_models(None, None)?
            .into_iter()
            .filter(|model| model.source_kind == "managed")
            .map(|model| {
                let source = db.resolve_model_source_ref(&model.id)?;
                Ok((model, source))
            })
            .collect::<Result<Vec<_>, LibraryError>>()
    })?;
    if models_and_sources.len() > 100_000 {
        return Err(invalid("Managed asset count exceeds the purge limit"));
    }
    let mut total = 0u64;
    let mut snapshots = Vec::with_capacity(models_and_sources.len());
    for (model, source) in models_and_sources {
        if source.archive_member.is_some() {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Managed model unexpectedly references an archive member".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        let relative = source
            .outer_path
            .strip_prefix(&root)
            .map_err(|_| RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Managed model asset is outside the Library".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })?
            .to_path_buf();
        validate_relative_source_path(&relative)?;
        if relative.components().next() != Some(std::path::Component::Normal("originals".as_ref()))
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Managed model asset is outside the owned originals namespace".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        let path = root.join(&relative);
        let metadata = fs::symlink_metadata(&path).map_err(|_| io_error())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || path.canonicalize().map_err(|_| io_error())? != path
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Managed model asset is not a regular owned file".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        let (bytes, sha256) = hash_management_file(&path, library_core::MAX_INDEXED_FILE_BYTES)?;
        if bytes != model.bytes
            || bytes != source.bytes
            || model.content_hash.as_deref() != Some(sha256.as_str())
            || source.content_hash != sha256
        {
            return Err(stale("Managed model bytes do not match the catalog"));
        }
        total = total
            .checked_add(bytes)
            .ok_or_else(|| invalid("Managed asset byte total is out of range"))?;
        if total > MAX_ORPHAN_CLEANUP_TOTAL_BYTES {
            return Err(invalid("Managed assets exceed the backup size limit"));
        }
        snapshots.push(ManagedAssetSnapshot {
            relative_path: relative,
            bytes,
            sha256,
        });
    }
    snapshots.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(snapshots)
}

fn verify_managed_asset_snapshots(
    root: &Path,
    snapshots: &[ManagedAssetSnapshot],
) -> Result<(), RuntimeError> {
    let root = root.canonicalize().map_err(|_| io_error())?;
    for snapshot in snapshots {
        validate_relative_source_path(&snapshot.relative_path)?;
        if snapshot.relative_path.components().next()
            != Some(std::path::Component::Normal("originals".as_ref()))
        {
            return Err(invalid("Managed asset snapshot is outside originals"));
        }
        let path = root.join(&snapshot.relative_path);
        let metadata = fs::symlink_metadata(&path).map_err(|_| stale("Managed asset changed"))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || path.canonicalize().map_err(|_| io_error())? != path
        {
            return Err(stale("Managed asset path changed"));
        }
        let actual = hash_management_file(&path, library_core::MAX_INDEXED_FILE_BYTES)?;
        if actual != (snapshot.bytes, snapshot.sha256.clone()) {
            return Err(stale("Managed asset bytes changed after confirmation"));
        }
    }
    Ok(())
}

fn purge_journal_path(root: &Path) -> Result<PathBuf, RuntimeError> {
    let canonical = canonical_path_allow_missing_tail(root)?;
    let parent = canonical
        .parent()
        .ok_or_else(|| invalid("Library root has no parent directory"))?;
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    let suffix = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(parent.join(format!(".gogglelab-purge-{suffix}.journal")))
}

#[cfg(test)]
fn purge_journal_temp_path(journal_path: &Path) -> PathBuf {
    let file_name = journal_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    journal_path.with_file_name(format!("{file_name}.tmp"))
}

#[cfg(test)]
fn remove_purge_journal_temp(journal_path: &Path) -> Result<bool, RuntimeError> {
    let temporary = purge_journal_temp_path(journal_path);
    let metadata = match fs::symlink_metadata(&temporary) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(io_error()),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > 8 * 1024
        || temporary.canonicalize().map_err(|_| io_error())? != temporary
    {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: "Purge journal temporary file is unsafe".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }
    fs::remove_file(temporary).map_err(|_| io_error())?;
    Ok(true)
}

#[cfg(test)]
fn purge_stage_marker_path(root: &Path) -> PathBuf {
    root.join(".gogglelab-purge-stage")
}

#[cfg(test)]
fn write_purge_stage_marker(root: &Path, nonce: &str) -> Result<(), RuntimeError> {
    let marker = purge_stage_marker_path(root);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&marker).map_err(|_| io_error())?;
    file.write_all(nonce.as_bytes()).map_err(|_| io_error())?;
    file.sync_all().map_err(|_| io_error())?;
    sync_directory(root)
}

#[cfg(test)]
fn purge_stage_marker_matches(root: &Path, nonce: &str) -> Result<bool, RuntimeError> {
    let marker = purge_stage_marker_path(root);
    let metadata = match fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(io_error()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 64 {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: "Purge staging marker is unsafe".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }
    let bytes = fs::read(&marker).map_err(|_| io_error())?;
    Ok(bytes == nonce.as_bytes())
}

#[cfg(unix)]
fn is_purge_marker_symlink_loop(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

#[cfg(not(unix))]
fn is_purge_marker_symlink_loop(_error: &std::io::Error) -> bool {
    false
}

fn purge_stage_marker_matches_at(
    root: &secure_fs::Directory,
    nonce: &str,
) -> Result<bool, RuntimeError> {
    let name = std::ffi::OsStr::new(".gogglelab-purge-stage");
    let mut marker = match root.open_file(name) {
        Ok(marker) => marker,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error)
            if error.kind() == std::io::ErrorKind::InvalidInput
                || error.kind() == std::io::ErrorKind::NotADirectory
                || is_purge_marker_symlink_loop(&error) =>
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Purge staging marker is unsafe".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        Err(_) => return Err(io_error()),
    };
    if marker.metadata().map_err(|_| io_error())?.len() > 64 {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: "Purge staging marker is unsafe".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }
    let mut bytes = Vec::new();
    (&mut marker)
        .take(65)
        .read_to_end(&mut bytes)
        .map_err(|_| io_error())?;
    if bytes.len() > 64 {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: "Purge staging marker is unsafe".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }
    Ok(bytes == nonce.as_bytes())
}

fn write_purge_stage_marker_at(
    directory: &secure_fs::Directory,
    root: &Path,
    nonce: &str,
) -> Result<GeneratedFileReceipt, RuntimeError> {
    create_generated_purge_marker(
        &GuardedDirectory(directory.try_clone().map_err(|_| io_error())?),
        root,
        nonce,
    )
    .map_err(generated_runtime_error)
}
fn remove_purge_stage_marker_at(
    directory: &secure_fs::Directory,
    root: &Path,
    nonce: &str,
) -> Result<(), RuntimeError> {
    let directory = GuardedDirectory(directory.try_clone().map_err(|_| io_error())?);
    if let Some(mut marker) =
        recover_generated_purge_marker(&directory, root, nonce).map_err(generated_runtime_error)?
    {
        directory
            .delete_generated_file(&mut marker)
            .map_err(generated_runtime_error)?;
    }
    directory.sync().map_err(generated_runtime_error)
}
fn generated_runtime_error(error: io::Error) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Corrupt,
        message: format!("Generated transaction metadata cleanup or finalization failed: {error}"),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    }
}

#[cfg(test)]
fn write_purge_journal(root: &Path, journal: &PurgeJournal) -> Result<PathBuf, RuntimeError> {
    let canonical = canonical_path_allow_missing_tail(root)?;
    let root_path = canonical
        .to_str()
        .ok_or_else(|| invalid("Purge requires a Unicode Library path"))?;
    let root_name = canonical.file_name().unwrap_or_default().to_string_lossy();
    if journal.root_path != root_path
        || journal.version != 1
        || journal.nonce.len() != 32
        || !journal.nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
        || journal.staged_name != format!(".gogglelab-empty-{}", journal.nonce)
        || journal.previous_name != format!("{root_name}.previous-purge-{}", journal.nonce)
        || journal.committed
    {
        return Err(invalid("Purge journal does not match the Library root"));
    }
    let parent = canonical
        .parent()
        .ok_or_else(|| invalid("Library root has no parent directory"))?;
    let path = purge_journal_path(&canonical)?;
    let bytes =
        serde_json::to_vec(journal).map_err(|_| internal("Purge journal encoding failed"))?;
    if bytes.len() > 8 * 1024 {
        return Err(invalid("Purge journal exceeds its size limit"));
    }
    match fs::symlink_metadata(&path) {
        Ok(_) => return Err(stale("A purge recovery journal already exists")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(io_error()),
    }
    if remove_purge_journal_temp(&path)? {
        sync_directory(parent)?;
    }
    let temporary = purge_journal_temp_path(&path);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|_| io_error())?;
    if file
        .write_all(&bytes)
        .and_then(|_| file.sync_all())
        .is_err()
    {
        let _ = fs::remove_file(&temporary);
        let _ = sync_directory(parent);
        return Err(io_error());
    }
    drop(file);
    if fs::hard_link(&temporary, &path).is_err() {
        let _ = fs::remove_file(&temporary);
        let _ = sync_directory(parent);
        return Err(io_error());
    }
    if fs::remove_file(&temporary).is_err() || sync_directory(parent).is_err() {
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&temporary);
        let _ = sync_directory(parent);
        return Err(io_error());
    }
    Ok(path)
}

fn write_purge_journal_at(
    parent: &secure_fs::Directory,
    root: &Path,
    journal: &PurgeJournal,
) -> Result<(PathBuf, GeneratedTransactionJournal), RuntimeError> {
    let path = purge_journal_path(root)?;
    let bytes =
        serde_json::to_vec(journal).map_err(|_| internal("Purge journal encoding failed"))?;
    let parent = GuardedDirectoryParent(parent.try_clone().map_err(|_| io_error())?);
    let journal =
        GeneratedTransactionJournal::create(&parent, root, GeneratedControlKind::Purge, &bytes)
            .map_err(generated_runtime_error)?;
    Ok((path, journal))
}

fn clear_purge_journal_at(
    parent: &secure_fs::Directory,
    journal: &mut GeneratedTransactionJournal,
) -> Result<(), RuntimeError> {
    journal
        .clear(&GuardedDirectoryParent(
            parent.try_clone().map_err(|_| io_error())?,
        ))
        .map_err(generated_runtime_error)
}

fn mark_purge_journal_committed_at(
    parent: &secure_fs::Directory,
    root: &Path,
    journal: &mut GeneratedTransactionJournal,
    nonce: &str,
) -> Result<(), RuntimeError> {
    if journal.namespace.root() != root
        || journal.nonce() != nonce
        || journal.kind != GeneratedControlKind::Purge
    {
        return Err(stale("Purge journal changed before commit"));
    }
    journal
        .mark_committed(&GuardedDirectoryParent(
            parent.try_clone().map_err(|_| io_error())?,
        ))
        .map_err(generated_runtime_error)
}

fn finish_catalog_purge_commit<M, C>(
    guard: &mut RuntimeMaintenanceGuard,
    staged_root: &secure_fs::Directory,
    parent: &secure_fs::Directory,
    root: &Path,
    journal_name: &mut GeneratedTransactionJournal,
    nonce: &str,
    remove_marker: M,
    clear_journal: C,
) -> Result<(), RuntimeError>
where
    M: FnOnce(&secure_fs::Directory, &str) -> Result<(), RuntimeError>,
    C: FnOnce(&secure_fs::Directory, &mut GeneratedTransactionJournal) -> Result<(), RuntimeError>,
{
    let result = (|| {
        mark_purge_journal_committed_at(parent, root, journal_name, nonce)?;
        remove_marker(staged_root, nonce)?;
        clear_journal(parent, journal_name)
    })();
    if let Err(error) = result {
        guard.remain_armed();
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
fn restore_journal_path(root: &Path) -> Result<PathBuf, RuntimeError> {
    let canonical = canonical_path_allow_missing_tail(root)?;
    let parent = canonical
        .parent()
        .ok_or_else(|| invalid("Library root has no parent directory"))?;
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    let suffix = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(parent.join(format!(".gogglelab-restore-{suffix}.journal")))
}

fn write_restore_journal_at(
    parent: &secure_fs::Directory,
    root: &Path,
    journal: &RestoreJournal,
) -> Result<GeneratedTransactionJournal, RuntimeError> {
    let bytes =
        serde_json::to_vec(journal).map_err(|_| internal("Restore journal encoding failed"))?;
    GeneratedTransactionJournal::create(
        &GuardedDirectoryParent(parent.try_clone().map_err(|_| io_error())?),
        root,
        GeneratedControlKind::Restore,
        &bytes,
    )
    .map_err(generated_runtime_error)
}

fn mark_restore_journal_committed_at(
    parent: &secure_fs::Directory,
    root: &Path,
    journal: &mut GeneratedTransactionJournal,
    nonce: &str,
) -> Result<(), RuntimeError> {
    if journal.namespace.root() != root
        || journal.nonce() != nonce
        || journal.kind != GeneratedControlKind::Restore
    {
        return Err(stale("Restore journal changed before commit"));
    }
    journal
        .mark_committed(&GuardedDirectoryParent(
            parent.try_clone().map_err(|_| io_error())?,
        ))
        .map_err(generated_runtime_error)
}

fn clear_restore_journal_at(
    parent: &secure_fs::Directory,
    journal: &mut GeneratedTransactionJournal,
) -> Result<(), RuntimeError> {
    journal
        .clear(&GuardedDirectoryParent(
            parent.try_clone().map_err(|_| io_error())?,
        ))
        .map_err(generated_runtime_error)
}

fn recover_interrupted_library_restore(configured_root: &Path) -> Result<(), RuntimeError> {
    recover_interrupted_library_restore_with_hook(configured_root, || {})
}

fn recover_interrupted_library_restore_with_hook(
    configured_root: &Path,
    after_parent_open: impl FnOnce(),
) -> Result<(), RuntimeError> {
    let root = canonical_path_allow_missing_tail(configured_root)?;
    let parent_path = root
        .parent()
        .ok_or_else(|| invalid("Library root has no parent directory"))?;
    let parent = secure_fs::Directory::open(parent_path).map_err(|_| io_error())?;
    after_parent_open();
    let Some(mut journal_control) = GeneratedTransactionJournal::recover(
        &GuardedDirectoryParent(parent.try_clone().map_err(|_| io_error())?),
        &root,
        GeneratedControlKind::Restore,
    )
    .map_err(generated_runtime_error)?
    else {
        return Ok(());
    };
    let journal: RestoreJournal = serde_json::from_slice(journal_control.bytes())
        .map_err(|_| internal("Recovered restore journal shape changed"))?;
    let root_text = root
        .to_str()
        .ok_or_else(|| invalid("Restore recovery requires a Unicode Library path"))?;
    let root_name = root.file_name().unwrap_or_default().to_string_lossy();
    if journal.version != 1
        || journal.root_path != root_text
        || journal.nonce.len() != 32
        || !journal.nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
        || journal.staged_name != format!(".gogglelab-restore-{}", journal.nonce)
        || journal.previous_name != format!("{root_name}.previous-{}", journal.nonce)
        || journal.failed_name != format!(".gogglelab-failed-restore-{}", journal.nonce)
    {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: "Restore recovery journal does not match the Library".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }
    require_durable_source_mutations()?;
    let root_name = root
        .file_name()
        .ok_or_else(|| invalid("Restore root name is invalid"))?;
    let root_directory = open_optional_real_directory_at(&parent, root_name)?;
    let previous_name = std::ffi::OsString::from(&journal.previous_name);
    let staged_name = std::ffi::OsString::from(&journal.staged_name);
    let previous_directory = open_optional_real_directory_at(&parent, &previous_name)?;
    let staged_directory = open_optional_real_directory_at(&parent, &staged_name)?;
    let root_exists = root_directory.is_some();
    let previous_exists = previous_directory.is_some();
    let staged_exists = staged_directory.is_some();
    if journal.committed {
        if !root_exists {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Committed restore has no current Library; previous Library was preserved"
                    .into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        clear_restore_journal_at(&parent, &mut journal_control)?;
        return Ok(());
    }
    if previous_exists {
        if root_exists {
            if parent
                .entry_exists(std::ffi::OsStr::new(&journal.failed_name))
                .map_err(|_| io_error())?
            {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Corrupt,
                    message: "Restore recovery destination is already occupied".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            parent
                .rename_child_exclusive(root_name, std::ffi::OsStr::new(&journal.failed_name))
                .map_err(|_| io_error())?;
            parent.sync().map_err(|_| io_error())?;
        }
        parent
            .rename_child_exclusive(std::ffi::OsStr::new(&journal.previous_name), root_name)
            .map_err(|_| RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Previous Library could not be restored after interrupted restore".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })?;
        parent.sync().map_err(|_| io_error())?;
    } else if !root_exists {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: "Interrupted restore has no recoverable Library".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }
    if staged_exists {
        parent
            .remove_child_tree(std::ffi::OsStr::new(&journal.staged_name))
            .map_err(|_| io_error())?;
        parent.sync().map_err(|_| io_error())?;
    }
    clear_restore_journal_at(&parent, &mut journal_control)
}

fn recover_interrupted_catalog_purge(configured_root: &Path) -> Result<(), RuntimeError> {
    recover_interrupted_catalog_purge_with_hook(configured_root, || {})
}

fn recover_interrupted_catalog_purge_with_hook(
    configured_root: &Path,
    after_parent_open: impl FnOnce(),
) -> Result<(), RuntimeError> {
    let configured_entry = configured_root
        .file_name()
        .map(|name| {
            configured_root
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(name)
        })
        .unwrap_or_else(|| configured_root.to_path_buf());
    match fs::symlink_metadata(&configured_entry) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Library root cannot be a symlink during purge recovery".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Library root is not a directory during purge recovery".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(io_error()),
    }

    let root = canonical_path_allow_missing_tail(configured_root)?;
    let parent_path = root
        .parent()
        .ok_or_else(|| invalid("Library root has no parent directory"))?;
    let root_name = root
        .file_name()
        .ok_or_else(|| invalid("Library root name is missing"))?
        .to_os_string();
    let parent = secure_fs::Directory::open(parent_path).map_err(|_| io_error())?;
    if !parent.matches_path(parent_path).map_err(|_| io_error())? {
        return Err(stale("Purge recovery parent identity changed"));
    }
    after_parent_open();
    let Some(mut journal_control) = GeneratedTransactionJournal::recover(
        &GuardedDirectoryParent(parent.try_clone().map_err(|_| io_error())?),
        &root,
        GeneratedControlKind::Purge,
    )
    .map_err(generated_runtime_error)?
    else {
        return Ok(());
    };
    let journal: PurgeJournal = serde_json::from_slice(journal_control.bytes())
        .map_err(|_| internal("Recovered purge journal shape changed"))?;
    let root_text = root
        .to_str()
        .ok_or_else(|| invalid("Purge recovery requires a Unicode Library path"))?;
    if journal.version != 1
        || journal.root_path != root_text
        || journal.nonce.len() != 32
        || !journal.nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
        || journal.staged_name != format!(".gogglelab-empty-{}", journal.nonce)
        || journal.previous_name
            != format!(
                "{}.previous-purge-{}",
                root.file_name().unwrap_or_default().to_string_lossy(),
                journal.nonce
            )
        || journal
            .previous_name
            .as_bytes()
            .iter()
            .any(|byte| *byte == b'/' || *byte == b'\\' || *byte == 0)
    {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: "Purge recovery journal does not match the Library".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }

    let previous_name = std::ffi::OsString::from(&journal.previous_name);
    let staged_name = std::ffi::OsString::from(&journal.staged_name);
    let root_directory = open_optional_real_directory_at(&parent, &root_name)?;
    let previous_directory = open_optional_real_directory_at(&parent, &previous_name)?;
    let staged_directory = open_optional_real_directory_at(&parent, &staged_name)?;
    require_durable_source_mutations()?;

    if journal.committed {
        let root_directory = root_directory.ok_or_else(|| RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: "Committed purge has no current Library; recovery data was preserved".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })?;
        remove_purge_stage_marker_at(&root_directory, &root, &journal.nonce)?;
        clear_purge_journal_at(&parent, &mut journal_control)?;
        return Ok(());
    }

    if previous_directory.is_some() {
        if root_directory.is_some() {
            let failed_name =
                std::ffi::OsString::from(format!(".gogglelab-interrupted-purge-{}", journal.nonce));
            if parent.entry_exists(&failed_name).map_err(|_| io_error())? {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Corrupt,
                    message: "Purge recovery destination is already occupied".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            parent
                .rename_child_exclusive(&root_name, &failed_name)
                .map_err(|_| io_error())?;
            parent.sync().map_err(|_| io_error())?;
        }
        parent
            .rename_child_exclusive(&previous_name, &root_name)
            .map_err(|_| RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Previous Library could not be restored after interrupted purge".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })?;
        parent.sync().map_err(|_| io_error())?;
    } else {
        let root_directory = root_directory.as_ref().ok_or_else(|| RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: "Interrupted purge has no recoverable previous Library".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })?;
        if purge_stage_marker_matches_at(root_directory, &journal.nonce)? {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Interrupted purge has no recoverable previous Library".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
    }

    // A published journal owns this exact nonce-bound staging child.
    if staged_directory.is_some() {
        parent
            .remove_child_tree(&staged_name)
            .map_err(|_| io_error())?;
        parent.sync().map_err(|_| io_error())?;
    }
    clear_purge_journal_at(&parent, &mut journal_control)
}

fn open_optional_real_directory_at(
    parent: &secure_fs::Directory,
    name: &std::ffi::OsStr,
) -> Result<Option<secure_fs::Directory>, RuntimeError> {
    match parent.open_child(name) {
        Ok(directory) => Ok(Some(directory)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: "Recovery directory is unsafe".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        }),
    }
}

fn validate_physical_model_action(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    model_id: &str,
) -> Result<(), RuntimeError> {
    runtime.require(context, Capability::CatalogWrite)?;
    runtime.require(context, Capability::PhysicalSourceWrite)?;
    let model = runtime
        .read_db(|db| db.get_model(model_id))?
        .ok_or_else(|| RuntimeError {
            code: RuntimeErrorCode::NotFound,
            message: "Model was not found".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })?;
    if model.source_kind != "indexed" || model.source_archive_member.is_some() {
        return Err(invalid("Physical action requires a loose indexed model"));
    }
    let root_id = model
        .source_root_id
        .as_deref()
        .ok_or_else(|| invalid("Indexed model has no source root"))?;
    let admitted = runtime
        .roots
        .iter()
        .find(|root| {
            root.catalog_root_id.as_deref() == Some(root_id)
                && root.handle.access == RootAccess::Writable
        })
        .ok_or_else(|| RuntimeError {
            code: RuntimeErrorCode::Forbidden,
            message: "Model source root is not admitted for writes".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })?;
    if admitted.path.canonicalize().map_err(|_| io_error())? != admitted.path {
        return Err(stale("Model source root identity changed"));
    }
    Ok(())
}

fn validate_physical_targets<'a>(
    model_ids: impl IntoIterator<Item = &'a str>,
) -> Result<(), RuntimeError> {
    let ids = model_ids.into_iter().collect::<Vec<_>>();
    if ids.is_empty() || ids.len() > MAX_PHYSICAL_MUTATION_TARGETS {
        return Err(invalid("Physical source target count is invalid"));
    }
    let mut unique = BTreeSet::new();
    for id in ids {
        validate_plain_id_text(id, "Physical source model")?;
        if !unique.insert(id) {
            return Err(invalid("Physical source targets must be unique"));
        }
    }
    Ok(())
}

fn validate_physical_source_batch(
    snapshots: &[PhysicalSourceSnapshot],
) -> Result<(), RuntimeError> {
    if snapshots.is_empty() || snapshots.len() > MAX_PHYSICAL_MUTATION_TARGETS {
        return Err(invalid("Physical source target count is invalid"));
    }
    let mut unique = BTreeSet::new();
    let total_bytes = snapshots.iter().try_fold(0u64, |total, snapshot| {
        if !unique.insert(snapshot.model_id.as_str()) {
            return Err(invalid("Physical source targets must be unique"));
        }
        total
            .checked_add(snapshot.bytes)
            .ok_or_else(|| invalid("Physical source byte total is out of range"))
    })?;
    if total_bytes > MAX_PHYSICAL_MUTATION_BYTES {
        return Err(invalid("Physical source batch exceeds its byte limit"));
    }
    Ok(())
}

fn snapshot_active_source(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    target: &VersionedTarget,
) -> Result<PhysicalSourceSnapshot, RuntimeError> {
    let model = runtime.read_db(|db| {
        let model = db
            .get_model(&target.model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(target.model_id.clone()))?;
        if model.metadata_version != target.expected_version {
            return Err(LibraryError::MetadataConflict {
                model_id: target.model_id.clone(),
                expected: target.expected_version,
                actual: model.metadata_version,
            });
        }
        Ok(model)
    })?;
    let snapshot = source_location_snapshot(runtime, context, &model)?;
    let source = runtime.read_db(|db| db.resolve_model_source_ref(&model.id))?;
    if source.outer_path != snapshot.root_path.join(&snapshot.relative_path)
        || source.archive_member.is_some()
    {
        return Err(stale("Indexed source identity changed"));
    }
    verify_source_snapshot_file(&snapshot)?;
    if model.bytes != snapshot.bytes || model.content_hash.as_deref() != Some(&snapshot.sha256) {
        return Err(stale("Indexed source bytes do not match the catalog"));
    }
    Ok(snapshot)
}

fn source_location_snapshot(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    model: &Model,
) -> Result<PhysicalSourceSnapshot, RuntimeError> {
    runtime.require(context, Capability::CatalogWrite)?;
    runtime.require(context, Capability::PhysicalSourceWrite)?;
    if model.source_kind != "indexed" || model.source_archive_member.is_some() {
        return Err(invalid("Physical action requires a loose indexed model"));
    }
    let catalog_root_id = model
        .source_root_id
        .as_deref()
        .ok_or_else(|| invalid("Indexed model has no source root"))?;
    let root = runtime
        .roots
        .iter()
        .find(|root| {
            root.catalog_root_id.as_deref() == Some(catalog_root_id)
                && root.handle.access == RootAccess::Writable
        })
        .ok_or_else(|| RuntimeError {
            code: RuntimeErrorCode::Forbidden,
            message: "Model source root is not admitted for writes".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })?;
    let root_path = runtime.admitted_root_path(context, &root.handle.id, RootAccess::Writable)?;
    if root_path.canonicalize().map_err(|_| io_error())? != root_path {
        return Err(stale("Model source root identity changed"));
    }
    let stored_source_path = PathBuf::from(
        model
            .source_path
            .as_deref()
            .ok_or_else(|| invalid("Indexed model has no source path"))?,
    );
    let relative_path = if stored_source_path.is_absolute() {
        stored_source_path
            .strip_prefix(&root_path)
            .map_err(|_| invalid("Indexed source path is outside its admitted root"))?
            .to_path_buf()
    } else {
        stored_source_path
    };
    validate_relative_source_path(&relative_path)?;
    Ok(PhysicalSourceSnapshot {
        model_id: model.id.clone(),
        metadata_version: model.metadata_version,
        root_handle_id: root.handle.id.clone(),
        catalog_root_id: catalog_root_id.into(),
        root_path,
        relative_path,
        bytes: model.bytes,
        sha256: model
            .content_hash
            .clone()
            .ok_or_else(|| invalid("Indexed model has no content hash"))?,
    })
}

fn snapshot_quarantine_sources(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    snapshot_id: &str,
) -> Result<Vec<PhysicalSourceSnapshot>, RuntimeError> {
    validate_plain_id_text(snapshot_id, "Quarantine snapshot")?;
    let removed = runtime
        .read_db(|db| {
            db.list_removed_models().map(|removed| {
                removed
                    .into_iter()
                    .filter(|item| item.snapshot_id == snapshot_id)
                    .collect::<Vec<_>>()
            })
        })?
        .into_iter()
        .collect::<Vec<_>>();
    if removed.is_empty() {
        return Err(RuntimeError {
            code: RuntimeErrorCode::NotFound,
            message: "Quarantine snapshot was not found".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }
    let mut snapshots = Vec::with_capacity(removed.len());
    for item in removed {
        let snapshot = source_location_snapshot(runtime, context, &item.model)?;
        let recovery_source =
            quarantine_source_for_model(&runtime.root, snapshot_id, &snapshot.model_id, false)?;
        let (bytes, sha256) =
            hash_management_file(&recovery_source, library_core::MAX_INDEXED_FILE_BYTES)?;
        if bytes != snapshot.bytes || sha256 != snapshot.sha256 {
            return Err(stale(
                "Quarantined source does not match its catalog record",
            ));
        }
        let original_path = snapshot.root_path.join(&snapshot.relative_path);
        validate_relative_target(&snapshot.root_path, &snapshot.relative_path, false)?;
        match fs::symlink_metadata(&original_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(metadata)
                if metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && hash_management_file(
                        &original_path,
                        library_core::MAX_INDEXED_FILE_BYTES,
                    )
                    .is_ok_and(|fingerprint| {
                        fingerprint == (snapshot.bytes, snapshot.sha256.clone())
                    }) =>
            {
                // A previous attempt may have removed the catalog rows before it
                // finished deleting original files. Matching bytes are safe to reuse.
            }
            Ok(_) => {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Conflict,
                    message: "Original source path is occupied by different bytes".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            Err(_) => return Err(io_error()),
        }
        snapshots.push(snapshot);
    }
    validate_physical_source_batch(&snapshots)?;
    Ok(snapshots)
}

fn validate_relative_source_path(path: &Path) -> Result<(), RuntimeError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(invalid("Indexed source path is unsafe"));
    }
    Ok(())
}

fn model_relative_source_path(model: &Model, root: &Path) -> Result<PathBuf, RuntimeError> {
    let stored = PathBuf::from(
        model
            .source_path
            .as_deref()
            .ok_or_else(|| invalid("Indexed model has no source path"))?,
    );
    let relative = if stored.is_absolute() {
        stored
            .strip_prefix(root)
            .map_err(|_| invalid("Indexed source path is outside its admitted root"))?
            .to_path_buf()
    } else {
        stored
    };
    validate_relative_source_path(&relative)?;
    Ok(relative)
}

fn validate_relative_target(
    root: &Path,
    relative: &Path,
    require_file: bool,
) -> Result<PathBuf, RuntimeError> {
    validate_relative_source_path(relative)?;
    if root.canonicalize().map_err(|_| io_error())? != root {
        return Err(stale("Admitted source root identity changed"));
    }
    let mut path = root.to_path_buf();
    let parts = relative.components().collect::<Vec<_>>();
    for (index, component) in parts.iter().enumerate() {
        let std::path::Component::Normal(name) = component else {
            return Err(invalid("Indexed source path is unsafe"));
        };
        path.push(name);
        let final_component = index + 1 == parts.len();
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    || (!final_component && !metadata.is_dir())
                    || (final_component && require_file && !metadata.is_file()) =>
            {
                return Err(invalid(
                    "Indexed source path is not a regular contained file",
                ));
            }
            Ok(_) => {}
            Err(error)
                if final_component
                    && !require_file
                    && error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(io_error());
            }
            Err(_) => return Err(io_error()),
        }
    }
    Ok(path)
}

fn verify_source_snapshot_file(snapshot: &PhysicalSourceSnapshot) -> Result<PathBuf, RuntimeError> {
    let path = validate_relative_target(&snapshot.root_path, &snapshot.relative_path, true)?;
    let (bytes, sha256) = hash_management_file(&path, library_core::MAX_INDEXED_FILE_BYTES)?;
    if bytes != snapshot.bytes || sha256 != snapshot.sha256 {
        return Err(stale("Indexed source bytes changed after planning"));
    }
    Ok(path)
}

fn open_physical_source_parent(
    snapshot: &PhysicalSourceSnapshot,
) -> Result<(secure_fs::Directory, std::ffi::OsString), RuntimeError> {
    validate_relative_source_path(&snapshot.relative_path)?;
    let root = secure_fs::Directory::open(&snapshot.root_path)
        .map_err(|_| stale("Indexed source root is no longer a safe directory"))?;
    if !root
        .matches_path(&snapshot.root_path)
        .map_err(|_| stale("Indexed source root identity changed"))?
    {
        return Err(stale("Indexed source root identity changed"));
    }
    let parent = snapshot
        .relative_path
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let directory = root
        .open_relative(parent)
        .map_err(|_| stale("Indexed source parent changed"))?;
    let name = snapshot
        .relative_path
        .file_name()
        .ok_or_else(|| invalid("Indexed source path has no file name"))?
        .to_os_string();
    Ok((directory, name))
}

fn quarantine_source_for_model(
    library_root: &Path,
    snapshot_id: &str,
    model_id: &str,
    create: bool,
) -> Result<PathBuf, RuntimeError> {
    validate_plain_id_text(model_id, "Quarantine model")?;
    let owned = quarantine_source_directory(library_root, snapshot_id, create)?;
    let model_file = owned.join(format!("{model_id}.bin"));
    match fs::symlink_metadata(&model_file) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() && !create => {
            Ok(model_file)
        }
        Ok(_) => Err(RuntimeError {
            code: RuntimeErrorCode::Conflict,
            message: "Quarantine model recovery path is occupied".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => Ok(model_file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let legacy = owned.join("source.bin");
            match fs::symlink_metadata(&legacy) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    Ok(legacy)
                }
                _ => Err(RuntimeError {
                    code: RuntimeErrorCode::NotFound,
                    message: "Quarantine model recovery file was not found".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                }),
            }
        }
        Err(_) => Err(io_error()),
    }
}

fn quarantine_source_directory(
    library_root: &Path,
    snapshot_id: &str,
    create: bool,
) -> Result<PathBuf, RuntimeError> {
    validate_plain_id_text(snapshot_id, "Quarantine snapshot")?;
    let trash_root = library_root.join("trash");
    match fs::symlink_metadata(&trash_root) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(invalid("Quarantine directory is unsafe")),
        Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&trash_root).map_err(|_| io_error())?;
            sync_directory(library_root)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(RuntimeError {
                code: RuntimeErrorCode::NotFound,
                message: "Quarantine snapshot was not found".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        Err(_) => return Err(io_error()),
    }
    if trash_root.canonicalize().map_err(|_| io_error())? != trash_root {
        return Err(invalid("Quarantine directory identity changed"));
    }
    let owned = trash_root.join(format!("source-{snapshot_id}"));
    if create {
        match fs::create_dir(&owned) {
            Ok(()) => sync_directory(&trash_root)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Conflict,
                    message: "Quarantine snapshot directory already exists".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            Err(_) => return Err(io_error()),
        }
    } else {
        let metadata = fs::symlink_metadata(&owned).map_err(|_| io_error())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid("Quarantine snapshot directory is unsafe"));
        }
    }
    if owned.canonicalize().map_err(|_| io_error())? != owned {
        return Err(invalid("Quarantine snapshot identity changed"));
    }
    Ok(owned)
}

fn metadata_modified_seconds(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn revalidate_action_targets(
    runtime: &LibraryRuntime,
    targets: &[ActionTargetSnapshot],
) -> Result<(), RuntimeError> {
    runtime.read_db(|db| {
        for target in targets {
            let model = db
                .get_model(&target.model_id)?
                .ok_or_else(|| LibraryError::ModelNotFound(target.model_id.clone()))?;
            if model.metadata_version != target.metadata_version {
                return Err(LibraryError::MetadataConflict {
                    model_id: target.model_id.clone(),
                    expected: target.metadata_version,
                    actual: model.metadata_version,
                });
            }
            if model.content_hash != target.content_hash {
                return Err(LibraryError::SourceChanged);
            }
        }
        Ok(())
    })
}

fn validate_plain_id_text(value: &str, label: &str) -> Result<(), RuntimeError> {
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        })
    {
        return Err(invalid(&format!("{label} is invalid")));
    }
    Ok(())
}

fn validate_output_name(value: &str) -> Result<(), RuntimeError> {
    if value.is_empty()
        || value.len() > 200
        || value == "."
        || value == ".."
        || value.contains(['/', '\\', '\0'])
        || value.chars().any(char::is_control)
    {
        return Err(invalid("Management file name is invalid"));
    }
    Ok(())
}

fn admitted_file(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    handle_id: &str,
    required: RootAccess,
    file_name: &str,
    must_exist: bool,
) -> Result<(PathBuf, PathBuf), RuntimeError> {
    validate_output_name(file_name)?;
    let root = runtime.admitted_root_path(context, handle_id, required)?;
    if root.canonicalize().map_err(|_| io_error())? != root {
        return Err(stale("Operator root identity changed"));
    }
    let path = root.join(file_name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if must_exist && metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) if must_exist => return Err(invalid("Management source is not a regular file")),
        Ok(_) => return Err(invalid("Management destination already exists")),
        Err(error) if !must_exist && error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) if must_exist => return Err(io_error()),
        Err(_) => return Err(io_error()),
    }
    Ok((root, path))
}

fn recheck_management_plan(
    runtime: &LibraryRuntime,
    expected_epoch: u64,
    expected_revision: u64,
) -> Result<(), RuntimeError> {
    if runtime.library_epoch() != expected_epoch || runtime.catalog_revision() != expected_revision
    {
        return Err(stale("Management plan changed before exclusive execution"));
    }
    Ok(())
}

fn hash_management_file(path: &Path, max_bytes: u64) -> Result<(u64, String), RuntimeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| io_error())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > max_bytes {
        return Err(invalid("Management source file is unsafe or too large"));
    }
    let mut file = File::open(path).map_err(|_| io_error())?;
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|_| io_error())?;
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(count as u64)
            .ok_or_else(|| invalid("Management source size overflowed"))?;
        if bytes > metadata.len() || bytes > max_bytes {
            return Err(stale("Management source file changed"));
        }
        digest.update(&buffer[..count]);
    }
    if bytes != metadata.len() {
        return Err(stale("Management source file changed"));
    }
    Ok((bytes, format!("{:x}", digest.finalize())))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), RuntimeError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| io_error())
}

/// Remove an entry from its active name without unlinking bytes. The hold and
/// its durable journal remain beside the original path so cleanup and rollback
/// do not race a same-UID replacement into deleting unrelated data.
fn hold_runtime_path(path: &Path) -> std::io::Result<secure_fs::HeldEntry> {
    let parent_path = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "entry has no parent")
    })?;
    let canonical_parent = parent_path.canonicalize()?;
    if canonical_parent != parent_path {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "entry parent is not canonical",
        ));
    }
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "entry has no name")
    })?;
    secure_fs::Directory::open(&canonical_parent)?.hold_entry(name)
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), RuntimeError> {
    Err(RuntimeError {
        code: RuntimeErrorCode::Unsupported,
        message: "Durable source mutations are unsupported on this host".into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn require_durable_source_mutations() -> Result<(), RuntimeError> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn require_durable_source_mutations() -> Result<(), RuntimeError> {
    Err(RuntimeError {
        code: RuntimeErrorCode::Unsupported,
        message: "Guarded source mutations are unavailable on this platform".into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    })
}

fn execute_quarantine_batch(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    targets: &[VersionedTarget],
    snapshots: &[PhysicalSourceSnapshot],
    expected_epoch: u64,
    expected_revision: u64,
) -> Result<ManagementResult, RuntimeError> {
    require_durable_source_mutations()?;
    runtime.require(context, Capability::CatalogWrite)?;
    runtime.require(context, Capability::PhysicalSourceWrite)?;
    validate_physical_targets(targets.iter().map(|target| target.model_id.as_str()))?;
    validate_physical_source_batch(snapshots)?;
    if targets.len() != snapshots.len()
        || targets.iter().zip(snapshots).any(|(target, snapshot)| {
            target.model_id != snapshot.model_id
                || target.expected_version != snapshot.metadata_version
        })
    {
        return Err(stale("Quarantine plan targets changed"));
    }
    let runtime_arc = runtime
        .self_weak
        .get()
        .and_then(Weak::upgrade)
        .ok_or_else(|| internal("Runtime owner is unavailable"))?;
    let guard = runtime_arc.begin_source_mutation_maintenance()?;
    recheck_management_plan(runtime, expected_epoch, expected_revision)?;
    let library_root = guard.root().to_path_buf();
    let library_directory = secure_fs::Directory::open(&library_root)
        .map_err(|_| stale("Library root is no longer a safe directory"))?;
    if !library_directory
        .matches_path(&library_root)
        .map_err(|_| stale("Library root identity changed"))?
    {
        return Err(stale("Library root identity changed"));
    }
    let db = guard.catalog()?;
    #[cfg(test)]
    runtime.run_after_guarded_catalog_open_hook();
    if !db.catalog_path_matches() {
        return Err(stale(
            "Library catalog identity changed during quarantine setup",
        ));
    }
    if db.root() != library_root {
        return Err(stale("Library root identity changed before quarantine"));
    }
    let source_paths = targets
        .iter()
        .zip(snapshots)
        .map(|(target, snapshot)| revalidate_active_source(db, runtime, context, target, snapshot))
        .collect::<Result<Vec<_>, _>>()?;
    let source_directories = snapshots
        .iter()
        .map(open_physical_source_parent)
        .collect::<Result<Vec<_>, _>>()?;
    let snapshot_id = format!("removal-{}", uuid::Uuid::new_v4().simple());
    let trash_directory = library_directory
        .open_child(std::ffi::OsStr::new("trash"))
        .map_err(|_| invalid("Quarantine directory is unsafe"))?;
    let owned_name = std::ffi::OsString::from(format!("source-{snapshot_id}"));
    trash_directory
        .create_child_directory(&owned_name)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                stale("Quarantine snapshot directory already exists")
            } else {
                io_error()
            }
        })?;
    let owned_directory = trash_directory
        .open_child(&owned_name)
        .map_err(|_| io_error())?;
    for (index, snapshot) in snapshots.iter().enumerate() {
        validate_plain_id_text(&snapshot.model_id, "Quarantine model")?;
        let recovery_name = std::ffi::OsString::from(format!("{}.bin", snapshot.model_id));
        let expected = (snapshot.bytes, snapshot.sha256.clone());
        if secure_fs::copy_verified_into_directory(
            &source_directories[index].0,
            &source_directories[index].1,
            &owned_directory,
            &recovery_name,
            &expected,
            library_core::MAX_INDEXED_FILE_BYTES,
        )
        .is_err()
        {
            let _ = trash_directory.remove_child_tree(&owned_name);
            return Err(stale(
                "Quarantine source changed while copying recovery bytes",
            ));
        }
    }
    if !db.catalog_path_matches() {
        return Err(stale(
            "Library catalog identity changed before quarantine commit; original sources remain active",
        ));
    }
    let removal_targets = targets
        .iter()
        .map(|target| CoreVersionedTarget {
            model_id: target.model_id.clone(),
            expected_version: target.expected_version,
        })
        .collect::<Vec<_>>();
    if let Err(error) =
        db.remove_models_recoverably_with_snapshot_id(&removal_targets, &snapshot_id)
    {
        let _ = trash_directory.remove_child_tree(&owned_name);
        return Err(runtime_library_error(error));
    }
    if !db.catalog_path_matches() {
        if db.restore_removal(&snapshot_id).is_ok() {
            return Err(stale(
                "Library catalog identity changed during quarantine; original sources remain active",
            ));
        }
        guard.remain_armed();
        return Err(RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: format!(
                "Library catalog identity changed during quarantine and its database rollback failed; original sources remain active and recovery bytes remain in trash/source-{snapshot_id}"
            ),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }
    let mut source_holds = Vec::with_capacity(snapshots.len());
    let mut removal_error = None;
    for (index, snapshot) in snapshots.iter().enumerate() {
        let expected = (snapshot.bytes, snapshot.sha256.clone());
        let recovery_name = std::ffi::OsString::from(format!("{}.bin", snapshot.model_id));
        let source_matches = source_directories[index]
            .0
            .open_file(&source_directories[index].1)
            .and_then(|mut file| {
                secure_fs::hash_file(&mut file, library_core::MAX_INDEXED_FILE_BYTES)
            })
            .is_ok_and(|fingerprint| fingerprint == expected);
        let recovery_matches = owned_directory
            .hash_file(&recovery_name, library_core::MAX_INDEXED_FILE_BYTES)
            .is_ok_and(|fingerprint| fingerprint == expected);
        if !source_matches
            || !recovery_matches
            || !source_directories[index]
                .0
                .matches_path(source_paths[index].parent().unwrap())
                .is_ok_and(|matches| matches)
        {
            removal_error = Some(stale("Source or recovery bytes changed during quarantine"));
            break;
        }
        let operation_key = format!("{snapshot_id}-{}", snapshot.model_id);
        let (hold_name, journal_name) = deterministic_source_hold_names(&operation_key)?;
        match source_directories[index].0.hold_verified_file_as(
            &source_directories[index].1,
            &expected,
            library_core::MAX_INDEXED_FILE_BYTES,
            &hold_name,
            &journal_name,
        ) {
            Ok(held) => source_holds.push(source_hold_receipt(snapshot, &held)?),
            Err(error) => {
                if let Some(receipt) = recover_source_hold_receipt(
                    &source_directories[index].0,
                    snapshot,
                    &journal_name,
                ) {
                    source_holds.push(receipt);
                }
                removal_error = Some(RuntimeError {
                    code: RuntimeErrorCode::Io,
                    message: format!(
                        "Quarantine stopped while retaining source bytes: {error}; earlier source holds: {}",
                        source_hold_summary(&source_holds)
                    ),
                    retryable: false,
                    current_version: None,
                    source_holds: source_holds.clone(),
                });
                break;
            }
        }
    }
    if removal_error.is_none() && !db.catalog_path_matches() {
        removal_error = Some(stale(
            "Library catalog identity changed during source quarantine",
        ));
    }
    if let Some(error) = removal_error {
        if let Err(rollback_error) = rollback_quarantine_batch(
            db,
            &guard,
            &snapshot_id,
            snapshots,
            &source_directories,
            &source_paths,
            &trash_directory,
            &owned_name,
            &owned_directory,
        ) {
            guard.remain_armed();
            return Err(RuntimeError {
                message: format!(
                    "{}; source recovery holds: {}",
                    rollback_error.message,
                    source_hold_summary(&source_holds)
                ),
                source_holds: source_holds.clone(),
                ..rollback_error
            });
        }
        return Err(RuntimeError {
            message: format!(
                "{}; source recovery holds: {}",
                error.message,
                source_hold_summary(&source_holds)
            ),
            source_holds: source_holds.clone(),
            ..error
        });
    }
    guard.invalidate();
    let model_ids = targets
        .iter()
        .map(|target| target.model_id.clone())
        .collect::<Vec<_>>();
    if model_ids.len() == 1 {
        Ok(ManagementResult::Quarantined {
            snapshot_id,
            model_id: model_ids[0].clone(),
            source_holds,
        })
    } else {
        Ok(ManagementResult::QuarantinedBatch {
            snapshot_id,
            models: model_ids,
            source_holds,
        })
    }
}

fn rollback_quarantine_batch(
    db: &LibraryDb,
    guard: &RuntimeMaintenanceGuard,
    snapshot_id: &str,
    snapshots: &[PhysicalSourceSnapshot],
    source_directories: &[(secure_fs::Directory, std::ffi::OsString)],
    source_paths: &[PathBuf],
    trash_directory: &secure_fs::Directory,
    owned_name: &std::ffi::OsStr,
    owned_directory: &secure_fs::Directory,
) -> Result<(), RuntimeError> {
    for (index, snapshot) in snapshots.iter().enumerate() {
        let expected = (snapshot.bytes, snapshot.sha256.clone());
        let recovery_name = std::ffi::OsString::from(format!("{}.bin", snapshot.model_id));
        if !owned_directory
            .hash_file(&recovery_name, library_core::MAX_INDEXED_FILE_BYTES)
            .is_ok_and(|fingerprint| fingerprint == expected)
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Quarantine recovery bytes changed; originals were preserved".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        if !source_directories[index]
            .0
            .matches_path(source_paths[index].parent().unwrap())
            .is_ok_and(|matches| matches)
        {
            return Err(stale("Source root changed and blocks quarantine rollback"));
        }
        match source_directories[index]
            .0
            .open_file(&source_directories[index].1)
        {
            Ok(mut file) => {
                if !secure_fs::hash_file(&mut file, library_core::MAX_INDEXED_FILE_BYTES)
                    .is_ok_and(|fingerprint| fingerprint == expected)
                {
                    return Err(RuntimeError {
                        code: RuntimeErrorCode::Corrupt,
                        message: "A changed source path blocks quarantine rollback".into(),
                        retryable: false,
                        current_version: None,
                        source_holds: Vec::new(),
                    });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                secure_fs::copy_verified_into_directory(
                    owned_directory,
                    &recovery_name,
                    &source_directories[index].0,
                    &source_directories[index].1,
                    &expected,
                    library_core::MAX_INDEXED_FILE_BYTES,
                )
                .map_err(|_| stale("Quarantine recovery copy could not be restored"))?;
            }
            Err(_) => return Err(io_error()),
        }
    }
    restore_quarantine_catalog(db, snapshot_id)?;
    trash_directory
        .remove_child_tree(owned_name)
        .map_err(|_| io_error())?;
    guard.invalidate();
    Ok(())
}

fn restore_quarantine_catalog(db: &LibraryDb, snapshot_id: &str) -> Result<(), RuntimeError> {
    if db.restore_removal(snapshot_id).is_ok() {
        return Ok(());
    }
    Err(RuntimeError {
        code: RuntimeErrorCode::Corrupt,
        message: "Quarantine failed and the catalog rollback did not complete".into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    })
}

fn retain_created_quarantine_source_files(
    source_directories: &[(secure_fs::Directory, std::ffi::OsString)],
    expected_files: &[(u64, String)],
    created: &[bool],
    snapshots: &[PhysicalSourceSnapshot],
    snapshot_id: &str,
) -> (Vec<SourceHoldReceipt>, Vec<String>) {
    let mut source_holds = Vec::new();
    let mut failures = Vec::new();
    for (index, created) in created.iter().enumerate() {
        if !created {
            continue;
        }
        let operation_key = format!("undo-{snapshot_id}-{}", snapshots[index].model_id);
        let (hold_name, journal_name) = match deterministic_source_hold_names(&operation_key) {
            Ok(names) => names,
            Err(error) => {
                failures.push(format!("{}: {error}", snapshots[index].model_id));
                continue;
            }
        };
        match source_directories[index].0.hold_verified_file_as(
            &source_directories[index].1,
            &expected_files[index],
            library_core::MAX_INDEXED_FILE_BYTES,
            &hold_name,
            &journal_name,
        ) {
            Ok(held) => match source_hold_receipt(&snapshots[index], &held) {
                Ok(receipt) => source_holds.push(receipt),
                Err(error) => failures.push(format!("{}: {}", snapshots[index].model_id, error)),
            },
            Err(error) => {
                if let Some(receipt) = recover_source_hold_receipt(
                    &source_directories[index].0,
                    &snapshots[index],
                    &journal_name,
                ) {
                    source_holds.push(receipt);
                }
                failures.push(format!("{}: {error}", snapshots[index].model_id));
            }
        }
    }
    (source_holds, failures)
}

fn quarantine_undo_cleanup_error(
    message: &str,
    source_holds: Vec<SourceHoldReceipt>,
    hold_failures: Vec<String>,
) -> RuntimeError {
    let code = if hold_failures.is_empty() {
        RuntimeErrorCode::Stale
    } else {
        RuntimeErrorCode::Corrupt
    };
    let details = if hold_failures.is_empty() {
        format!(
            "recoverable source holds: {}",
            source_hold_summary(&source_holds)
        )
    } else {
        format!(
            "hold failures: {}; successful source holds: {}",
            hold_failures.join(", "),
            source_hold_summary(&source_holds)
        )
    };
    RuntimeError {
        code,
        message: format!("{message}; {details}"),
        retryable: false,
        current_version: None,
        source_holds,
    }
}

fn execute_undo_quarantine_batch(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    snapshot_id: &str,
    snapshots: &[PhysicalSourceSnapshot],
    expected_epoch: u64,
    expected_revision: u64,
) -> Result<ManagementResult, RuntimeError> {
    require_durable_source_mutations()?;
    runtime.require(context, Capability::CatalogWrite)?;
    runtime.require(context, Capability::PhysicalSourceWrite)?;
    validate_plain_id_text(snapshot_id, "Quarantine snapshot")?;
    validate_physical_source_batch(snapshots)?;
    let runtime_arc = runtime
        .self_weak
        .get()
        .and_then(Weak::upgrade)
        .ok_or_else(|| internal("Runtime owner is unavailable"))?;
    let guard = runtime_arc.begin_source_mutation_maintenance()?;
    recheck_management_plan(runtime, expected_epoch, expected_revision)?;
    let library_root = guard.root().to_path_buf();
    let library_directory = secure_fs::Directory::open(&library_root)
        .map_err(|_| stale("Library root is no longer a safe directory"))?;
    if !library_directory
        .matches_path(&library_root)
        .map_err(|_| stale("Library root identity changed"))?
    {
        return Err(stale("Library root identity changed"));
    }
    let db = guard.catalog()?;
    #[cfg(test)]
    runtime.run_after_guarded_catalog_open_hook();
    if !db.catalog_path_matches() {
        return Err(stale(
            "Library catalog identity changed during quarantine undo setup",
        ));
    }
    if db.root() != library_root {
        return Err(stale(
            "Library root identity changed before quarantine undo",
        ));
    }
    let removed = db
        .list_removed_models()
        .map_err(runtime_library_error)?
        .into_iter()
        .filter(|item| item.snapshot_id == snapshot_id)
        .collect::<Vec<_>>();
    if removed.len() != snapshots.len() {
        return Err(stale("Quarantine snapshot membership changed"));
    }
    let trash_directory = library_directory
        .open_child(std::ffi::OsStr::new("trash"))
        .map_err(|_| stale("Quarantine recovery namespace is unsafe"))?;
    let owned_name = std::ffi::OsString::from(format!("source-{snapshot_id}"));
    let owned_directory = trash_directory
        .open_child(&owned_name)
        .map_err(|_| stale("Quarantine recovery directory is unavailable"))?;
    let source_directories = snapshots
        .iter()
        .map(open_physical_source_parent)
        .collect::<Result<Vec<_>, _>>()?;
    let mut already_present = Vec::with_capacity(snapshots.len());
    let mut expected_files = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let item = removed
            .iter()
            .find(|item| item.model.id == snapshot.model_id)
            .ok_or_else(|| stale("Quarantine snapshot membership changed"))?;
        if item.model.metadata_version != snapshot.metadata_version
            || item.model.content_hash.as_deref() != Some(snapshot.sha256.as_str())
            || item.model.bytes != snapshot.bytes
        {
            return Err(stale("Quarantine catalog record changed"));
        }
        let expected = (snapshot.bytes, snapshot.sha256.clone());
        let recovery_name = std::ffi::OsString::from(format!("{}.bin", snapshot.model_id));
        if !owned_directory
            .hash_file(&recovery_name, library_core::MAX_INDEXED_FILE_BYTES)
            .is_ok_and(|fingerprint| fingerprint == expected)
        {
            return Err(stale("Quarantine recovery bytes changed"));
        }
        let destination = revalidate_removed_destination(runtime, context, &item.model, snapshot)?;
        if destination != snapshot.root_path.join(&snapshot.relative_path)
            || !source_directories[expected_files.len()]
                .0
                .matches_path(destination.parent().unwrap())
                .is_ok_and(|matches| matches)
        {
            return Err(stale("Quarantine destination root changed"));
        }
        let source_name = &source_directories[expected_files.len()].1;
        let exists = match source_directories[expected_files.len()]
            .0
            .open_file(source_name)
        {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Ok(mut file) => {
                if secure_fs::hash_file(&mut file, library_core::MAX_INDEXED_FILE_BYTES)
                    .is_ok_and(|fingerprint| fingerprint == expected)
                {
                    true
                } else {
                    return Err(RuntimeError {
                        code: RuntimeErrorCode::Conflict,
                        message: "Original source path is occupied by different bytes".into(),
                        retryable: false,
                        current_version: None,
                        source_holds: Vec::new(),
                    });
                }
            }
            Err(_) => return Err(stale("Original source path is unsafe or unavailable")),
        };
        already_present.push(exists);
        expected_files.push(expected);
    }
    let mut created = Vec::with_capacity(snapshots.len());
    for (index, snapshot) in snapshots.iter().enumerate() {
        if already_present[index] {
            created.push(false);
            continue;
        }
        if !source_directories[index]
            .0
            .matches_path(
                snapshot
                    .root_path
                    .join(&snapshot.relative_path)
                    .parent()
                    .unwrap(),
            )
            .is_ok_and(|matches| matches)
        {
            let (source_holds, hold_failures) = retain_created_quarantine_source_files(
                &source_directories,
                &expected_files,
                &created,
                snapshots,
                snapshot_id,
            );
            if !hold_failures.is_empty() {
                guard.remain_armed();
            }
            return Err(quarantine_undo_cleanup_error(
                "Quarantine destination root changed before catalog recovery",
                source_holds,
                hold_failures,
            ));
        }
        let recovery_name = std::ffi::OsString::from(format!("{}.bin", snapshot.model_id));
        if let Err(copy_error) = secure_fs::copy_verified_into_directory(
            &owned_directory,
            &recovery_name,
            &source_directories[index].0,
            &source_directories[index].1,
            &expected_files[index],
            library_core::MAX_INDEXED_FILE_BYTES,
        ) {
            let (source_holds, hold_failures) = retain_created_quarantine_source_files(
                &source_directories,
                &expected_files,
                &created,
                snapshots,
                snapshot_id,
            );
            if !hold_failures.is_empty() {
                guard.remain_armed();
            }
            return Err(quarantine_undo_cleanup_error(
                &format!("Quarantine recovery copy could not be restored: {copy_error}"),
                source_holds,
                hold_failures,
            ));
        }
        created.push(true);
    }
    if source_directories
        .iter()
        .enumerate()
        .any(|(index, directory)| {
            !directory
                .0
                .matches_path(
                    snapshots[index]
                        .root_path
                        .join(&snapshots[index].relative_path)
                        .parent()
                        .unwrap(),
                )
                .is_ok_and(|matches| matches)
        })
        || !library_directory
            .matches_path(&library_root)
            .is_ok_and(|matches| matches)
    {
        let (source_holds, hold_failures) = retain_created_quarantine_source_files(
            &source_directories,
            &expected_files,
            &created,
            snapshots,
            snapshot_id,
        );
        if !hold_failures.is_empty() {
            guard.remain_armed();
        }
        return Err(quarantine_undo_cleanup_error(
            "Quarantine destination root changed before catalog recovery",
            source_holds,
            hold_failures,
        ));
    }
    let source_preflight_matches =
        source_directories
            .iter()
            .enumerate()
            .all(|(index, directory)| {
                directory
                    .0
                    .open_file(&directory.1)
                    .and_then(|mut file| {
                        secure_fs::hash_file(&mut file, library_core::MAX_INDEXED_FILE_BYTES)
                    })
                    .is_ok_and(|fingerprint| fingerprint == expected_files[index])
            });
    if !source_preflight_matches {
        let (source_holds, hold_failures) = retain_created_quarantine_source_files(
            &source_directories,
            &expected_files,
            &created,
            snapshots,
            snapshot_id,
        );
        if !hold_failures.is_empty() {
            guard.remain_armed();
        }
        return Err(quarantine_undo_cleanup_error(
            "Quarantine source changed before catalog recovery",
            source_holds,
            hold_failures,
        ));
    }
    if !db.catalog_path_matches() {
        let (source_holds, hold_failures) = retain_created_quarantine_source_files(
            &source_directories,
            &expected_files,
            &created,
            snapshots,
            snapshot_id,
        );
        if !hold_failures.is_empty() {
            guard.remain_armed();
        }
        return Err(quarantine_undo_cleanup_error(
            "Library catalog identity changed before quarantine catalog recovery",
            source_holds,
            hold_failures,
        ));
    }
    if let Err(error) = db.restore_removal(snapshot_id) {
        let (source_holds, hold_failures) = retain_created_quarantine_source_files(
            &source_directories,
            &expected_files,
            &created,
            snapshots,
            snapshot_id,
        );
        if !hold_failures.is_empty() {
            guard.remain_armed();
            return Err(quarantine_undo_cleanup_error(
                &format!("Quarantine catalog rollback failed: {error}"),
                source_holds,
                hold_failures,
            ));
        }
        return Err(RuntimeError {
            message: format!(
                "Quarantine catalog rollback failed: {error}; recovery source copies were moved into holds: {}",
                source_hold_summary(&source_holds)
            ),
            source_holds,
            ..runtime_library_error(error)
        });
    }
    if !db.catalog_path_matches() {
        let (source_holds, hold_failures) = retain_created_quarantine_source_files(
            &source_directories,
            &expected_files,
            &created,
            snapshots,
            snapshot_id,
        );
        if !hold_failures.is_empty() {
            guard.remain_armed();
        }
        return Err(quarantine_undo_cleanup_error(
            "Library catalog identity changed during quarantine recovery",
            source_holds,
            hold_failures,
        ));
    }
    let models = snapshots
        .iter()
        .map(|snapshot| MutationVersion {
            model_id: snapshot.model_id.clone(),
            metadata_version: snapshot.metadata_version.saturating_add(1),
        })
        .collect::<Vec<_>>();
    for (index, expected) in expected_files.iter().enumerate() {
        if !source_directories[index]
            .0
            .open_file(&source_directories[index].1)
            .and_then(|mut file| {
                secure_fs::hash_file(&mut file, library_core::MAX_INDEXED_FILE_BYTES)
            })
            .is_ok_and(|fingerprint| fingerprint == *expected)
            || !source_directories[index]
                .0
                .matches_path(
                    snapshots[index]
                        .root_path
                        .join(&snapshots[index].relative_path)
                        .parent()
                        .unwrap(),
                )
                .is_ok_and(|matches| matches)
        {
            guard.invalidate();
            guard.remain_armed();
            return Err(stale(
                "Restored source changed during catalog recovery; the verified recovery copy was retained",
            ));
        }
    }
    trash_directory
        .remove_child_tree(&owned_name)
        .map_err(|_| io_error())?;
    guard.invalidate();
    if models.len() == 1 {
        Ok(ManagementResult::QuarantineUndone {
            model: models[0].clone(),
        })
    } else {
        Ok(ManagementResult::QuarantineUndoneBatch { models })
    }
}

fn execute_move_batch(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    targets: &[MoveLooseSourceTarget],
    destination_handle_id: &str,
    snapshots: &[PhysicalSourceSnapshot],
    expected_epoch: u64,
    expected_revision: u64,
) -> Result<ManagementResult, RuntimeError> {
    require_durable_source_mutations()?;
    runtime.require(context, Capability::CatalogWrite)?;
    runtime.require(context, Capability::PhysicalSourceWrite)?;
    validate_physical_targets(targets.iter().map(|target| target.model_id.as_str()))?;
    validate_physical_source_batch(snapshots)?;
    if targets.len() != snapshots.len()
        || targets.iter().zip(snapshots).any(|(target, snapshot)| {
            target.model_id != snapshot.model_id
                || target.expected_version != snapshot.metadata_version
        })
    {
        return Err(stale("Move plan targets changed"));
    }
    for target in targets {
        validate_output_name(&target.file_name)?;
    }
    let runtime_arc = runtime
        .self_weak
        .get()
        .and_then(Weak::upgrade)
        .ok_or_else(|| internal("Runtime owner is unavailable"))?;
    let guard = runtime_arc.begin_source_mutation_maintenance()?;
    recheck_management_plan(runtime, expected_epoch, expected_revision)?;
    let db = guard.catalog()?;
    #[cfg(test)]
    runtime.run_after_guarded_catalog_open_hook();
    if !db.catalog_path_matches() {
        return Err(stale("Library catalog identity changed during move setup"));
    }
    let source_paths = targets
        .iter()
        .zip(snapshots)
        .map(|(target, snapshot)| {
            revalidate_active_source(
                db,
                runtime,
                context,
                &VersionedTarget {
                    model_id: target.model_id.clone(),
                    expected_version: target.expected_version,
                },
                snapshot,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let destination_root =
        runtime.admitted_root_path(context, destination_handle_id, RootAccess::Writable)?;
    if destination_root.canonicalize().map_err(|_| io_error())? != destination_root {
        return Err(stale("Move destination root identity changed"));
    }
    let destination_directory = secure_fs::Directory::open(&destination_root)
        .map_err(|_| stale("Move destination root is no longer a safe directory"))?;
    if !destination_directory
        .matches_path(&destination_root)
        .map_err(|_| stale("Move destination root identity changed"))?
    {
        return Err(stale("Move destination root identity changed"));
    }
    let source_directories = snapshots
        .iter()
        .map(|snapshot| {
            let source_root = secure_fs::Directory::open(&snapshot.root_path)
                .map_err(|_| stale("Move source root is no longer a safe directory"))?;
            let parent = snapshot
                .relative_path
                .parent()
                .unwrap_or_else(|| Path::new(""));
            source_root
                .open_relative(parent)
                .map_err(|_| stale("Move source parent changed"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let source_names = snapshots
        .iter()
        .map(|snapshot| {
            snapshot
                .relative_path
                .file_name()
                .map(|name| name.to_os_string())
                .ok_or_else(|| invalid("Move source path has no file name"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let destination_catalog_root_id = runtime
        .roots
        .iter()
        .find(|root| root.handle.id == destination_handle_id)
        .and_then(|root| root.catalog_root_id.as_deref())
        .ok_or_else(|| invalid("Move destination is not a catalog root"))?;
    let destination_catalog_root = db
        .list_index_roots()
        .map_err(runtime_library_error)?
        .into_iter()
        .find(|root| root.id == destination_catalog_root_id && root.path == destination_root)
        .ok_or_else(|| stale("Move destination catalog mapping changed"))?;
    let mut destinations = Vec::with_capacity(targets.len());
    let mut destination_set = BTreeSet::new();
    for (target, source_path) in targets.iter().zip(&source_paths) {
        let destination =
            validate_relative_target(&destination_root, Path::new(&target.file_name), false)?;
        if destination == *source_path || !destination_set.insert(destination.clone()) {
            return Err(invalid(
                "Move destinations must be distinct from sources and each other",
            ));
        }
        destinations.push(destination);
    }
    if destinations
        .iter()
        .any(|destination| source_paths.contains(destination))
    {
        return Err(invalid(
            "Move destinations cannot overlap batch source paths",
        ));
    }
    let expected = snapshots
        .iter()
        .map(|snapshot| (snapshot.bytes, snapshot.sha256.clone()))
        .collect::<Vec<_>>();
    let mut copied = Vec::new();
    let mut destination_metadata = Vec::with_capacity(targets.len());
    for (index, fingerprint) in expected.iter().enumerate() {
        if !source_directories[index]
            .matches_path(source_paths[index].parent().unwrap())
            .is_ok_and(|matches| matches)
            || !destination_directory
                .matches_path(&destination_root)
                .is_ok_and(|matches| matches)
        {
            return Err(stale("Move source or destination root identity changed"));
        }
        let destination_name = std::ffi::OsStr::new(&targets[index].file_name);
        let result = secure_fs::copy_verified_into_directory(
            &source_directories[index],
            &source_names[index],
            &destination_directory,
            destination_name,
            fingerprint,
            library_core::MAX_INDEXED_FILE_BYTES,
        );
        match result {
            Ok(metadata) => {
                copied.push((targets[index].file_name.clone(), fingerprint.clone()));
                destination_metadata.push(metadata);
            }
            Err(_) => {
                let cleanup_ok = copied.iter().all(|(name, expected)| {
                    destination_directory
                        .unlink_verified_file(
                            std::ffi::OsStr::new(name),
                            expected,
                            library_core::MAX_INDEXED_FILE_BYTES,
                        )
                        .is_ok()
                });
                if !cleanup_ok {
                    guard.remain_armed();
                    return Err(RuntimeError {
                        code: RuntimeErrorCode::Corrupt,
                        message: "Move copy failed and verified destination copies remain".into(),
                        retryable: false,
                        current_version: None,
                        source_holds: Vec::new(),
                    });
                }
                return Err(io_error());
            }
        }
    }
    let sources_match =
        source_paths
            .iter()
            .zip(&expected)
            .enumerate()
            .all(|(index, (_source, fingerprint))| {
                source_directories[index]
                    .open_file(&source_names[index])
                    .and_then(|mut file| {
                        secure_fs::hash_file(&mut file, library_core::MAX_INDEXED_FILE_BYTES)
                    })
                    .is_ok_and(|actual| actual == *fingerprint)
            });
    let destinations_match = expected.iter().enumerate().all(|(index, fingerprint)| {
        destination_directory
            .open_file(std::ffi::OsStr::new(&targets[index].file_name))
            .and_then(|mut file| {
                secure_fs::hash_file(&mut file, library_core::MAX_INDEXED_FILE_BYTES)
            })
            .is_ok_and(|actual| actual == *fingerprint)
    }) && destination_directory
        .matches_path(&destination_root)
        .is_ok_and(|matches| matches);
    if !sources_match || !destinations_match {
        let cleanup_ok = copied.iter().all(|(name, expected)| {
            destination_directory
                .unlink_verified_file(
                    std::ffi::OsStr::new(name),
                    expected,
                    library_core::MAX_INDEXED_FILE_BYTES,
                )
                .is_ok()
        });
        if !cleanup_ok {
            guard.remain_armed();
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Move preflight changed and verified destination copies remain".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        return Err(stale(
            "Move source or destination changed before catalog update",
        ));
    }
    let core_moves = targets
        .iter()
        .zip(&destination_metadata)
        .map(|(target, metadata)| CoreIndexedModelMove {
            model_id: target.model_id.clone(),
            expected_version: target.expected_version,
            destination_root_id: destination_catalog_root.id.clone(),
            destination_relative_path: PathBuf::from(&target.file_name),
            modified_at: metadata_modified_seconds(metadata),
        })
        .collect::<Vec<_>>();
    if !db.catalog_path_matches() {
        return Err(stale(
            "Library catalog identity changed before move commit; original sources remain active",
        ));
    }
    let moved = match db.move_loose_indexed_models(&core_moves) {
        Ok(moved) => moved,
        Err(error) => {
            let cleanup_ok = copied.iter().all(|(name, expected)| {
                destination_directory
                    .unlink_verified_file(
                        std::ffi::OsStr::new(name),
                        expected,
                        library_core::MAX_INDEXED_FILE_BYTES,
                    )
                    .is_ok()
            });
            if !cleanup_ok {
                guard.remain_armed();
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Corrupt,
                    message:
                        "Move catalog transaction failed and verified destination copies remain"
                            .into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            return Err(runtime_library_error(error));
        }
    };
    if !db.catalog_path_matches() {
        return Err(stale(
            "Library catalog identity changed during move; original sources remain active and verified destination copies were retained",
        ));
    }
    guard.invalidate();
    let all_destinations_verified = expected.iter().enumerate().all(|(index, fingerprint)| {
        destination_directory
            .open_file(std::ffi::OsStr::new(&targets[index].file_name))
            .and_then(|mut file| {
                secure_fs::hash_file(&mut file, library_core::MAX_INDEXED_FILE_BYTES)
            })
            .is_ok_and(|actual| actual == *fingerprint)
    }) && destination_directory
        .matches_path(&destination_root)
        .is_ok_and(|matches| matches);
    if !all_destinations_verified {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: "Move catalog update committed, but a destination changed; original sources were retained".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }
    let mut source_holds = Vec::with_capacity(expected.len());
    for (index, fingerprint) in expected.iter().enumerate() {
        let source_parent_path = source_paths[index].parent().unwrap();
        if !source_directories[index]
            .matches_path(source_parent_path)
            .is_ok_and(|matches| matches)
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: format!(
                    "Move catalog update committed, but a source changed; remaining originals were preserved. Source recovery holds: {}",
                    source_hold_summary(&source_holds)
                ),
                retryable: false,
                current_version: None,
                source_holds: source_holds.clone(),
            });
        }
        let hold_key = format!(
            "move-{}-v{}",
            snapshots[index].model_id, snapshots[index].metadata_version
        );
        let (hold_name, journal_name) = deterministic_source_hold_names(&hold_key)?;
        let hold = match source_directories[index].hold_verified_file_as(
            &source_names[index],
            fingerprint,
            library_core::MAX_INDEXED_FILE_BYTES,
            &hold_name,
            &journal_name,
        ) {
            Ok(hold) => hold,
            Err(error) => {
                if let Some(receipt) = recover_source_hold_receipt(
                    &source_directories[index],
                    &snapshots[index],
                    &journal_name,
                ) {
                    source_holds.push(receipt);
                }
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Corrupt,
                    message: format!(
                        "Move catalog update committed, but source retention was interrupted: {error}. Source recovery holds: {}",
                        source_hold_summary(&source_holds)
                    ),
                    retryable: false,
                    current_version: None,
                    source_holds: source_holds.clone(),
                });
            }
        };
        source_holds.push(source_hold_receipt(&snapshots[index], &hold)?);
    }
    if !db.catalog_path_matches() {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Stale,
            message: format!(
                "Library catalog identity changed during move; source recovery holds: {}",
                source_hold_summary(&source_holds)
            ),
            retryable: false,
            current_version: None,
            source_holds,
        });
    }
    let models = moved
        .iter()
        .cloned()
        .map(mutation_version)
        .collect::<Vec<_>>();
    if models.len() == 1 {
        Ok(ManagementResult::Moved {
            model: models[0].clone(),
            source_holds,
        })
    } else {
        Ok(ManagementResult::MovedBatch {
            models,
            source_holds,
        })
    }
}

fn revalidate_active_source(
    db: &LibraryDb,
    runtime: &LibraryRuntime,
    context: &RequestContext,
    target: &VersionedTarget,
    snapshot: &PhysicalSourceSnapshot,
) -> Result<PathBuf, RuntimeError> {
    let model = db
        .get_model(&target.model_id)
        .map_err(runtime_library_error)?
        .ok_or_else(|| stale("Model was removed before source mutation"))?;
    if model.metadata_version != target.expected_version
        || model.metadata_version != snapshot.metadata_version
        || model.source_root_id.as_deref() != Some(snapshot.catalog_root_id.as_str())
        || model_relative_source_path(&model, &snapshot.root_path)? != snapshot.relative_path
        || model.content_hash.as_deref() != Some(snapshot.sha256.as_str())
        || model.bytes != snapshot.bytes
        || model.source_kind != "indexed"
        || model.source_archive_member.is_some()
    {
        return Err(stale("Indexed source catalog identity changed"));
    }
    let source = db
        .resolve_model_source_ref(&target.model_id)
        .map_err(runtime_library_error)?;
    let root_path =
        runtime.admitted_root_path(context, &snapshot.root_handle_id, RootAccess::Writable)?;
    if root_path != snapshot.root_path
        || source.outer_path != snapshot.root_path.join(&snapshot.relative_path)
        || source.archive_member.is_some()
    {
        return Err(stale("Indexed source root mapping changed"));
    }
    verify_source_snapshot_file(snapshot)
}

fn revalidate_removed_destination(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    model: &Model,
    snapshot: &PhysicalSourceSnapshot,
) -> Result<PathBuf, RuntimeError> {
    if model.source_root_id.as_deref() != Some(snapshot.catalog_root_id.as_str())
        || model_relative_source_path(model, &snapshot.root_path)? != snapshot.relative_path
        || model.content_hash.as_deref() != Some(snapshot.sha256.as_str())
        || model.bytes != snapshot.bytes
    {
        return Err(stale("Quarantine source identity changed"));
    }
    let root_path =
        runtime.admitted_root_path(context, &snapshot.root_handle_id, RootAccess::Writable)?;
    if root_path != snapshot.root_path {
        return Err(stale("Quarantine root mapping changed"));
    }
    validate_relative_target(&snapshot.root_path, &snapshot.relative_path, false)
}

fn execute_backup(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    handle_id: &str,
    file_name: &str,
    expected_epoch: u64,
    expected_revision: u64,
) -> Result<ManagementResult, RuntimeError> {
    runtime.require(context, Capability::Maintenance)?;
    runtime.require_backup_write(context)?;
    runtime.read_db(|_| Ok(()))?;
    let (_, destination) = admitted_file(
        runtime,
        context,
        handle_id,
        RootAccess::BackupOnly,
        file_name,
        false,
    )?;
    let runtime_arc = runtime
        .self_weak
        .get()
        .and_then(Weak::upgrade)
        .ok_or_else(|| internal("Runtime self reference is unavailable"))?;
    let _guard = runtime_arc.begin_maintenance()?;
    recheck_management_plan(runtime, expected_epoch, expected_revision)?;
    let result = create_backup_file(&runtime.root, &destination)
        .map_err(|_| invalid("Library backup could not be created"))?;
    Ok(ManagementResult::BackupCreated {
        destination_handle_id: handle_id.into(),
        file_name: file_name.into(),
        files: result.files,
        bytes: result.bytes,
        sha256: result.sha256,
    })
}

fn execute_purge_catalog(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    backup_handle_id: &str,
    backup_file_name: &str,
    managed_assets: &[ManagedAssetSnapshot],
    expected_epoch: u64,
    expected_revision: u64,
) -> Result<ManagementResult, RuntimeError> {
    runtime.require(context, Capability::CatalogWrite)?;
    runtime.require(context, Capability::Maintenance)?;
    require_durable_source_mutations()?;
    runtime.require(context, Capability::BackupWrite)?;
    let (backup_root, _backup_path) = admitted_file(
        runtime,
        context,
        backup_handle_id,
        RootAccess::BackupOnly,
        backup_file_name,
        false,
    )?;
    let backup_directory = secure_fs::Directory::open(&backup_root)
        .map_err(|_| stale("Backup destination root is no longer a safe directory"))?;
    if !backup_directory
        .matches_path(&backup_root)
        .map_err(|_| stale("Backup destination root identity changed"))?
    {
        return Err(stale("Backup destination root identity changed"));
    }
    let runtime_arc = runtime
        .self_weak
        .get()
        .and_then(Weak::upgrade)
        .ok_or_else(|| internal("Runtime self reference is unavailable"))?;
    let mut guard = runtime_arc.begin_maintenance()?;
    recheck_management_plan(runtime, expected_epoch, expected_revision)?;
    let configured_root = runtime.root.clone();
    let parent = configured_root
        .parent()
        .ok_or_else(|| invalid("Library root has no parent directory"))?
        .to_path_buf();
    let root_name = configured_root
        .file_name()
        .ok_or_else(|| invalid("Library root name is invalid"))?
        .to_os_string();
    let parent_directory = secure_fs::Directory::open(&parent)
        .map_err(|_| stale("Library parent is no longer a safe directory"))?;
    if !parent_directory
        .matches_path(&parent)
        .map_err(|_| stale("Library parent identity changed"))?
    {
        return Err(stale("Library parent identity changed"));
    }
    let root_directory = parent_directory
        .open_child(&root_name)
        .map_err(|_| stale("Library root is no longer a safe directory"))?;
    if !root_directory
        .matches_path(&configured_root)
        .map_err(|_| stale("Library root identity changed"))?
    {
        return Err(stale("Library root identity changed"));
    }
    let root_handle_path = root_directory
        .path()
        .map_err(|_| RuntimeError {
            code: RuntimeErrorCode::Unsupported,
            message: "Pinned Library paths are unavailable on this Unix host".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })?
        .join(".");
    verify_managed_asset_snapshots(&root_handle_path, managed_assets)?;

    let active_db = LibraryDb::open(&root_handle_path).map_err(runtime_library_error)?;
    if active_db.root() != configured_root {
        return Err(stale("Library root identity changed before purge"));
    }
    let expected_model_count = active_db
        .list_models(None, None)
        .map_err(runtime_library_error)?
        .len();
    let expected_roots = active_db
        .list_index_roots()
        .map_err(runtime_library_error)?;
    let root = active_db.root().to_path_buf();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let scratch_backup_directory = create_scratch_directory("purge-backup")?;
    let scratch_backup_parent = scratch_backup_directory.path.clone();
    let scratch_backup = scratch_backup_parent.join(backup_file_name);
    let scratch_backup_result = create_backup_file(&root_handle_path, &scratch_backup)
        .map_err(|_| invalid("Library backup could not be created"))?;
    let (backup_bytes, backup_sha256) = scratch_backup_directory
        .directory
        .hash_file(
            std::ffi::OsStr::new(backup_file_name),
            library_backup::MAX_BACKUP_ARCHIVE_BYTES,
        )
        .map_err(|_| io_error())?;
    if scratch_backup_result.files == 0
        || backup_bytes == 0
        || backup_sha256 != scratch_backup_result.sha256
    {
        return Err(invalid("Library backup failed integrity verification"));
    }
    let scratch_backup_parent =
        secure_fs::Directory::open(&scratch_backup_parent).map_err(|_| io_error())?;
    secure_fs::copy_verified_into_directory(
        &scratch_backup_parent,
        std::ffi::OsStr::new(backup_file_name),
        &backup_directory,
        std::ffi::OsStr::new(backup_file_name),
        &(backup_bytes, backup_sha256.clone()),
        MAX_ORPHAN_CLEANUP_TOTAL_BYTES + 16 * 1024 * 1024,
    )
    .map_err(|_| stale("Backup destination changed before purge"))?;
    let backup_at_destination = backup_directory
        .open_file(std::ffi::OsStr::new(backup_file_name))
        .and_then(|mut file| {
            secure_fs::hash_file(&mut file, MAX_ORPHAN_CLEANUP_TOTAL_BYTES + 16 * 1024 * 1024)
        })
        .map_err(|_| stale("Purge backup changed after creation"))?;
    if backup_at_destination != (backup_bytes, backup_sha256.clone())
        || !backup_directory
            .matches_path(&backup_root)
            .is_ok_and(|matches| matches)
    {
        let _ = backup_directory.unlink_verified_file(
            std::ffi::OsStr::new(backup_file_name),
            &(backup_bytes, backup_sha256.clone()),
            MAX_ORPHAN_CLEANUP_TOTAL_BYTES + 16 * 1024 * 1024,
        );
        return Err(stale("Backup destination root identity changed"));
    }
    // Read the generated archive back into isolated staging and open its database
    // before replacing the active catalog. A backup that cannot be restored never
    // authorizes the purge.
    let verification_temp = create_scratch_directory("purge-verify")?;
    let verification_name = std::ffi::OsString::from("restored-library");
    verification_temp
        .directory
        .create_child_directory(&verification_name)
        .map_err(|_| io_error())?;
    let verification_directory = verification_temp
        .directory
        .open_child(&verification_name)
        .map_err(|_| io_error())?;
    let mut verification_target = GuardedDirectory(verification_directory);
    let verification_archive = scratch_backup_directory
        .directory
        .open_file(std::ffi::OsStr::new(backup_file_name))
        .map_err(|_| stale("Purge verification archive is unavailable"))?;
    library_backup::restore_backup_from_reader(verification_archive, &mut verification_target)
        .map_err(|_| invalid("Library backup failed restore verification"))?;
    let verification_root = verification_target.io_path().map_err(|_| io_error())?;
    let verified_db = LibraryDb::open(&verification_root)
        .map_err(|_| invalid("Library backup database failed verification"))?;
    if verified_db
        .list_models(None, None)
        .map_err(|_| invalid("Library backup database failed verification"))?
        .len()
        != expected_model_count
    {
        return Err(invalid(
            "Library backup model count does not match the catalog",
        ));
    }
    drop(verified_db);
    drop(verification_temp);
    verify_managed_asset_snapshots(&root_handle_path, managed_assets)?;

    let root_name_text = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("Library root name is invalid"))?;
    let previous_name =
        std::ffi::OsString::from(format!("{root_name_text}.previous-purge-{nonce}"));
    let staged_name = std::ffi::OsString::from(format!(".gogglelab-empty-{nonce}"));
    if parent_directory
        .entry_exists(&previous_name)
        .map_err(|_| io_error())?
        || parent_directory
            .entry_exists(&staged_name)
            .map_err(|_| io_error())?
    {
        return Err(stale("Purge staging or recovery path is already occupied"));
    }
    let journal = PurgeJournal {
        version: 1,
        root_path: root
            .to_str()
            .ok_or_else(|| invalid("Purge requires a Unicode Library path"))?
            .to_owned(),
        nonce: nonce.clone(),
        staged_name: staged_name.to_string_lossy().into_owned(),
        previous_name: previous_name.to_string_lossy().into_owned(),
        committed: false,
    };
    let (_journal_path, mut journal_name) =
        write_purge_journal_at(&parent_directory, &root, &journal)?;
    let journal_guard = PurgeJournalGuard::new(
        parent_directory.try_clone().map_err(|_| io_error())?,
        &mut journal_name,
    );

    let mut staged_temp =
        OwnedDirectory::create_at(&parent_directory, &staged_name.to_string_lossy())?;
    let staged_directory = parent_directory
        .open_child(&staged_name)
        .map_err(|_| io_error())?;
    let template_temp = create_scratch_directory("purge-empty-library")?;
    let template_root = template_temp.path.clone();
    let staged_db = LibraryDb::open(&template_root).map_err(runtime_library_error)?;
    staged_db
        .copy_catalog_configuration_from(&active_db)
        .map_err(runtime_library_error)?;
    let staged_models = staged_db
        .list_models(None, None)
        .map_err(runtime_library_error)?;
    let staged_roots = staged_db
        .list_index_roots()
        .map_err(runtime_library_error)?;
    if !staged_models.is_empty()
        || staged_roots.len() != expected_roots.len()
        || staged_roots.iter().any(|root| {
            !expected_roots.iter().any(|expected| {
                root.id == expected.id
                    && root.path == expected.path
                    && root.enabled == expected.enabled
                    && root.scan_interval_minutes == expected.scan_interval_minutes
                    && root.max_file_bytes == expected.max_file_bytes
                    && root.scan_zip == expected.scan_zip
            })
        })
    {
        return Err(invalid(
            "Purged Library staging configuration failed validation",
        ));
    }
    drop(staged_db);
    for name in ["originals", "manuals", "thumbnails", "staging", "trash"] {
        staged_directory
            .create_child_directory(std::ffi::OsStr::new(name))
            .map_err(|_| io_error())?;
    }
    let template_directory =
        secure_fs::Directory::open(&template_root).map_err(|error| RuntimeError {
            code: RuntimeErrorCode::Io,
            message: format!("Purge staging template directory could not be opened: {error}"),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })?;
    let (template_bytes, template_hash) = secure_fs::hash_file(
        &mut template_directory
            .open_file(std::ffi::OsStr::new("library.sqlite3"))
            .map_err(|_| io_error())?,
        16 * 1024 * 1024,
    )
    .map_err(|_| io_error())?;
    secure_fs::copy_verified_into_directory(
        &template_directory,
        std::ffi::OsStr::new("library.sqlite3"),
        &staged_directory,
        std::ffi::OsStr::new("library.sqlite3"),
        &(template_bytes, template_hash),
        16 * 1024 * 1024,
    )
    .map_err(|_| io_error())?;
    let mut marker_receipt = write_purge_stage_marker_at(&staged_directory, &root, &nonce)?;
    drop(active_db);
    if !root_directory
        .matches_path(&configured_root)
        .is_ok_and(|matches| matches)
        || !backup_directory
            .matches_path(&backup_root)
            .is_ok_and(|matches| matches)
    {
        return Err(stale(
            "Library or backup destination identity changed before purge",
        ));
    }
    verify_managed_asset_snapshots(&root_handle_path, managed_assets)?;

    let epoch = guard.invalidate();
    parent_directory
        .rename_child_exclusive(&root_name, &previous_name)
        .map_err(|_| stale("Library root changed before purge"))?;
    journal_guard.disarm();
    if parent_directory.sync().is_err() {
        guard.remain_armed();
        return Err(io_error());
    }
    if parent_directory
        .rename_child_exclusive(&staged_name, &root_name)
        .is_err()
    {
        if parent_directory
            .rename_child_exclusive(&previous_name, &root_name)
            .is_err()
        {
            guard.remain_armed();
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Could not install the empty Library or restore the previous Library"
                    .into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        if parent_directory.sync().is_err() {
            guard.remain_armed();
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Purge failed and the previous Library was restored, but the recovery journal could not be committed".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        if let Err(error) = clear_purge_journal_at(&parent_directory, &mut journal_name) {
            guard.remain_armed();
            return Err(error);
        }
        return Err(io_error());
    }
    staged_temp.disarm();
    if parent_directory.sync().is_err() {
        guard.remain_armed();
        return Err(io_error());
    }

    let validation = LibraryDb::open(
        staged_directory
            .path()
            .map_err(|_| invalid("Pinned staged Library path is unavailable"))?
            .join("."),
    )
    .and_then(|db| {
        let models = db.list_models(None, None)?;
        let roots = db.list_index_roots()?;
        if !models.is_empty()
            || roots.len() != expected_roots.len()
            || roots.iter().any(|root| {
                !expected_roots.iter().any(|expected| {
                    root.id == expected.id
                        && root.path == expected.path
                        && root.enabled == expected.enabled
                        && root.scan_interval_minutes == expected.scan_interval_minutes
                        && root.max_file_bytes == expected.max_file_bytes
                        && root.scan_zip == expected.scan_zip
                })
            })
        {
            return Err(LibraryError::InvalidInput(
                "Purged Library failed validation".into(),
            ));
        }
        Ok(())
    });
    if validation.is_err()
        || !staged_directory
            .matches_path(&configured_root)
            .is_ok_and(|matches| matches)
    {
        let failed_name = std::ffi::OsString::from(format!(".gogglelab-failed-purge-{nonce}"));
        if parent_directory
            .rename_child_exclusive(&root_name, &failed_name)
            .is_err()
            || parent_directory.sync().is_err()
        {
            guard.remain_armed();
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Purged Library failed validation; recovery data remains under the pinned parent".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        if parent_directory
            .rename_child_exclusive(&previous_name, &root_name)
            .is_err()
            || parent_directory.sync().is_err()
        {
            guard.remain_armed();
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Purged Library failed validation; recovery data remains under the pinned parent".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        if let Err(error) = clear_purge_journal_at(&parent_directory, &mut journal_name) {
            guard.remain_armed();
            return Err(error);
        }
        return Err(invalid(
            "Purged Library failed validation and was rolled back",
        ));
    }

    finish_catalog_purge_commit(
        &mut guard,
        &staged_directory,
        &parent_directory,
        &root,
        &mut journal_name,
        &nonce,
        |directory, _| {
            GuardedDirectory(directory.try_clone().map_err(|_| io_error())?)
                .delete_generated_file(&mut marker_receipt)
                .map_err(generated_runtime_error)
        },
        clear_purge_journal_at,
    )?;

    Ok(ManagementResult::Purged {
        backup_destination_handle_id: backup_handle_id.into(),
        backup_file_name: backup_file_name.into(),
        library_epoch: epoch,
    })
}

fn execute_selection_export(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    targets: &[VersionedTarget],
    handle_id: &str,
    file_name: &str,
    expected_epoch: u64,
    expected_revision: u64,
) -> Result<ManagementResult, RuntimeError> {
    runtime.require(context, Capability::Maintenance)?;
    runtime.require_backup_write(context)?;
    let (_, destination) = admitted_file(
        runtime,
        context,
        handle_id,
        RootAccess::BackupOnly,
        file_name,
        false,
    )?;
    let runtime_arc = runtime
        .self_weak
        .get()
        .and_then(Weak::upgrade)
        .ok_or_else(|| internal("Runtime self reference is unavailable"))?;
    let _guard = runtime_arc.begin_maintenance()?;
    recheck_management_plan(runtime, expected_epoch, expected_revision)?;
    let ids = targets
        .iter()
        .map(|target| target.model_id.clone())
        .collect::<Vec<_>>();
    let manifest = library_recovery::export_library_zip(&runtime.root, &ids, &destination)
        .map_err(|_| invalid("Recovery export could not be created"))?;
    let bytes = fs::symlink_metadata(&destination)
        .map_err(|_| io_error())?
        .len();
    Ok(ManagementResult::SelectionExported {
        destination_handle_id: handle_id.into(),
        file_name: file_name.into(),
        models: manifest.models.len(),
        bytes,
    })
}

fn resolve_library_export_destination(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    dest_path: Option<&str>,
) -> Result<(String, String), RuntimeError> {
    let roots = runtime
        .roots
        .iter()
        .filter(|root| root.handle.access == RootAccess::BackupOnly)
        .collect::<Vec<_>>();
    if roots.is_empty() {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Forbidden,
            message: "A backup-only destination is required for library export".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }

    let destination = dest_path.map(str::trim).filter(|value| !value.is_empty());
    let (root, file_name) = match destination {
        None => {
            if roots.len() != 1 {
                return Err(invalid(
                    "destPath is required when multiple backup-only roots are configured",
                ));
            }
            let timestamp = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| internal("Export timestamp is invalid"))?
                .as_millis();
            (
                roots[0],
                format!(
                    "printventory-library-{timestamp}-{}.json",
                    uuid::Uuid::new_v4().simple()
                ),
            )
        }
        Some(value) if Path::new(value).is_absolute() => {
            let requested = Path::new(value);
            let file_name = requested
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| invalid("destPath must name a JSON file"))?;
            let parent = requested
                .parent()
                .ok_or_else(|| invalid("destPath must be a file inside a backup root"))?
                .canonicalize()
                .map_err(|_| io_error())?;
            let root = roots
                .iter()
                .copied()
                .find(|root| root.path == parent)
                .ok_or_else(|| RuntimeError {
                    code: RuntimeErrorCode::Forbidden,
                    message: "destPath must be directly inside an admitted backup-only root".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                })?;
            (root, file_name.to_owned())
        }
        Some(value) if value.contains('/') => {
            let (handle_id, file_name) = value
                .split_once('/')
                .ok_or_else(|| invalid("destPath is invalid"))?;
            if file_name.contains('/') || file_name.contains('\\') {
                return Err(invalid(
                    "destPath must name a file directly under a backup root",
                ));
            }
            let root = roots
                .iter()
                .copied()
                .find(|root| root.handle.id == handle_id)
                .ok_or_else(|| RuntimeError {
                    code: RuntimeErrorCode::Forbidden,
                    message: "destPath backup handle is not admitted".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                })?;
            (root, file_name.to_owned())
        }
        Some(value) => {
            if roots.len() != 1 {
                return Err(invalid(
                    "destPath must include a backup handle when multiple backup-only roots are configured",
                ));
            }
            (roots[0], value.to_owned())
        }
    };

    validate_output_name(&file_name)?;
    runtime.admitted_root_path(context, &root.handle.id, RootAccess::BackupOnly)?;
    Ok((root.handle.id.clone(), file_name))
}

fn export_filament_value(filament: &CoreFilament) -> serde_json::Value {
    let source = match &filament.origin {
        CoreFilamentOrigin::Manual => "manual",
        CoreFilamentOrigin::Spoolman => "spoolman",
    };
    serde_json::json!({
        "name": filament.name,
        "vendor": filament.brand,
        "material": filament.material,
        "color_hex": filament.color,
        "diameter": null,
        "spoolman_id": filament.external_id,
        "source": source
    })
}

fn export_filaments(
    filament_ids: &[String],
    filaments: &HashMap<String, serde_json::Value>,
) -> Result<Vec<serde_json::Value>, LibraryError> {
    filament_ids
        .iter()
        .map(|id| {
            filaments.get(id).cloned().ok_or_else(|| {
                LibraryError::InvalidInput(
                    "Library JSON export cannot preserve an unresolved filament reference".into(),
                )
            })
        })
        .collect()
}

fn export_print_status(status: &CorePrintStatus) -> &'static str {
    match status {
        CorePrintStatus::Unprinted => "unprinted",
        CorePrintStatus::Want => "want",
        CorePrintStatus::Queued => "queued",
        CorePrintStatus::Printing => "printing",
        CorePrintStatus::Printed => "printed",
        CorePrintStatus::Failed => "failed",
    }
}

fn export_print_outcome(outcome: &CorePrintOutcome) -> &'static str {
    match outcome {
        CorePrintOutcome::Printed => "printed",
        CorePrintOutcome::Failed => "failed",
        CorePrintOutcome::Cancelled => "cancelled",
    }
}

fn export_part_stock_value(part: &CorePartStock) -> serde_json::Value {
    serde_json::json!({
        "gogglelab_part_id": part.id,
        "name": part.name,
        "category": part.category,
        "quantity": part.quantity,
        "unit": part.unit,
        "notes": part.notes,
        "low_stock": part.low_stock,
    })
}

fn export_print_attempt_parts(parts: &[CorePrintAttemptPart]) -> Vec<serde_json::Value> {
    parts
        .iter()
        .map(|part| {
            serde_json::json!({
                "id": part.id,
                "name": part.name,
                "category": part.category,
                "unit": part.unit,
                "quantity": part.quantity,
                "quantityPerCopy": part.quantity_per_copy,
            })
        })
        .collect()
}

fn export_rfc3339_timestamp(value: &str, field: &'static str) -> Result<String, LibraryError> {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    if let Ok(seconds) = value.parse::<i64>() {
        return OffsetDateTime::from_unix_timestamp(seconds)
            .map_err(|_| {
                LibraryError::InvalidInput(format!(
                    "{field} timestamp is outside the supported range"
                ))
            })?
            .format(&Rfc3339)
            .map_err(|_| {
                LibraryError::InvalidInput(format!("{field} timestamp could not be formatted"))
            });
    }

    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| LibraryError::InvalidInput(format!("{field} timestamp is invalid")))?;
    Ok(value.to_owned())
}

const MAX_PRINTVENTORY_JSON_BYTES: usize = 25 * 1024 * 1024;
const PRINTVENTORY_JSON_PAGE_ROWS: u16 = 50;
const PRINTVENTORY_JSON_MODEL_PAGE_ROWS: u16 = 1;

fn export_printer_value(
    printer: &library_core::models::Printer,
) -> Result<serde_json::Value, LibraryError> {
    let web_url = library_core::normalize_printer_web_url(printer.web_url.as_deref())?;
    let created_at = export_rfc3339_timestamp(&printer.created_at, "Printer creation")?;
    let updated_at = export_rfc3339_timestamp(&printer.updated_at, "Printer update")?;
    Ok(serde_json::json!({
        "gogglelab_printer_id": printer.id.as_str(),
        "nickname": printer.nickname.as_str(),
        "manufacturer": printer.make.as_deref(),
        "model": printer.model.as_deref(),
        "printer_type": printer.printer_type.as_deref(),
        "firmware_type": printer.firmware.as_deref(),
        "host": printer.host.as_deref(),
        "web_url": web_url,
        "bed_mm": printer.bed_mm,
        "notes": printer.notes.as_str(),
        "source_kind": export_printer_source_kind(printer.source_kind),
        "created_at": created_at,
        "updated_at": updated_at,
    }))
}

fn export_printer_source_kind(source: library_core::models::PrinterSourceKind) -> &'static str {
    match source {
        library_core::models::PrinterSourceKind::Manual => "manual",
        library_core::models::PrinterSourceKind::Discovered => "discovered",
        library_core::models::PrinterSourceKind::Imported => "imported",
    }
}

fn export_printer_reminder_status(
    status: library_core::models::PrinterReminderStatus,
) -> &'static str {
    match status {
        library_core::models::PrinterReminderStatus::Active => "active",
        library_core::models::PrinterReminderStatus::Paused => "paused",
        library_core::models::PrinterReminderStatus::Completed => "completed",
    }
}

/// Portable slicer metadata. Local executable identity and trust are deliberately
/// excluded: an importing workstation must select and trust its own executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlicerJsonExportEntry {
    pub name: String,
    pub path: String,
}

fn validate_slicer_json_export(slicers: &[SlicerJsonExportEntry]) -> Result<(), LibraryError> {
    if slicers.len() > 64 {
        return Err(LibraryError::InvalidInput(
            "At most 64 slicer profiles can be exported".into(),
        ));
    }
    let mut names = std::collections::HashSet::new();
    let mut paths = std::collections::HashSet::new();
    for slicer in slicers {
        if slicer.name.trim().is_empty()
            || slicer.name.chars().count() > 128
            || slicer.name.chars().any(char::is_control)
            || slicer.path.trim().is_empty()
            || slicer.path.chars().count() > 4096
            || slicer.path.chars().any(char::is_control)
        {
            return Err(LibraryError::InvalidInput("Slicer export requires bounded, non-empty names and paths without control characters".into()));
        }
        if !names.insert(slicer.name.trim().to_lowercase())
            || !paths.insert(
                slicer
                    .path
                    .trim()
                    .replace('\\', "/")
                    .trim_end_matches('/')
                    .to_lowercase(),
            )
        {
            return Err(LibraryError::InvalidInput(
                "Slicer export contains duplicate names or paths".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
fn build_library_export_value(
    db: &LibraryDb,
    export_date: &str,
) -> Result<(serde_json::Value, usize), LibraryError> {
    build_library_export_value_with_slicers(db, export_date, &[])
}

fn build_library_export_value_with_slicers(
    db: &LibraryDb,
    export_date: &str,
    slicers: &[SlicerJsonExportEntry],
) -> Result<(serde_json::Value, usize), LibraryError> {
    validate_slicer_json_export(slicers)?;
    let models = db.list_models_for_json_export()?;
    let model_count = models.len();
    let parts = db.list_part_stock()?;
    let model_names = models
        .iter()
        .map(|model| (model.id.clone(), model.display_name.clone()))
        .collect::<HashMap<_, _>>();
    let tag_categories = db
        .list_tags()?
        .into_iter()
        .map(|tag| (tag.name, tag.category))
        .collect::<HashMap<_, _>>();
    let filaments = db
        .list_filaments(true)?
        .into_iter()
        .map(|filament| (filament.id.clone(), export_filament_value(&filament)))
        .collect::<HashMap<_, _>>();
    let printer_snapshot = db.export_printer_data_snapshot()?;
    let printers = printer_snapshot.printers;
    let printers_by_id = printers
        .iter()
        .map(|printer| (printer.id.as_str(), printer))
        .collect::<HashMap<_, _>>();
    let exported_printers = printers
        .iter()
        .map(export_printer_value)
        .collect::<Result<Vec<_>, _>>()?;
    let exported_maintenance_logs = printer_snapshot
        .maintenance_logs
        .iter()
        .map(|log| {
            let created_at = export_rfc3339_timestamp(&log.created_at, "Maintenance log creation")?;
            Ok(serde_json::json!({
                "gogglelab_maintenance_log_id": log.id.as_str(),
                "gogglelab_printer_id": log.printer_id.as_deref(),
                "printer_nickname": log.printer_nickname.as_str(),
                "title": log.title.as_str(),
                "notes": log.notes.as_str(),
                "performed_at": log.performed_at.as_str(),
                "created_at": created_at,
            }))
        })
        .collect::<Result<Vec<_>, LibraryError>>()?;
    let exported_maintenance_reminders = printer_snapshot
        .reminders
        .iter()
        .map(|reminder| {
            let created_at = export_rfc3339_timestamp(&reminder.created_at, "Reminder creation")?;
            let updated_at = export_rfc3339_timestamp(&reminder.updated_at, "Reminder update")?;
            Ok(serde_json::json!({
                "gogglelab_printer_reminder_id": reminder.id.as_str(),
                "gogglelab_printer_id": reminder.printer_id.as_deref(),
                "printer_nickname": reminder.printer_nickname.as_str(),
                "title": reminder.title.as_str(),
                "notes": reminder.notes.as_str(),
                "due_at": reminder.due_at.as_str(),
                "interval_days": reminder.interval_days,
                "status": export_printer_reminder_status(reminder.status),
                "last_completed_at": reminder.last_completed_at.as_deref(),
                "created_at": created_at,
                "updated_at": updated_at,
            }))
        })
        .collect::<Result<Vec<_>, LibraryError>>()?;
    let mut exported_models = Vec::with_capacity(model_count);

    for model in models {
        let mut print_events = Vec::new();
        for attempt in db.list_print_attempts(&model.id)? {
            let attempt_filaments = export_filaments(&attempt.filament_ids, &filaments)?;
            let created_at = export_rfc3339_timestamp(&attempt.created_at, "Print event creation")?;
            let printer = attempt
                .printer_id
                .as_deref()
                .and_then(|id| printers_by_id.get(id).copied());
            let printer_nickname = attempt
                .printer_name
                .clone()
                .or_else(|| printer.map(|record| record.nickname.clone()));
            let printer_manufacturer = printer.and_then(|record| record.make.clone());
            let printer_model = printer.and_then(|record| record.model.clone());
            let printer_type = printer.and_then(|record| record.printer_type.clone());
            let gogglelab_printer_id = attempt.printer_id.clone();
            print_events.push(serde_json::json!({
                "printed_at": attempt.printed_at,
                "outcome": export_print_outcome(&attempt.outcome),
                "quantity": attempt.quantity,
                "notes": attempt.notes,
                "created_at": created_at,
                "printer_nickname": printer_nickname,
                "printer_manufacturer": printer_manufacturer,
                "printer_model": printer_model,
                "printer_type": printer_type,
                "gogglelab_printer_id": gogglelab_printer_id,
                "filaments": attempt_filaments,
                "parts": export_print_attempt_parts(&attempt.parts)
            }));
        }
        let model_filaments = export_filaments(&model.filament_ids, &filaments)?;
        let file_path = model
            .source_path
            .as_deref()
            .or(model.storage_path.as_deref())
            .unwrap_or_default();
        let print_status = export_print_status(&model.metadata.print_status);
        let parent_model = model
            .metadata
            .parent_model_id
            .as_ref()
            .and_then(|id| model_names.get(id))
            .map(String::as_str);
        let categorized_tags = model
            .tags
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "category": tag_categories.get(name).cloned().flatten()
                })
            })
            .collect::<Vec<_>>();
        exported_models.push(serde_json::json!({
            "filePath": file_path,
            "fileName": model.original_name.as_deref().unwrap_or(&model.display_name),
            "designer": model.metadata.designer,
            "source": model.metadata.source_url,
            "notes": model.metadata.notes,
            "printed": model.metadata.print_count > 0 || print_status == "printed",
            "print_status": print_status,
            "print_count": model.metadata.print_count,
            "last_printed_at": model.metadata.last_printed_at,
            "parentModel": parent_model,
            "gogglelabParentModelId": model.metadata.parent_model_id,
            "license": model.metadata.model_license,
            "rating": model.metadata.rating,
            "favorite": model.metadata.favorite,
            "tags": model.tags,
            "gogglelabTags": categorized_tags,
            "filaments": model_filaments,
            "printEvents": print_events
        }));
    }

    Ok((
        serde_json::json!({
            "version": "1.0",
            "exportDate": export_date,
            "printers": exported_printers,
            "printer_maintenance_logs": exported_maintenance_logs,
            "printer_maintenance_reminders": exported_maintenance_reminders,
            "parts": parts.iter().map(export_part_stock_value).collect::<Vec<_>>(),
            "slicers": slicers,
            "models": exported_models
        }),
        model_count,
    ))
}

struct CappedJsonWriter<W> {
    inner: W,
    limit: usize,
    bytes_written: usize,
    exceeded: bool,
}

impl<W> CappedJsonWriter<W> {
    fn new(inner: W, limit: usize) -> Self {
        Self {
            inner,
            limit,
            bytes_written: 0,
            exceeded: false,
        }
    }

    fn bytes_written(&self) -> usize {
        self.bytes_written
    }

    fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for CappedJsonWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if buffer.len() > self.limit.saturating_sub(self.bytes_written) {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Printventory JSON byte limit exceeded",
            ));
        }
        let written = self.inner.write(buffer)?;
        self.bytes_written += written;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn library_json_export_size_error(limit: usize) -> LibraryError {
    if limit == MAX_PRINTVENTORY_JSON_BYTES {
        LibraryError::InvalidInput(
            "Library JSON export exceeds the 25 MiB Printventory limit".into(),
        )
    } else {
        LibraryError::InvalidInput(format!(
            "Library JSON export exceeds its {limit}-byte limit"
        ))
    }
}

fn library_json_export_writer_error<W>(writer: &CappedJsonWriter<W>) -> LibraryError {
    if writer.exceeded {
        library_json_export_size_error(writer.limit)
    } else {
        LibraryError::InvalidInput("Library JSON export could not be saved".into())
    }
}

struct JsonExportContext<'a> {
    db: &'a LibraryDb,
    failure: std::cell::RefCell<Option<LibraryError>>,
    model_count: Cell<usize>,
    parts: Vec<CorePartStock>,
    printer_counts: library_core::PrinterDataExportCounts,
    printers_by_id: HashMap<String, library_core::PrinterExportAttribution>,
}

impl<'a> JsonExportContext<'a> {
    fn new(db: &'a LibraryDb) -> Result<Self, LibraryError> {
        let printer_counts = db.printer_data_export_counts()?;
        let printers = db.list_printer_export_attributions()?;
        if printers.len() != printer_counts.printers {
            return Err(LibraryError::InvalidInput(
                "printer export snapshot did not read every counted profile".into(),
            ));
        }
        let printers_by_id = printers
            .into_iter()
            .map(|printer| (printer.id.clone(), printer))
            .collect();
        let parts = db.list_part_stock()?;
        Ok(Self {
            db,
            failure: std::cell::RefCell::new(None),
            model_count: Cell::new(0),
            parts,
            printer_counts,
            printers_by_id,
        })
    }

    fn serialization_failure<E: serde::ser::Error>(&self, error: LibraryError) -> E {
        let mut failure = self.failure.borrow_mut();
        if failure.is_none() {
            *failure = Some(error);
        }
        E::custom("Printventory export could not be serialized")
    }

    fn serialize_result<T, E: serde::ser::Error>(
        &self,
        result: Result<T, LibraryError>,
    ) -> Result<T, E> {
        result.map_err(|error| self.serialization_failure(error))
    }
}

struct LibraryJsonExportView<'a> {
    context: &'a JsonExportContext<'a>,
    export_date: &'a str,
    slicers: &'a [SlicerJsonExportEntry],
}

impl Serialize for LibraryJsonExportView<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        let context = self.context;
        let mut map = serializer.serialize_map(Some(8))?;
        map.serialize_entry("exportDate", self.export_date)?;
        map.serialize_entry("models", &ExportModels { context })?;
        map.serialize_entry("parts", &ExportPartStocks { context })?;
        map.serialize_entry(
            "printer_maintenance_logs",
            &ExportMaintenanceLogs { context },
        )?;
        map.serialize_entry(
            "printer_maintenance_reminders",
            &ExportReminders { context },
        )?;
        map.serialize_entry("printers", &ExportPrinters { context })?;
        map.serialize_entry("slicers", self.slicers)?;
        map.serialize_entry("version", "1.0")?;
        map.end()
    }
}

struct ExportPartStocks<'a> {
    context: &'a JsonExportContext<'a>,
}

impl Serialize for ExportPartStocks<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;

        let mut sequence = serializer.serialize_seq(Some(self.context.parts.len()))?;
        for part in &self.context.parts {
            sequence.serialize_element(&export_part_stock_value(part))?;
        }
        sequence.end()
    }
}

struct ExportPrinters<'a> {
    context: &'a JsonExportContext<'a>,
}

impl Serialize for ExportPrinters<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;

        let expected = self.context.printer_counts.printers;
        let mut sequence = serializer.serialize_seq(Some(expected))?;
        let mut after_id: Option<String> = None;
        let mut rows = 0usize;
        loop {
            let page = self.context.serialize_result::<_, S::Error>(
                self.context.db.list_printers_for_json_export_page(
                    after_id.as_deref(),
                    PRINTVENTORY_JSON_PAGE_ROWS,
                ),
            )?;
            if page.is_empty() {
                break;
            }
            for printer in &page {
                let value = self
                    .context
                    .serialize_result::<_, S::Error>(export_printer_value(printer))?;
                sequence.serialize_element(&value)?;
            }
            rows += page.len();
            after_id = page.last().map(|printer| printer.id.clone());
            if rows > expected {
                return Err(self
                    .context
                    .serialization_failure(LibraryError::InvalidInput(
                        "printer export read more profiles than counted".into(),
                    )));
            }
        }
        if rows != expected {
            return Err(self
                .context
                .serialization_failure(LibraryError::InvalidInput(
                    "printer export did not read every counted profile".into(),
                )));
        }
        sequence.end()
    }
}

struct ExportMaintenanceLogs<'a> {
    context: &'a JsonExportContext<'a>,
}

impl Serialize for ExportMaintenanceLogs<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;

        let expected = self.context.printer_counts.maintenance_logs;
        let mut sequence = serializer.serialize_seq(Some(expected))?;
        let mut after_id: Option<String> = None;
        let mut rows = 0usize;
        loop {
            let page = self.context.serialize_result::<_, S::Error>(
                self.context
                    .db
                    .list_printer_maintenance_logs_for_json_export_page(
                        after_id.as_deref(),
                        PRINTVENTORY_JSON_PAGE_ROWS,
                    ),
            )?;
            if page.is_empty() {
                break;
            }
            for log in &page {
                let created_at =
                    self.context
                        .serialize_result::<_, S::Error>(export_rfc3339_timestamp(
                            &log.created_at,
                            "Maintenance log creation",
                        ))?;
                let value = serde_json::json!({
                    "gogglelab_maintenance_log_id": log.id,
                    "gogglelab_printer_id": log.printer_id,
                    "printer_nickname": log.printer_nickname,
                    "title": log.title,
                    "notes": log.notes,
                    "performed_at": log.performed_at,
                    "created_at": created_at,
                });
                sequence.serialize_element(&value)?;
            }
            rows += page.len();
            after_id = page.last().map(|log| log.id.clone());
            if rows > expected {
                return Err(self
                    .context
                    .serialization_failure(LibraryError::InvalidInput(
                        "printer export read more maintenance logs than counted".into(),
                    )));
            }
        }
        if rows != expected {
            return Err(self
                .context
                .serialization_failure(LibraryError::InvalidInput(
                    "printer export did not read every counted maintenance log".into(),
                )));
        }
        sequence.end()
    }
}

struct ExportReminders<'a> {
    context: &'a JsonExportContext<'a>,
}

impl Serialize for ExportReminders<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;

        let expected = self.context.printer_counts.reminders;
        let mut sequence = serializer.serialize_seq(Some(expected))?;
        let mut after_id: Option<String> = None;
        let mut rows = 0usize;
        loop {
            let page = self.context.serialize_result::<_, S::Error>(
                self.context.db.list_printer_reminders_for_json_export_page(
                    after_id.as_deref(),
                    PRINTVENTORY_JSON_PAGE_ROWS,
                ),
            )?;
            if page.is_empty() {
                break;
            }
            for reminder in &page {
                let created_at =
                    self.context
                        .serialize_result::<_, S::Error>(export_rfc3339_timestamp(
                            &reminder.created_at,
                            "Reminder creation",
                        ))?;
                let updated_at =
                    self.context
                        .serialize_result::<_, S::Error>(export_rfc3339_timestamp(
                            &reminder.updated_at,
                            "Reminder update",
                        ))?;
                let value = serde_json::json!({
                    "gogglelab_printer_reminder_id": reminder.id,
                    "gogglelab_printer_id": reminder.printer_id,
                    "printer_nickname": reminder.printer_nickname,
                    "title": reminder.title,
                    "notes": reminder.notes,
                    "due_at": reminder.due_at,
                    "interval_days": reminder.interval_days,
                    "status": export_printer_reminder_status(reminder.status),
                    "last_completed_at": reminder.last_completed_at,
                    "created_at": created_at,
                    "updated_at": updated_at,
                });
                sequence.serialize_element(&value)?;
            }
            rows += page.len();
            after_id = page.last().map(|reminder| reminder.id.clone());
            if rows > expected {
                return Err(self
                    .context
                    .serialization_failure(LibraryError::InvalidInput(
                        "printer export read more reminders than counted".into(),
                    )));
            }
        }
        if rows != expected {
            return Err(self
                .context
                .serialization_failure(LibraryError::InvalidInput(
                    "printer export did not read every counted reminder".into(),
                )));
        }
        sequence.end()
    }
}

struct ExportModels<'a> {
    context: &'a JsonExportContext<'a>,
}

impl Serialize for ExportModels<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;

        let expected = self
            .context
            .serialize_result::<_, S::Error>(self.context.db.count_models_for_json_export())?;
        self.context.model_count.set(expected);
        let mut sequence = serializer.serialize_seq(Some(expected))?;
        let mut rows = 0usize;
        while rows < expected {
            let page = self.context.serialize_result::<_, S::Error>(
                self.context
                    .db
                    .list_models_for_json_export_page(rows, PRINTVENTORY_JSON_MODEL_PAGE_ROWS),
            )?;
            if page.is_empty() {
                return Err(self
                    .context
                    .serialization_failure(LibraryError::InvalidInput(
                        "JSON export did not read every counted model".into(),
                    )));
            }
            for model in &page {
                sequence.serialize_element(&ExportModel {
                    context: self.context,
                    model,
                })?;
            }
            rows += page.len();
            if rows > expected {
                return Err(self
                    .context
                    .serialization_failure(LibraryError::InvalidInput(
                        "JSON export read more models than counted".into(),
                    )));
            }
        }
        sequence.end()
    }
}

struct ExportModel<'a> {
    context: &'a JsonExportContext<'a>,
    model: &'a Model,
}

impl Serialize for ExportModel<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        let model = self.model;
        let context = self.context;
        let file_path = model
            .source_path
            .as_deref()
            .or(model.storage_path.as_deref())
            .unwrap_or_default();
        let print_status = export_print_status(&model.metadata.print_status);
        let parent_model = if let Some(id) = model.metadata.parent_model_id.as_deref() {
            context.serialize_result::<_, S::Error>(
                context.db.active_model_display_name_for_json_export(id),
            )?
        } else {
            None
        };

        let mut map = serializer.serialize_map(Some(18))?;
        map.serialize_entry("designer", &model.metadata.designer)?;
        map.serialize_entry("favorite", &model.metadata.favorite)?;
        map.serialize_entry(
            "filaments",
            &ExportFilaments {
                context,
                ids: &model.filament_ids,
            },
        )?;
        map.serialize_entry(
            "fileName",
            model
                .original_name
                .as_deref()
                .unwrap_or(&model.display_name),
        )?;
        map.serialize_entry("filePath", file_path)?;
        map.serialize_entry("gogglelabParentModelId", &model.metadata.parent_model_id)?;
        map.serialize_entry(
            "gogglelabTags",
            &ExportCategorizedTags {
                context,
                tags: &model.tags,
            },
        )?;
        map.serialize_entry("last_printed_at", &model.metadata.last_printed_at)?;
        map.serialize_entry("license", &model.metadata.model_license)?;
        map.serialize_entry("notes", &model.metadata.notes)?;
        map.serialize_entry("parentModel", &parent_model)?;
        map.serialize_entry(
            "printEvents",
            &ExportPrintEvents {
                context,
                model_id: &model.id,
            },
        )?;
        map.serialize_entry("print_count", &model.metadata.print_count)?;
        map.serialize_entry("print_status", print_status)?;
        map.serialize_entry(
            "printed",
            &(model.metadata.print_count > 0 || print_status == "printed"),
        )?;
        map.serialize_entry("rating", &model.metadata.rating)?;
        map.serialize_entry("source", &model.metadata.source_url)?;
        map.serialize_entry("tags", &model.tags)?;
        map.end()
    }
}

struct ExportCategorizedTags<'a> {
    context: &'a JsonExportContext<'a>,
    tags: &'a [String],
}

impl Serialize for ExportCategorizedTags<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;

        let mut sequence = serializer.serialize_seq(Some(self.tags.len()))?;
        for tag in self.tags {
            let category = self.context.serialize_result::<_, S::Error>(
                self.context.db.tag_category_for_json_export(tag),
            )?;
            let value = serde_json::json!({ "name": tag, "category": category });
            sequence.serialize_element(&value)?;
        }
        sequence.end()
    }
}

struct ExportFilaments<'a> {
    context: &'a JsonExportContext<'a>,
    ids: &'a [String],
}

impl Serialize for ExportFilaments<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut sequence = serializer.serialize_seq(Some(self.ids.len()))?;
        for id in self.ids {
            let value = self
                .context
                .serialize_result::<_, S::Error>(export_filament_from_db(self.context.db, id))?;
            sequence.serialize_element(&value)?;
        }
        sequence.end()
    }
}

struct ExportPrintEvents<'a> {
    context: &'a JsonExportContext<'a>,
    model_id: &'a str,
}

impl Serialize for ExportPrintEvents<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut sequence = serializer.serialize_seq(None)?;
        let mut after_printed_at: Option<String> = None;
        let mut after_id: Option<String> = None;
        loop {
            let page = self.context.serialize_result::<_, S::Error>(
                self.context.db.list_print_attempts_for_json_export_page(
                    self.model_id,
                    after_printed_at.as_deref(),
                    after_id.as_deref(),
                    PRINTVENTORY_JSON_PAGE_ROWS,
                ),
            )?;
            if page.is_empty() {
                break;
            }
            for attempt in &page {
                sequence.serialize_element(&ExportPrintEvent {
                    context: self.context,
                    attempt,
                })?;
            }
            let last = page.last().expect("non-empty print event page");
            after_printed_at = Some(last.printed_at.clone());
            after_id = Some(last.id.clone());
        }
        sequence.end()
    }
}

struct ExportPrintEvent<'a> {
    context: &'a JsonExportContext<'a>,
    attempt: &'a CorePrintAttempt,
}

impl Serialize for ExportPrintEvent<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let attempt = self.attempt;
        let printer = attempt
            .printer_id
            .as_ref()
            .and_then(|id| self.context.printers_by_id.get(id));
        let printer_nickname = attempt
            .printer_name
            .clone()
            .or_else(|| printer.map(|value| value.nickname.clone()));
        let created_at = self
            .context
            .serialize_result::<_, S::Error>(export_rfc3339_timestamp(
                &attempt.created_at,
                "Print event creation",
            ))?;
        let parts = export_print_attempt_parts(&attempt.parts);
        let mut map = serializer.serialize_map(Some(12))?;
        map.serialize_entry("created_at", &created_at)?;
        map.serialize_entry(
            "filaments",
            &ExportFilaments {
                context: self.context,
                ids: &attempt.filament_ids,
            },
        )?;
        map.serialize_entry("gogglelab_printer_id", &attempt.printer_id)?;
        map.serialize_entry("notes", &attempt.notes)?;
        map.serialize_entry("outcome", export_print_outcome(&attempt.outcome))?;
        map.serialize_entry("parts", &parts)?;
        map.serialize_entry("printed_at", &attempt.printed_at)?;
        map.serialize_entry(
            "printer_manufacturer",
            &printer.and_then(|value| value.make.as_ref()),
        )?;
        map.serialize_entry(
            "printer_model",
            &printer.and_then(|value| value.model.as_ref()),
        )?;
        map.serialize_entry("printer_nickname", &printer_nickname)?;
        map.serialize_entry(
            "printer_type",
            &printer.and_then(|value| value.printer_type.as_ref()),
        )?;
        map.serialize_entry("quantity", &attempt.quantity)?;
        map.end()
    }
}

fn export_filament_from_db(db: &LibraryDb, id: &str) -> Result<serde_json::Value, LibraryError> {
    let filament = db.get_filament(id)?.ok_or_else(|| {
        LibraryError::InvalidInput(
            "Library JSON export cannot preserve an unresolved filament reference".into(),
        )
    })?;
    Ok(export_filament_value(&filament))
}

#[cfg(test)]
fn stream_library_json_export<W: Write>(
    db: &LibraryDb,
    export_date: &str,
    writer: &mut CappedJsonWriter<W>,
) -> Result<usize, LibraryError> {
    stream_library_json_export_with_slicers(db, export_date, writer, &[])
}

fn stream_library_json_export_with_slicers<W: Write>(
    db: &LibraryDb,
    export_date: &str,
    writer: &mut CappedJsonWriter<W>,
    slicers: &[SlicerJsonExportEntry],
) -> Result<usize, LibraryError> {
    validate_slicer_json_export(slicers)?;
    let context = JsonExportContext::new(db)?;
    let view = LibraryJsonExportView {
        context: &context,
        export_date,
        slicers,
    };
    let serialization = {
        let mut serializer = serde_json::Serializer::pretty(&mut *writer);
        view.serialize(&mut serializer)
    };
    if serialization.is_err() {
        if writer.exceeded {
            return Err(library_json_export_size_error(writer.limit));
        }
        if let Some(error) = context.failure.into_inner() {
            return Err(error);
        }
        return Err(LibraryError::InvalidInput(
            "Library JSON export could not be encoded or saved".into(),
        ));
    }
    Ok(context.model_count.get())
}

#[derive(Debug, Clone)]
pub struct LibraryJsonExportResult {
    pub models: usize,
    pub bytes: u64,
}

struct ExportDestination {
    path: PathBuf,
    parent_path: PathBuf,
    name: std::ffi::OsString,
    #[cfg(unix)]
    directory: GuardedDirectory,
}

fn export_destination(destination: &Path) -> Result<ExportDestination, LibraryError> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_path = parent.canonicalize().map_err(|_| {
        LibraryError::InvalidInput("Library JSON export destination is unavailable".into())
    })?;
    let name = destination.file_name().ok_or_else(|| {
        LibraryError::InvalidInput("Library JSON export destination is unavailable".into())
    })?;
    #[cfg(unix)]
    let directory = GuardedDirectory::open(&parent_path).map_err(|_| {
        LibraryError::InvalidInput("Library JSON export destination is unavailable".into())
    })?;
    Ok(ExportDestination {
        path: parent_path.join(name),
        parent_path,
        name: name.to_os_string(),
        #[cfg(unix)]
        directory,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExportFingerprint {
    bytes: u64,
    sha256: String,
}

struct FingerprintedWriter<'a, W> {
    inner: &'a mut W,
    bytes: u64,
    digest: Sha256,
}

impl<W> FingerprintedWriter<'_, W> {
    fn fingerprint(&self) -> ExportFingerprint {
        ExportFingerprint {
            bytes: self.bytes,
            sha256: format!("{:x}", self.digest.clone().finalize()),
        }
    }
}

impl<W: Write> Write for FingerprintedWriter<'_, W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.bytes = self
            .bytes
            .checked_add(written as u64)
            .ok_or_else(|| std::io::Error::other("export byte count overflow"))?;
        self.digest.update(&buffer[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn write_export_bytes<W: Write>(
    output: &mut W,
    bytes: &[u8],
) -> Result<ExportFingerprint, (std::io::Error, ExportFingerprint)> {
    let mut writer = FingerprintedWriter {
        inner: output,
        bytes: 0,
        digest: Sha256::new(),
    };
    let write_result = writer.write_all(bytes);
    let fingerprint = writer.fingerprint();
    match write_result {
        Ok(()) => Ok(fingerprint),
        Err(error) => Err((error, fingerprint)),
    }
}

struct FailAfterWriter<'a> {
    file: &'a mut File,
    remaining: usize,
}

impl Write for FailAfterWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::other("injected export write failure"));
        }
        let limit = buffer.len().min(self.remaining);
        let written = self.file.write(&buffer[..limit])?;
        self.remaining -= written;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn same_export_file_identity(expected: &File, current: &File) -> bool {
    library_core::catalog_vfs::same_file_identity(expected, current).is_ok_and(|same| same)
}

#[cfg(windows)]
fn file_fingerprint(file: File) -> std::io::Result<ExportFingerprint> {
    use std::os::windows::fs::FileExt;
    // Windows seek_read advances this handle's cursor. Consume only the fresh
    // verification handle, never a clone sharing the created output's cursor.
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.seek_read(&mut buffer, bytes)?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or_else(|| std::io::Error::other("export byte count overflow"))?;
        if bytes > MAX_PRINTVENTORY_JSON_BYTES as u64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "export exceeds its byte limit",
            ));
        }
        digest.update(&buffer[..read]);
    }
    Ok(ExportFingerprint {
        bytes,
        sha256: format!("{:x}", digest.finalize()),
    })
}

fn destination_matches_open_file(
    destination: &ExportDestination,
    output: &File,
    fingerprint: &ExportFingerprint,
) -> bool {
    #[cfg(unix)]
    {
        let Ok(mut current) = destination.directory.open_file_handle(&destination.name) else {
            return false;
        };
        if !same_export_file_identity(output, &current) {
            return false;
        }
        return secure_fs::hash_file(&mut current, MAX_PRINTVENTORY_JSON_BYTES as u64)
            .map(|(bytes, sha256)| bytes == fingerprint.bytes && sha256 == fingerprint.sha256)
            .unwrap_or(false);
    }
    #[cfg(windows)]
    {
        let Ok(current) = library_core::catalog_vfs::open_pinned_file(&destination.path) else {
            return false;
        };
        if !same_export_file_identity(output, &current) {
            return false;
        }
        return file_fingerprint(current).ok().as_ref() == Some(fingerprint);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (destination, output, fingerprint);
        false
    }
}

#[derive(Debug)]
pub struct StagedLibraryJsonExport {
    destination: ExportDestination,
    bytes: Vec<u8>,
    fingerprint: ExportFingerprint,
    result: LibraryJsonExportResult,
}

impl std::fmt::Debug for ExportDestination {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExportDestination")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl StagedLibraryJsonExport {
    pub fn result(&self) -> &LibraryJsonExportResult {
        &self.result
    }

    /// Creates the selected path exclusively, then writes the bounded export.
    /// Overflow is detected before this method is called.
    pub fn publish(self, destination: &Path) -> Result<LibraryJsonExportResult, LibraryError> {
        self.publish_with_writer(
            destination,
            |file, bytes| write_export_to_file(file, bytes, None),
            false,
        )
    }

    #[cfg(test)]
    fn publish_with_failure_after(
        self,
        destination: &Path,
        fail_after: usize,
    ) -> Result<LibraryJsonExportResult, LibraryError> {
        self.publish_with_writer(
            destination,
            |file, bytes| write_export_to_file(file, bytes, Some(fail_after)),
            false,
        )
    }

    #[cfg(test)]
    fn publish_with_sync_failure(
        self,
        destination: &Path,
    ) -> Result<LibraryJsonExportResult, LibraryError> {
        self.publish_with_writer(
            destination,
            |file, bytes| write_export_to_file(file, bytes, None),
            true,
        )
    }

    #[cfg(all(test, unix))]
    fn publish_with_replacement_during_write(
        self,
        destination: &Path,
        replace_after: usize,
        replacement: &[u8],
        displaced_path: &Path,
    ) -> Result<LibraryJsonExportResult, LibraryError> {
        let destination_path = self.destination.path.clone();
        let displaced_path = displaced_path.to_path_buf();
        let replacement = replacement.to_vec();
        self.publish_with_writer(
            destination,
            move |file, bytes| {
                let mut writer = ReplaceDuringWriteWriter {
                    file,
                    remaining: replace_after,
                    destination_path,
                    displaced_path,
                    replacement,
                    replaced: false,
                };
                write_export_bytes(&mut writer, bytes)
            },
            false,
        )
    }

    fn publish_with_writer<F>(
        self,
        destination: &Path,
        write_output: F,
        inject_sync_failure: bool,
    ) -> Result<LibraryJsonExportResult, LibraryError>
    where
        F: FnOnce(
            &mut File,
            &[u8],
        ) -> Result<ExportFingerprint, (std::io::Error, ExportFingerprint)>,
    {
        let selected = export_destination(destination)?;
        if selected.path != self.destination.path {
            return Err(LibraryError::InvalidInput(
                "Library export destination changed before publication".into(),
            ));
        }
        #[cfg(unix)]
        {
            let parent_matches = self
                .destination
                .directory
                .0
                .matches_path(&self.destination.parent_path)
                .unwrap_or(false);
            if !parent_matches {
                return Err(LibraryError::InvalidInput(
                    "Library export destination changed before publication".into(),
                ));
            }
            let mut file = self
                .destination
                .directory
                .0
                .create_new_file_read_write(&self.destination.name)
                .map_err(|_| {
                    LibraryError::InvalidInput(
                        "Library export destination already exists or is unavailable".into(),
                    )
                })?;
            let fingerprint = match write_output(&mut file, &self.bytes) {
                Ok(fingerprint) if fingerprint == self.fingerprint => fingerprint,
                Ok(_) | Err(_) => return Err(export_write_failure()),
            };
            if inject_sync_failure || file.sync_all().is_err() {
                return Err(export_write_failure());
            }
            if self.destination.directory.0.sync().is_err() {
                return Err(export_write_failure());
            }
            if !destination_matches_open_file(&self.destination, &file, &fingerprint) {
                return Err(export_verify_failure());
            }
        }
        #[cfg(not(unix))]
        {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.access_mode(0x8000_0000 | 0x4000_0000);
                options.share_mode(0x0000_0001);
            }
            let mut file = options.open(&self.destination.path).map_err(|_| {
                LibraryError::InvalidInput(
                    "Library export destination already exists or is unavailable".into(),
                )
            })?;
            let fingerprint = match write_output(&mut file, &self.bytes) {
                Ok(fingerprint) if fingerprint == self.fingerprint => fingerprint,
                Ok(_) | Err(_) => return Err(export_write_failure()),
            };
            if inject_sync_failure || file.sync_all().is_err() {
                return Err(export_write_failure());
            }
            if !destination_matches_open_file(&self.destination, &file, &fingerprint) {
                return Err(export_verify_failure());
            }
        }
        Ok(self.result)
    }
}

fn export_write_failure() -> LibraryError {
    LibraryError::InvalidInput(
        "Library JSON export could not be saved; a partial file may remain at the chosen destination. Review or remove it before retrying.".into(),
    )
}

fn export_verify_failure() -> LibraryError {
    LibraryError::InvalidInput(
        "Library JSON export could not be verified; a file may remain at the chosen destination. Review or remove it before retrying.".into(),
    )
}

fn write_export_to_file(
    file: &mut File,
    bytes: &[u8],
    fail_after: Option<usize>,
) -> Result<ExportFingerprint, (std::io::Error, ExportFingerprint)> {
    if let Some(remaining) = fail_after {
        let mut limited = FailAfterWriter { file, remaining };
        write_export_bytes(&mut limited, bytes)
    } else {
        write_export_bytes(file, bytes)
    }
}

#[cfg(all(test, unix))]
struct ReplaceDuringWriteWriter<'a> {
    file: &'a mut File,
    remaining: usize,
    destination_path: PathBuf,
    displaced_path: PathBuf,
    replacement: Vec<u8>,
    replaced: bool,
}

#[cfg(all(test, unix))]
impl Write for ReplaceDuringWriteWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.replaced {
            return self.file.write(buffer);
        }
        if self.remaining == 0 {
            self.replace_path()?;
            return self.file.write(buffer);
        }
        let count = buffer.len().min(self.remaining);
        let written = self.file.write(&buffer[..count])?;
        self.remaining -= written;
        if self.remaining == 0 {
            self.replace_path()?;
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

#[cfg(all(test, unix))]
impl ReplaceDuringWriteWriter<'_> {
    fn replace_path(&mut self) -> std::io::Result<()> {
        std::fs::rename(&self.destination_path, &self.displaced_path)?;
        std::fs::write(&self.destination_path, &self.replacement)?;
        self.replaced = true;
        Ok(())
    }
}

#[cfg(test)]
fn stage_library_json_export_with_limit(
    db: &LibraryDb,
    export_date: &str,
    destination: &Path,
    limit: usize,
) -> Result<StagedLibraryJsonExport, LibraryError> {
    stage_library_json_export_with_limit_and_slicers(db, export_date, destination, limit, &[])
}

fn stage_library_json_export_with_limit_and_slicers(
    db: &LibraryDb,
    export_date: &str,
    destination: &Path,
    limit: usize,
    slicers: &[SlicerJsonExportEntry],
) -> Result<StagedLibraryJsonExport, LibraryError> {
    validate_slicer_json_export(slicers)?;
    let destination = export_destination(destination)?;
    let mut writer = CappedJsonWriter::new(Vec::new(), limit);
    let models = db.with_json_export_snapshot(|db| {
        stream_library_json_export_with_slicers(db, export_date, &mut writer, slicers)
    })?;
    writer
        .flush()
        .map_err(|_| library_json_export_writer_error(&writer))?;
    let bytes = writer.bytes_written() as u64;
    let content = writer.into_inner();
    let fingerprint = ExportFingerprint {
        bytes,
        sha256: format!("{:x}", Sha256::digest(&content)),
    };
    Ok(StagedLibraryJsonExport {
        destination,
        bytes: content,
        fingerprint,
        result: LibraryJsonExportResult { models, bytes },
    })
}

pub fn stage_library_json_export(
    db: &LibraryDb,
    destination: &Path,
) -> Result<StagedLibraryJsonExport, LibraryError> {
    stage_library_json_export_with_slicers(db, destination, &[])
}

/// Desktop callers provide a snapshot of saved name/path metadata. Headless
/// exports use the empty default because slicer applications are workstation-local.
pub fn stage_library_json_export_with_slicers(
    db: &LibraryDb,
    destination: &Path,
    slicers: &[SlicerJsonExportEntry],
) -> Result<StagedLibraryJsonExport, LibraryError> {
    let export_date = format_system_time(SystemTime::now()).map_err(|_| {
        LibraryError::InvalidInput("Library JSON export timestamp could not be formatted".into())
    })?;
    stage_library_json_export_with_limit_and_slicers(
        db,
        &export_date,
        destination,
        MAX_PRINTVENTORY_JSON_BYTES,
        slicers,
    )
}

/// Builds a compatible library JSON value with a fresh UTC date.
pub fn export_library_json_value(
    db: &LibraryDb,
) -> Result<(serde_json::Value, usize), LibraryError> {
    let (export, model_count, _) = export_library_json_value_with_bytes(db)?;
    Ok((export, model_count))
}

pub fn export_library_json_value_with_slicers(
    db: &LibraryDb,
    slicers: &[SlicerJsonExportEntry],
) -> Result<(serde_json::Value, usize), LibraryError> {
    let (export, model_count, _) = export_library_json_value_with_bytes_and_slicers(db, slicers)?;
    Ok((export, model_count))
}

fn export_library_json_value_with_bytes(
    db: &LibraryDb,
) -> Result<(serde_json::Value, usize, Vec<u8>), LibraryError> {
    export_library_json_value_with_bytes_and_slicers(db, &[])
}

fn export_library_json_value_with_bytes_and_slicers(
    db: &LibraryDb,
    slicers: &[SlicerJsonExportEntry],
) -> Result<(serde_json::Value, usize, Vec<u8>), LibraryError> {
    validate_slicer_json_export(slicers)?;
    let export_date = format_system_time(SystemTime::now()).map_err(|_| {
        LibraryError::InvalidInput("Library JSON export timestamp could not be formatted".into())
    })?;
    db.with_json_export_snapshot(|db| {
        let mut preflight = CappedJsonWriter::new(std::io::sink(), MAX_PRINTVENTORY_JSON_BYTES);
        stream_library_json_export_with_slicers(db, &export_date, &mut preflight, slicers)?;
        let (export, model_count) =
            build_library_export_value_with_slicers(db, &export_date, slicers)?;
        let bytes = serialize_library_json_export(&export)?;
        Ok((export, model_count, bytes))
    })
}

fn serialize_library_json_export(export: &serde_json::Value) -> Result<Vec<u8>, LibraryError> {
    let mut writer = CappedJsonWriter::new(Vec::new(), MAX_PRINTVENTORY_JSON_BYTES);
    serde_json::to_writer_pretty(&mut writer, export).map_err(|_| {
        if writer.exceeded {
            library_json_export_size_error(writer.limit)
        } else {
            LibraryError::InvalidInput("Library JSON export could not be encoded".into())
        }
    })?;
    Ok(writer.into_inner())
}

fn execute_library_export(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    destination_handle_id: &str,
    file_name: &str,
    expected_epoch: u64,
    expected_revision: u64,
) -> Result<ManagementResult, RuntimeError> {
    runtime.require(context, Capability::Maintenance)?;
    runtime.require(context, Capability::BackupWrite)?;
    let (_, destination) = admitted_file(
        runtime,
        context,
        destination_handle_id,
        RootAccess::BackupOnly,
        file_name,
        false,
    )?;
    let stage = runtime.read_db(|db| stage_library_json_export(db, &destination))?;
    let runtime_arc = runtime
        .self_weak
        .get()
        .and_then(Weak::upgrade)
        .ok_or_else(|| internal("Runtime self reference is unavailable"))?;
    let _guard = runtime_arc.begin_maintenance()?;
    recheck_management_plan(runtime, expected_epoch, expected_revision)?;
    let result = stage.publish(&destination).map_err(runtime_library_error)?;
    Ok(ManagementResult::LibraryExported {
        destination_handle_id: destination_handle_id.into(),
        file_name: file_name.into(),
        models: result.models,
        bytes: result.bytes,
    })
}

#[cfg(test)]
fn write_library_json_export(
    destination: &Path,
    export: &serde_json::Value,
) -> Result<usize, RuntimeError> {
    let bytes = serialize_library_json_export(export).map_err(runtime_library_error)?;
    let byte_count = bytes.len();
    write_library_json_export_bytes(destination, &bytes)?;
    Ok(byte_count)
}

#[cfg(test)]
fn write_library_json_export_bytes(destination: &Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|_| invalid("Library export destination already exists or is unavailable"))?;
    let write_result = output
        .write_all(&bytes)
        .and_then(|()| output.sync_all())
        .map_err(|_| io_error());
    drop(output);
    if write_result.is_err() {
        let _ = hold_runtime_path(destination);
    }
    write_result?;
    Ok(())
}

struct RestoreExecution<'a> {
    source_handle_id: &'a str,
    file_name: &'a str,
    root_mappings: &'a [RestoreRootMapping],
    expected_input: &'a (u64, String),
    expected_epoch: u64,
    expected_revision: u64,
}

#[derive(Debug)]
struct RestoreSwapFailure {
    message: String,
    recovered: bool,
}

fn restore_swap_failure(message: impl Into<String>, recovered: bool) -> RestoreSwapFailure {
    RestoreSwapFailure {
        message: message.into(),
        recovered,
    }
}

fn rollback_restore_before_install(
    parent: &secure_fs::Directory,
    root_name: &std::ffi::OsStr,
    previous_name: &std::ffi::OsStr,
) -> Result<(), String> {
    parent
        .rename_child_exclusive(previous_name, root_name)
        .and_then(|()| parent.sync())
        .map_err(|error| error.to_string())
}

fn rollback_restore_after_install(
    parent: &secure_fs::Directory,
    root_name: &std::ffi::OsStr,
    failed_name: &std::ffi::OsStr,
    previous_name: &std::ffi::OsStr,
) -> Result<(), String> {
    parent
        .rename_child_exclusive(root_name, failed_name)
        .and_then(|()| parent.sync())
        .and_then(|()| parent.rename_child_exclusive(previous_name, root_name))
        .and_then(|()| parent.sync())
        .map_err(|error| error.to_string())
}

fn swap_restored_library_with<F, V>(
    parent: &secure_fs::Directory,
    original: &secure_fs::Directory,
    staged: &secure_fs::Directory,
    root_name: &std::ffi::OsStr,
    staged_name: &std::ffi::OsStr,
    previous_name: &std::ffi::OsStr,
    failed_name: &std::ffi::OsStr,
    after_original_saved: F,
    validate_installed: V,
) -> Result<(), RestoreSwapFailure>
where
    F: FnOnce(),
    V: FnOnce(&Path) -> Result<(), String>,
{
    let root_path = parent
        .child_path(root_name)
        .map_err(|error| restore_swap_failure(error.to_string(), true))?;
    let staged_path = parent
        .child_path(staged_name)
        .map_err(|error| restore_swap_failure(error.to_string(), true))?;
    if !original
        .matches_path(&root_path)
        .map_err(|error| restore_swap_failure(error.to_string(), true))?
        || !staged
            .matches_path(&staged_path)
            .map_err(|error| restore_swap_failure(error.to_string(), true))?
    {
        return Err(restore_swap_failure(
            "Library or restore staging directory identity changed before swap",
            true,
        ));
    }
    if parent
        .entry_exists(previous_name)
        .map_err(|error| restore_swap_failure(error.to_string(), true))?
    {
        return Err(restore_swap_failure(
            "Previous Library destination is already occupied",
            true,
        ));
    }
    parent
        .rename_child_exclusive(root_name, previous_name)
        .map_err(|error| restore_swap_failure(error.to_string(), true))?;
    if let Err(error) = parent.sync() {
        return match rollback_restore_before_install(parent, root_name, previous_name) {
            Ok(()) => Err(restore_swap_failure(
                format!("Could not sync the saved Library directory: {error}"),
                true,
            )),
            Err(rollback) => Err(restore_swap_failure(
                format!(
                    "Could not sync or restore the saved Library directory: {error}; {rollback}"
                ),
                false,
            )),
        };
    }
    let previous_matches = parent
        .child_path(previous_name)
        .ok()
        .and_then(|path| original.matches_path(&path).ok())
        .unwrap_or(false);
    if !previous_matches {
        return match rollback_restore_before_install(parent, root_name, previous_name) {
            Ok(()) => Err(restore_swap_failure(
                "Saved Library identity changed during restore swap",
                true,
            )),
            Err(rollback) => Err(restore_swap_failure(
                format!("Saved Library identity changed and rollback failed: {rollback}"),
                false,
            )),
        };
    }

    after_original_saved();
    let staged_still_named = parent
        .child_path(staged_name)
        .ok()
        .and_then(|path| staged.matches_path(&path).ok())
        .unwrap_or(false);
    if !staged_still_named {
        return match rollback_restore_before_install(parent, root_name, previous_name) {
            Ok(()) => Err(restore_swap_failure(
                "Restore staging directory changed before installation",
                true,
            )),
            Err(rollback) => Err(restore_swap_failure(
                format!("Restore staging changed and Library rollback failed: {rollback}"),
                false,
            )),
        };
    }
    if let Err(error) = parent.rename_child_exclusive(staged_name, root_name) {
        return match rollback_restore_before_install(parent, root_name, previous_name) {
            Ok(()) => Err(restore_swap_failure(
                format!("Library replacement failed: {error}"),
                true,
            )),
            Err(rollback) => Err(restore_swap_failure(
                format!("Library replacement failed: {error}; rollback failed: {rollback}"),
                false,
            )),
        };
    }
    if let Err(error) = parent.sync() {
        return match rollback_restore_after_install(parent, root_name, failed_name, previous_name) {
            Ok(()) => Err(restore_swap_failure(
                format!("Could not sync the installed Library directory: {error}"),
                true,
            )),
            Err(rollback) => Err(restore_swap_failure(
                format!("Could not sync installed Library or roll back: {error}; {rollback}"),
                false,
            )),
        };
    }
    let root_path = match parent.child_path(root_name) {
        Ok(path) => path,
        Err(error) => {
            return match rollback_restore_after_install(
                parent,
                root_name,
                failed_name,
                previous_name,
            ) {
                Ok(()) => Err(restore_swap_failure(
                    format!("Could not resolve installed Library path: {error}"),
                    true,
                )),
                Err(rollback) => Err(restore_swap_failure(
                    format!(
                        "Could not resolve or roll back installed Library: {error}; {rollback}"
                    ),
                    false,
                )),
            };
        }
    };
    if !staged.matches_path(&root_path).unwrap_or(false) {
        return match rollback_restore_after_install(parent, root_name, failed_name, previous_name) {
            Ok(()) => Err(restore_swap_failure(
                "Restored Library path changed after installation",
                true,
            )),
            Err(rollback) => Err(restore_swap_failure(
                format!("Restored Library identity changed and rollback failed: {rollback}"),
                false,
            )),
        };
    }
    if let Err(validation) = validate_installed(&root_path) {
        return match rollback_restore_after_install(parent, root_name, failed_name, previous_name) {
            Ok(()) => Err(restore_swap_failure(
                format!(
                    "Restored Library failed validation; previous Library recovered: {validation}"
                ),
                true,
            )),
            Err(rollback) => Err(restore_swap_failure(
                format!(
                    "Restored Library failed validation and rollback failed: {validation}; {rollback}"
                ),
                false,
            )),
        };
    }
    if !staged.matches_path(&root_path).unwrap_or(false) {
        return match rollback_restore_after_install(parent, root_name, failed_name, previous_name) {
            Ok(()) => Err(restore_swap_failure(
                "Restored Library identity changed during final validation",
                true,
            )),
            Err(rollback) => Err(restore_swap_failure(
                format!("Restored Library identity changed and rollback failed: {rollback}"),
                false,
            )),
        };
    }
    Ok(())
}

fn execute_restore(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    execution: RestoreExecution<'_>,
) -> Result<ManagementResult, RuntimeError> {
    require_durable_source_mutations()?;
    runtime.require(context, Capability::Maintenance)?;
    let (_, source) = admitted_file(
        runtime,
        context,
        execution.source_handle_id,
        RootAccess::Inbox,
        execution.file_name,
        true,
    )
    .or_else(|_| {
        admitted_file(
            runtime,
            context,
            execution.source_handle_id,
            RootAccess::ReadOnly,
            execution.file_name,
            true,
        )
    })?;
    let floor = runtime.read_db(|db| db.max_metadata_version())?;
    let runtime_arc = runtime
        .self_weak
        .get()
        .and_then(Weak::upgrade)
        .ok_or_else(|| internal("Runtime self reference is unavailable"))?;
    let guard = runtime_arc.begin_maintenance()?;
    recheck_management_plan(
        runtime,
        execution.expected_epoch,
        execution.expected_revision,
    )?;
    let root = runtime.root.clone();
    let parent = root
        .parent()
        .ok_or_else(|| invalid("Library root has no stable parent"))?;
    let parent_directory = secure_fs::Directory::open(parent)
        .map_err(|_| stale("Library parent is no longer a safe directory"))?;
    if !parent_directory
        .matches_path(parent)
        .map_err(|_| stale("Library parent identity changed"))?
    {
        return Err(stale("Library parent identity changed"));
    }
    let root_name = root
        .file_name()
        .ok_or_else(|| invalid("Library root name is invalid"))?
        .to_os_string();
    let original = parent_directory
        .open_child(&root_name)
        .map_err(|_| stale("Library root is no longer a safe directory"))?;
    if !original
        .matches_path(&root)
        .map_err(|_| stale("Library root identity changed"))?
    {
        return Err(stale("Library root identity changed"));
    }
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let stable_source_name = std::ffi::OsString::from(format!(".restore-input-{suffix}.zip"));
    let staged_name = std::ffi::OsString::from(format!(".gogglelab-restore-{suffix}"));
    let previous_name = std::ffi::OsString::from(format!("library.previous-{suffix}"));
    let previous_backup_name = std::ffi::OsString::from(format!("library.previous-{suffix}.zip"));
    let failed_name = std::ffi::OsString::from(format!(".gogglelab-failed-restore-{suffix}"));
    let source_parent = source
        .parent()
        .ok_or_else(|| invalid("Restore archive has no parent directory"))?;
    let source_directory = secure_fs::Directory::open(source_parent)
        .map_err(|_| stale("Restore archive parent changed"))?;
    let source_name = source
        .file_name()
        .ok_or_else(|| invalid("Restore archive name is invalid"))?;
    let input_scratch = create_scratch_directory("restore-input")?;
    secure_fs::copy_verified_into_directory(
        &source_directory,
        source_name,
        &input_scratch.directory,
        &stable_source_name,
        execution.expected_input,
        MAX_ORPHAN_CLEANUP_TOTAL_BYTES + 16 * 1024 * 1024,
    )
    .map_err(|_| stale("Restore backup changed after planning"))?;
    let mut stable_source = input_scratch
        .directory
        .open_file(&stable_source_name)
        .map_err(|_| io_error())?;
    if secure_fs::hash_file(
        &mut stable_source,
        MAX_ORPHAN_CLEANUP_TOTAL_BYTES + 16 * 1024 * 1024,
    )
    .map_err(|_| io_error())?
        != *execution.expected_input
    {
        return Err(stale("Restore backup changed after planning"));
    }
    stable_source.rewind().map_err(|_| io_error())?;
    parent_directory
        .create_child_directory(&staged_name)
        .map_err(|_| io_error())?;
    let staged = parent_directory
        .open_child(&staged_name)
        .map_err(|_| io_error())?;
    let restore_journal = RestoreJournal {
        version: 1,
        root_path: root.to_string_lossy().into_owned(),
        nonce: suffix.clone(),
        staged_name: staged_name.to_string_lossy().into_owned(),
        previous_name: previous_name.to_string_lossy().into_owned(),
        failed_name: failed_name.to_string_lossy().into_owned(),
        committed: false,
    };
    let mut journal_name =
        match write_restore_journal_at(&parent_directory, &root, &restore_journal) {
            Ok(name) => name,
            Err(error) => {
                let stage_still_owned = parent_directory
                    .child_path(&staged_name)
                    .ok()
                    .and_then(|path| staged.matches_path(&path).ok())
                    .unwrap_or(false);
                if stage_still_owned && parent_directory.remove_child_tree(&staged_name).is_err() {
                    guard.remain_armed();
                }
                return Err(error);
            }
        };
    let mut swap_started = false;
    let result = (|| {
        let mut staged_target = GuardedDirectory(staged.try_clone().map_err(|_| io_error())?);
        library_backup::restore_backup_from_reader(stable_source, &mut staged_target)
            .map_err(|_| invalid("Backup could not be restored to staging"))?;
        let staged_path = parent_directory
            .child_path(&staged_name)
            .map_err(|_| io_error())?;
        if !parent_directory
            .matches_path(parent)
            .map_err(|_| stale("Library parent identity changed during restore"))?
            || !staged
                .matches_path(&staged_path)
                .map_err(|_| stale("Restore staging identity changed"))?
        {
            return Err(stale("Restore staging or parent identity changed"));
        }
        let staged_db = LibraryDb::open(&staged_path).map_err(runtime_library_error)?;
        if !staged.matches_path(&staged_path).map_err(|_| io_error())? {
            return Err(stale(
                "Restore staging identity changed during database open",
            ));
        }
        let mappings = execution
            .root_mappings
            .iter()
            .map(|mapping| {
                let admitted = runtime
                    .roots
                    .iter()
                    .find(|root| root.handle.id == mapping.operator_handle_id)
                    .ok_or_else(|| invalid("Restore operator root is not admitted"))?;
                let target_id = admitted
                    .catalog_root_id
                    .clone()
                    .ok_or_else(|| invalid("Restore operator root has no catalog identity"))?;
                if admitted.path.canonicalize().map_err(|_| io_error())? != admitted.path {
                    return Err(stale("Restore operator root identity changed"));
                }
                Ok((
                    mapping.backup_catalog_root_id.clone(),
                    target_id,
                    admitted.path.clone(),
                    mapping.enabled,
                ))
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?;
        staged_db
            .remap_restored_index_roots(&mappings)
            .map_err(runtime_library_error)?;
        if !staged.matches_path(&staged_path).map_err(|_| io_error())? {
            return Err(stale(
                "Restore staging identity changed during index-root remapping",
            ));
        }
        staged_db
            .bump_all_metadata_versions_above(floor)
            .map_err(runtime_library_error)?;
        if !staged.matches_path(&staged_path).map_err(|_| io_error())? {
            return Err(stale(
                "Restore staging identity changed during metadata update",
            ));
        }
        let models = staged_db
            .list_models(None, None)
            .map_err(runtime_library_error)?
            .len();
        drop(staged_db);
        let backup_scratch = create_scratch_directory("restore-previous-backup")?;
        let backup_name = std::ffi::OsString::from("previous-library.zip");
        let backup_path = backup_scratch.path.join(&backup_name);
        let root_handle_path = original.path().map_err(|_| io_error())?;
        if !original.matches_path(&root).map_err(|_| io_error())? {
            return Err(stale("Library root identity changed before backup"));
        }
        let previous_backup = library_backup::create_backup(&root_handle_path, &backup_path)
            .map_err(|_| invalid("Pre-restore backup could not be created"))?;
        let mut backup_file = backup_scratch
            .directory
            .open_file(&backup_name)
            .map_err(|_| io_error())?;
        let backup_fingerprint = secure_fs::hash_file(
            &mut backup_file,
            MAX_ORPHAN_CLEANUP_TOTAL_BYTES + 16 * 1024 * 1024,
        )
        .map_err(|_| io_error())?;
        if backup_fingerprint.0 == 0 || backup_fingerprint.1 != previous_backup.sha256 {
            return Err(invalid("Pre-restore backup failed integrity verification"));
        }
        secure_fs::copy_verified_into_directory(
            &backup_scratch.directory,
            &backup_name,
            &parent_directory,
            &previous_backup_name,
            &backup_fingerprint,
            MAX_ORPHAN_CLEANUP_TOTAL_BYTES + 16 * 1024 * 1024,
        )
        .map_err(|_| stale("Pre-restore backup destination changed"))?;
        let mut copied_backup = parent_directory
            .open_file(&previous_backup_name)
            .map_err(|_| io_error())?;
        if secure_fs::hash_file(
            &mut copied_backup,
            MAX_ORPHAN_CLEANUP_TOTAL_BYTES + 16 * 1024 * 1024,
        )
        .map_err(|_| io_error())?
            != backup_fingerprint
            || !parent_directory
                .matches_path(parent)
                .map_err(|_| stale("Library parent identity changed during backup"))?
        {
            return Err(stale("Pre-restore backup changed after writing"));
        }
        staged.sync_tree().map_err(|_| io_error())?;
        parent_directory.sync().map_err(|_| io_error())?;
        let epoch = guard.invalidate();
        swap_started = true;
        if let Err(failure) = swap_restored_library_with(
            &parent_directory,
            &original,
            &staged,
            &root_name,
            &staged_name,
            &previous_name,
            &failed_name,
            || {},
            |installed_path| {
                if !staged.matches_path(installed_path).unwrap_or(false) {
                    return Err("Restored Library identity changed before validation".into());
                }
                let db = LibraryDb::open(installed_path).map_err(|error| error.to_string())?;
                let result = db
                    .list_models(None, None)
                    .map(|_| ())
                    .map_err(|error| error.to_string());
                drop(db);
                if !staged.matches_path(installed_path).unwrap_or(false) {
                    return Err("Restored Library identity changed during validation".into());
                }
                result
            },
        ) {
            if !failure.recovered {
                guard.remain_armed();
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Corrupt,
                    message: failure.message,
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                });
            }
            if let Err(cleanup_error) =
                clear_restore_journal_at(&parent_directory, &mut journal_name)
            {
                guard.remain_armed();
                return Err(cleanup_error);
            }
            let stage_still_owned = parent_directory
                .child_path(&staged_name)
                .ok()
                .and_then(|path| staged.matches_path(&path).ok())
                .unwrap_or(false);
            if stage_still_owned {
                let _ = parent_directory.remove_child_tree(&staged_name);
            }
            return Err(RuntimeError {
                code: RuntimeErrorCode::Io,
                message: failure.message,
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        if let Err(error) =
            mark_restore_journal_committed_at(&parent_directory, &root, &mut journal_name, &suffix)
        {
            guard.remain_armed();
            return Err(error);
        }
        if let Err(error) = clear_restore_journal_at(&parent_directory, &mut journal_name) {
            guard.remain_armed();
            return Err(error);
        }
        Ok(ManagementResult::BackupRestored {
            models,
            library_epoch: epoch,
        })
    })();
    if result.is_err() && !swap_started {
        let stage_still_owned = parent_directory
            .child_path(&staged_name)
            .ok()
            .and_then(|path| staged.matches_path(&path).ok())
            .unwrap_or(false);
        if stage_still_owned && parent_directory.remove_child_tree(&staged_name).is_err() {
            guard.remain_armed();
        }
        if clear_restore_journal_at(&parent_directory, &mut journal_name).is_err() {
            guard.remain_armed();
        }
    }
    result
}

fn access_allows(actual: RootAccess, required: RootAccess) -> bool {
    actual == required
        || (actual == RootAccess::Writable
            && matches!(required, RootAccess::ReadOnly | RootAccess::BackupOnly))
        || (actual == RootAccess::BackupOnly && required == RootAccess::ReadOnly)
}

fn validate_plain_id(value: &str, label: &str) -> Result<(), RuntimeError> {
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        })
    {
        return Err(invalid(&format!("{label} is invalid")));
    }
    Ok(())
}

fn validate_identifier(path: &Path, label: &str) -> Result<(), RuntimeError> {
    if path.file_name().is_none() || path.parent().is_none() {
        return Err(invalid(&format!("{label} is invalid")));
    }
    Ok(())
}

fn runtime_library_error(error: LibraryError) -> RuntimeError {
    match error {
        LibraryError::MetadataConflict {
            actual,
            expected: _,
            model_id: _,
        }
        | LibraryError::FilamentVersionConflict {
            actual,
            expected: _,
            filament_id: _,
        }
        | LibraryError::PrinterRecordVersionConflict {
            actual,
            expected: _,
            kind: _,
        }
        | LibraryError::PartStockVersionConflict {
            actual,
            expected: _,
            part_id: _,
        } => RuntimeError {
            code: RuntimeErrorCode::Conflict,
            message: "Stored version changed".into(),
            retryable: true,
            current_version: Some(actual),
            source_holds: Vec::new(),
        },
        LibraryError::IdempotencyConflict(_) => RuntimeError {
            code: RuntimeErrorCode::Conflict,
            message: "Idempotency key conflicts with another payload".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        },
        LibraryError::ModelNotFound(_)
        | LibraryError::FolderNotFound(_)
        | LibraryError::IndexRootNotFound(_) => RuntimeError {
            code: RuntimeErrorCode::NotFound,
            message: "Library record was not found".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        },
        LibraryError::SchemaTooNew { .. } | LibraryError::UnsupportedFormat(_) => RuntimeError {
            code: RuntimeErrorCode::Unsupported,
            message: "Library format is unsupported".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        },
        LibraryError::Io { .. } => io_error(),
        LibraryError::Database(_) => RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: "Library database could not be read safely".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        },
        other => RuntimeError {
            code: RuntimeErrorCode::InvalidInput,
            message: other.to_string(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        },
    }
}

fn core_metadata_patch(patch: MetadataPatch) -> CoreMetadataPatch {
    CoreMetadataPatch {
        designer: patch.designer,
        source_url: patch.source_url,
        notes: patch.notes,
        model_license: patch.model_license,
        print_status: patch.print_status.map(|status| match status {
            PrintStatus::Unprinted => CorePrintStatus::Unprinted,
            PrintStatus::Want => CorePrintStatus::Want,
            PrintStatus::Queued => CorePrintStatus::Queued,
            PrintStatus::Printing => CorePrintStatus::Printing,
            PrintStatus::Printed => CorePrintStatus::Printed,
            PrintStatus::Failed => CorePrintStatus::Failed,
        }),
        tags: patch.tag_ids,
        tag_mode: patch.tag_mode.map(|mode| match mode {
            TagMode::Replace => CoreTagMode::Replace,
            TagMode::Merge => CoreTagMode::Merge,
        }),
        folder_id: patch.folder_id,
        parent_model_id: patch.parent_model_id,
        rating: patch.rating,
        favorite: patch.favorite,
    }
}

fn core_model_query(query: &ModelQuery) -> CoreModelPageQuery {
    CoreModelPageQuery {
        folder_id: query.folder_id.clone(),
        search: query.search.clone(),
        sort: match query.sort {
            ModelSort::Name => CoreModelSort::Name,
            ModelSort::Size => CoreModelSort::Size,
            ModelSort::Date => CoreModelSort::Date,
        },
        direction: match query.direction {
            SortDirection::Asc => CoreSortDirection::Asc,
            SortDirection::Desc => CoreSortDirection::Desc,
        },
        tag_id: query.tag_id.clone(),
        category: query.category.clone(),
        designer: query.designer.clone(),
        model_license: query.model_license.clone(),
        print_status: query.print_status.map(|status| match status {
            PrintStatus::Unprinted => CorePrintStatus::Unprinted,
            PrintStatus::Want => CorePrintStatus::Want,
            PrintStatus::Queued => CorePrintStatus::Queued,
            PrintStatus::Printing => CorePrintStatus::Printing,
            PrintStatus::Printed => CorePrintStatus::Printed,
            PrintStatus::Failed => CorePrintStatus::Failed,
        }),
        ever_printed: query.ever_printed,
        bundle_id: query.bundle_id.clone(),
        root_id: query.root_id.clone(),
        filament_id: query.filament_id.clone(),
        source_missing: query.source_missing,
        duplicates_only: query.duplicates_only,
        source_kind: query.source_kind.clone(),
        thumbnail_mode: query.thumbnail_mode.clone(),
        source_directory: query.source_directory.clone(),
        minimum_rating: query.minimum_rating,
        favorite: query.favorite,
        tag_names: query.tags.clone().unwrap_or_default(),
        file_type: query.file_type.clone(),
        exclude_model_ids: query.exclude_model_ids.clone(),
        group: query.group.map(|group| match group {
            ModelGroupFilter::Standalone => CoreModelGroupFilter::Standalone,
            ModelGroupFilter::Bundle => CoreModelGroupFilter::Bundle,
            ModelGroupFilter::Archive => CoreModelGroupFilter::Archive,
        }),
    }
}

fn model_sort_value(model: &Model, sort: ModelSort) -> String {
    match sort {
        ModelSort::Name => model.display_name.to_ascii_lowercase(),
        ModelSort::Size => model.bytes.to_string(),
        ModelSort::Date => model.created_at.clone(),
    }
}

fn mutation_version(model: Model) -> MutationVersion {
    MutationVersion {
        model_id: model.id,
        metadata_version: model.metadata_version,
    }
}

fn core_filament_input(input: FilamentInput) -> CoreFilamentInput {
    CoreFilamentInput {
        brand: input.brand,
        material: input.material,
        color: input.color,
        name: input.name,
        notes: input.notes,
    }
}

fn folder(value: CoreFolder) -> Folder {
    Folder {
        id: value.id,
        name: value.name,
        parent_id: value.parent_id,
        model_count: value.model_count,
    }
}

fn filament(value: CoreFilament) -> Filament {
    Filament {
        id: value.id,
        version: value.version,
        brand: value.brand,
        material: value.material,
        color: value.color,
        name: value.name,
        notes: value.notes,
        origin: match value.origin {
            library_core::FilamentOrigin::Manual => "manual",
            library_core::FilamentOrigin::Spoolman => "spoolman",
        }
        .into(),
        resource_kind: match value.resource_kind {
            library_core::FilamentResourceKind::Manual => "manual",
            library_core::FilamentResourceKind::Filament => "filament",
            library_core::FilamentResourceKind::Spool => "spool",
        }
        .into(),
        source_id: value.source_id,
        external_id: value.external_id,
        parent_filament_id: value.parent_filament_id,
        archived: value.archived,
        source_missing: value.source_missing,
    }
}

fn core_print_attempt_input(input: PrintAttemptInput) -> CorePrintAttemptInput {
    CorePrintAttemptInput {
        printed_at: input.printed_at,
        quantity: input.quantity,
        notes: input.notes,
        filament_ids: input.filament_ids,
        outcome: match input.outcome {
            PrintOutcome::Printed => CorePrintOutcome::Printed,
            PrintOutcome::Failed => CorePrintOutcome::Failed,
            PrintOutcome::Cancelled => CorePrintOutcome::Cancelled,
        },
        parts: input.parts.map(|parts| {
            parts
                .into_iter()
                .map(|part| CorePartUsageInput {
                    id: part.id,
                    quantity: part.quantity,
                    snapshot: part.snapshot.map(|snapshot| CorePartUsageSnapshot {
                        name: snapshot.name,
                        category: snapshot.category,
                        unit: snapshot.unit,
                    }),
                })
                .collect()
        }),
    }
}

fn core_part_stock_input(input: PartStockInput) -> CorePartStockInput {
    CorePartStockInput {
        name: input.name,
        category: input.category,
        quantity: input.quantity,
        unit: input.unit,
        notes: input.notes,
        low_stock: input.low_stock,
    }
}

fn part_stock(value: CorePartStock) -> PartStock {
    PartStock {
        id: value.id,
        metadata_version: value.metadata_version,
        name: value.name,
        category: value.category,
        quantity: value.quantity,
        unit: value.unit,
        notes: value.notes,
        low_stock: value.low_stock,
        created_at: value.created_at,
        updated_at: value.updated_at,
    }
}

fn core_print_attempt_input_with_printer(
    input: PrintAttemptInputWithPrinter,
) -> CorePrintAttemptInputWithPrinter {
    CorePrintAttemptInputWithPrinter {
        input: core_print_attempt_input(input.input),
        printer_id: input.printer_id,
    }
}

fn print_attempt(value: CorePrintAttempt) -> PrintAttempt {
    PrintAttempt {
        id: value.id,
        model_id: value.model_id,
        printed_at: value.printed_at,
        quantity: value.quantity,
        notes: value.notes,
        filament_ids: value.filament_ids,
        parts: value
            .parts
            .into_iter()
            .map(|part| PrintAttemptPart {
                id: part.id,
                name: part.name,
                category: part.category,
                unit: part.unit,
                quantity_per_copy: part.quantity_per_copy,
                quantity: part.quantity,
            })
            .collect(),
        outcome: match value.outcome {
            CorePrintOutcome::Printed => PrintOutcome::Printed,
            CorePrintOutcome::Failed => PrintOutcome::Failed,
            CorePrintOutcome::Cancelled => PrintOutcome::Cancelled,
        },
        stock_adjusted: value.stock_adjusted,
        printer_id: value.printer_id,
        printer_name: value.printer_name,
        created_at: value.created_at,
        updated_at: value.updated_at,
    }
}

fn catalog_model(model: Model, thumbnail_id: Option<String>) -> CatalogModel {
    let availability = if model.catalog_kind == "bookmark" {
        ModelAvailability::Bookmark
    } else if model.source_missing {
        ModelAvailability::Missing
    } else {
        ModelAvailability::Available
    };
    let print_status = match model.metadata.print_status {
        CorePrintStatus::Unprinted => "unprinted",
        CorePrintStatus::Want => "want",
        CorePrintStatus::Queued => "queued",
        CorePrintStatus::Printing => "printing",
        CorePrintStatus::Printed => "printed",
        CorePrintStatus::Failed => "failed",
    };
    CatalogModel {
        id: model.id,
        display_name: model.display_name,
        folder_id: model.folder_id,
        format: model.format,
        bytes: model.bytes,
        content_hash: model.content_hash,
        metadata_version: model.metadata_version,
        tags: model.tags,
        filament_ids: model.filament_ids,
        source_kind: model.source_kind,
        root_id: model.source_root_id,
        bundle_id: model.bundle_id,
        archive_member: model.source_archive_member.is_some(),
        parent_model_id: model.metadata.parent_model_id.clone(),
        created_at: model.created_at,
        updated_at: model.updated_at,
        last_printed_at: model.metadata.last_printed_at.clone(),
        print_count: model.metadata.print_count,
        rating: model.metadata.rating,
        favorite: model.metadata.favorite,
        availability,
        catalog_kind: model.catalog_kind,
        thumbnail_id,
        designer: model.metadata.designer,
        source_url: model.metadata.source_url,
        notes: model.metadata.notes,
        model_license: model.metadata.model_license,
        print_status: print_status.into(),
    }
}

fn catalog_stats(stats: CoreCatalogStats) -> CatalogStats {
    CatalogStats {
        total_models: stats.total_models,
        available_models: stats.available_models,
        missing_models: stats.missing_models,
        bookmarks: stats.bookmarks,
        total_bytes: stats.total_bytes,
        bundle_count: stats.bundle_count,
        duplicate_group_count: stats.duplicate_group_count,
    }
}

fn catalog_settings(settings: CoreCatalogSettings) -> CatalogSettings {
    CatalogSettings {
        thumbnail_size: settings.thumbnail_size,
        thumbnail_background: settings.thumbnail_background,
        thumbnail_concurrency: settings.thumbnail_concurrency,
        scan_batch_size: settings.scan_batch_size,
    }
}

fn catalog_facets(facets: CoreCatalogFacets) -> CatalogFacets {
    CatalogFacets {
        designers: facets.designers,
        licenses: facets.licenses,
        categories: facets.categories,
        parents: facets
            .parents
            .into_iter()
            .map(|value| CatalogFacetValue {
                id: value.id,
                label: value.label,
            })
            .collect(),
        folder_ids: facets.folder_ids,
        root_ids: facets.root_ids,
    }
}

fn capture_result(receipt: CoreCaptureReceipt, replayed_complete: bool) -> CaptureImportResult {
    let status = if replayed_complete {
        CaptureItemStatus::AlreadyImported
    } else {
        match receipt.status.as_str() {
            "imported" => CaptureItemStatus::Imported,
            "partial" => CaptureItemStatus::Partial,
            _ => CaptureItemStatus::Failed,
        }
    };
    CaptureImportResult {
        item_id: receipt.item_id,
        status,
        page_model_id: receipt.page_model_id,
        downloads: receipt
            .downloads
            .into_iter()
            .map(|download| CaptureDownloadResult {
                capture_id: download.capture_id,
                status: match download.status.as_str() {
                    "imported" => CaptureDownloadStatus::Imported,
                    "imported_removed" => CaptureDownloadStatus::AlreadyImportedRemoved,
                    "failed" => CaptureDownloadStatus::Failed,
                    _ => CaptureDownloadStatus::NeedsFile,
                },
                model_id: download.model_id,
                error_code: download.error_code,
            })
            .collect(),
    }
}

fn capture_summary(summary: CoreCaptureSummary) -> CaptureSummary {
    CaptureSummary {
        item_id: summary.item_id,
        created_at: summary.created_at,
        status: match summary.status.as_str() {
            "imported" => CaptureItemStatus::Imported,
            "failed" => CaptureItemStatus::Failed,
            _ => CaptureItemStatus::Partial,
        },
        page_model_id: summary.page_model_id,
        title: summary.title,
        url: summary.url,
        download_count: summary.download_count,
        failed_count: summary.failed_count,
        needs_file_count: summary.needs_file_count,
    }
}

fn scan_status(status: scanner::LibraryScanStatus, reveal_paths: bool) -> ScanStatus {
    ScanStatus {
        running: status.running,
        cancelling: status.cancelling,
        job_kind: status.job_kind,
        current_root_id: status.current_root_id,
        roots_total: status.roots_total,
        roots_completed: status.roots_completed,
        files_visited: status.files_visited,
        files_indexed: status.files_indexed,
        thumbnails_total: status.thumbnails_total,
        thumbnails_completed: status.thumbnails_completed,
        errors: if reveal_paths {
            status.errors
        } else {
            status
                .errors
                .into_iter()
                .map(|_| "A scan item failed; inspect the local desktop for details".into())
                .collect()
        },
    }
}

fn render_default_thumbnails_for_root(runtime: &Arc<LibraryRuntime>, root_id: &str) {
    let models = match runtime.read_db(|db| db.list_models(None, None)) {
        Ok(models) if models.len() <= 200_000 => models
            .into_iter()
            .filter(|model| {
                model.source_root_id.as_deref() == Some(root_id)
                    && !model.source_missing
                    && model.content_hash.is_some()
            })
            .collect::<Vec<_>>(),
        Ok(_) => {
            runtime
                .scan_control()
                .error("Thumbnail job exceeds its model limit".into());
            return;
        }
        Err(error) => {
            runtime.scan_control().error(format!(
                "Thumbnail job could not read catalog: {}",
                error.message
            ));
            return;
        }
    };
    runtime.scan_control().set_job_kind("thumbnails");
    runtime.scan_control().set_thumbnail_progress(models.len());
    for model in models {
        if runtime.scan_control().cancelled() {
            break;
        }
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        let result = render_default_thumbnail_for_model(runtime, &model, &mut context);
        if let Err(error) = result {
            runtime
                .scan_control()
                .error(format!("Thumbnail {}: {}", model.id, error.message));
        }
        runtime.scan_control().thumbnail_completed();
    }
    runtime.scan_control().set_job_kind("scan");
}

fn render_default_thumbnail_for_model(
    runtime: &LibraryRuntime,
    model: &Model,
    context: &mut PackageProcessingContext,
) -> Result<(), RuntimeError> {
    context
        .observer()
        .check()
        .map_err(|e| ProcessingError::from(e).into_runtime(invalid("Thumbnail render failed")))?;
    if runtime
        .read_db(|db| db.thumbnail_cache_entry_for_model(&model.id))?
        .is_some()
    {
        return Ok(());
    }
    let settings = runtime.read_db(|db| db.get_catalog_settings())?;
    let content_hash = model
        .content_hash
        .as_deref()
        .ok_or_else(|| invalid("Thumbnail model has no content hash"))?;
    let bytes = runtime.verified_model_source_bytes_in_context(model, context)?;
    let artifact = thumbnails::ensure_thumbnail_from_bytes_in_context_typed(
        &runtime.root,
        &bytes,
        content_hash,
        bytes.len() as u64,
        &settings,
        false,
        context,
    )
    .map_err(|error| error.into_runtime(invalid("Thumbnail render failed")))?;
    context
        .observer()
        .check()
        .map_err(|e| ProcessingError::from(e).into_runtime(invalid("Thumbnail render failed")))?;
    runtime.mutate_db(|db| {
        db.set_thumbnail_for_source_and_settings(
            &model.id,
            content_hash,
            &settings,
            &artifact.png_hash,
            &artifact.relative_path,
        )
    })?;
    Ok(())
}

fn scan_error(message: String) -> RuntimeError {
    RuntimeError {
        code: if message.contains("already running") {
            RuntimeErrorCode::Busy
        } else if message.contains("maintenance") {
            RuntimeErrorCode::Maintenance
        } else {
            RuntimeErrorCode::InvalidInput
        },
        message,
        retryable: true,
        current_version: None,
        source_holds: Vec::new(),
    }
}

fn safe_download_name(display_name: &str, source_name: &Path) -> String {
    let mut stem = display_name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, ' ' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    stem = stem.trim_matches([' ', '.']).to_owned();
    if stem.is_empty() {
        stem = "model".into();
    }
    let extension = source_name
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| {
            extension.eq_ignore_ascii_case("stl") || extension.eq_ignore_ascii_case("3mf")
        })
        .unwrap_or("stl")
        .to_ascii_lowercase();
    format!("{stem}.{extension}")
}

fn validate_upload_intent(intent: &UploadIntent) -> Result<(), RuntimeError> {
    for (value, label) in [
        (intent.item_id.as_str(), "Upload item ID"),
        (intent.capture_id.as_str(), "Upload capture ID"),
    ] {
        validate_plain_id(value, label)?;
    }
    if intent.file_name.is_empty()
        || intent.file_name.len() > 255
        || intent.file_name.contains(['/', '\\', '\0'])
        || intent.bytes == 0
        || intent.bytes > library_core::MAX_INDEXED_FILE_BYTES
        || intent.sha256.len() != 64
        || !intent.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid("Upload intent is invalid"));
    }
    Ok(())
}

fn format_system_time(value: SystemTime) -> Result<String, RuntimeError> {
    time::OffsetDateTime::from(value)
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| internal("Timestamp could not be formatted"))
}

fn verify_content_file(
    path: &Path,
    expected_bytes: u64,
    expected_hash: &str,
) -> Result<(), RuntimeError> {
    read_verified_content(path, expected_bytes, expected_hash).map(|_| ())
}

fn content_limit(kind: ContentKind) -> u64 {
    match kind {
        ContentKind::Thumbnail => 4 * 1024 * 1024,
        ContentKind::Model => library_core::MAX_INDEXED_FILE_BYTES,
        ContentKind::Geometry => 128 * 1024 * 1024,
        ContentKind::BundleArchive => 4 * 1024 * 1024 * 1024,
    }
}

fn verify_leased_file(
    path: &Path,
    expected_bytes: u64,
    expected_hash: &str,
    max_bytes: u64,
) -> Result<(), RuntimeError> {
    stream_verified_file(
        path,
        expected_bytes,
        expected_hash,
        max_bytes,
        &mut std::io::sink(),
    )
}

fn stream_verified_file(
    path: &Path,
    expected_bytes: u64,
    expected_hash: &str,
    max_bytes: u64,
    sink: &mut dyn Write,
) -> Result<(), RuntimeError> {
    if expected_bytes > max_bytes {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Unsupported,
            message: "Content exceeds its lease type limit".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| io_error())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != expected_bytes
    {
        return Err(stale("Leased content file changed"));
    }
    let mut file = File::open(path).map_err(|_| io_error())?;
    let mut digest = Sha256::new();
    let mut read = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|_| io_error())?;
        if count == 0 {
            break;
        }
        read = read
            .checked_add(count as u64)
            .ok_or_else(|| stale("Leased content size overflowed"))?;
        if read > expected_bytes {
            return Err(stale("Leased content size changed"));
        }
        digest.update(&buffer[..count]);
        sink.write_all(&buffer[..count]).map_err(|_| io_error())?;
    }
    if read != expected_bytes || format!("{:x}", digest.finalize()) != expected_hash {
        return Err(stale("Leased content bytes changed"));
    }
    Ok(())
}

fn read_verified_content(
    path: &Path,
    expected_bytes: u64,
    expected_hash: &str,
) -> Result<Vec<u8>, RuntimeError> {
    let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
    read_verified_content_in_context(path, expected_bytes, expected_hash, &mut context)
}

fn read_verified_content_in_context(
    path: &Path,
    expected_bytes: u64,
    expected_hash: &str,
    context: &mut PackageProcessingContext,
) -> Result<Vec<u8>, RuntimeError> {
    let observer = context.observer();
    observer
        .check()
        .map_err(|e| ProcessingError::from(e).into_runtime(io_error()))?;
    if expected_bytes > library_core::MAX_INDEXED_FILE_BYTES {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Unsupported,
            message: "Model source exceeds the runtime download limit".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        });
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| io_error())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != expected_bytes
    {
        let _ = observer.stop_io(stl_core::ResourceStop::EnvelopeChanged);
        return Err(stale("Model source file changed"));
    }
    let mut file = File::open(path).map_err(|_| io_error())?;
    let bytes = collect_verified_model_source(&mut file, expected_bytes, &observer)
        .map_err(|e| ProcessingError::from(e).into_runtime(io_error()))?;
    if bytes.len() as u64 != expected_bytes
        || format!("{:x}", Sha256::digest(&bytes)) != expected_hash
    {
        let _ = observer.stop_io(stl_core::ResourceStop::EnvelopeChanged);
        return Err(stale("Model source bytes changed"));
    }
    Ok(bytes)
}

fn invalid(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::InvalidInput,
        message: message.into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    }
}

fn io_error() -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Io,
        message: "Library filesystem operation failed".into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    }
}

fn maintenance_error() -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Maintenance,
        message: "Library maintenance is in progress".into(),
        retryable: true,
        current_version: None,
        source_holds: Vec::new(),
    }
}

fn stale(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Stale,
        message: message.into(),
        retryable: true,
        current_version: None,
        source_holds: Vec::new(),
    }
}

fn internal(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Internal,
        message: message.into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use library_protocol::{
        BundleEngine, CaptureDownload, CaptureEngine, CaptureEnvelope, CaptureItemStatus,
        CapturePage, CatalogEngine, ContentEngine, DownloadEngine, FilamentEngine, GalleryEngine,
        MetadataEngine, ModelQuery, PageRequest, RuntimeStatusEngine, UploadEngine, UploadIntent,
        CAPTURE_ENVELOPE_VERSION,
    };

    pub(super) fn nested_package_fixture(
        temp: &tempfile::TempDir,
        method: zip::CompressionMethod,
    ) -> (Arc<LibraryRuntime>, Model, Vec<u8>, u64) {
        use library_core::{ScannedModel, SourceRef};
        let sources = temp.path().join("nested-sources");
        fs::create_dir(&sources).unwrap();
        let inner = metadata_3mf();
        let model_xml_len = zip::ZipArchive::new(std::io::Cursor::new(&inner))
            .unwrap()
            .by_name("3D/3dmodel.model")
            .unwrap()
            .size();
        let outer = sources.join("models.zip");
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file(
                "part.3mf",
                zip::write::SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
        writer.write_all(&inner).unwrap();
        fs::write(&outer, writer.finish().unwrap().into_inner()).unwrap();
        let inventory =
            archives::archive_inventory_with_cancel(&outer, 1024 * 1024, &|| false).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| {
                let root = db.add_index_root(&sources, 60, 16 * 1024 * 1024)?;
                let added = db.record_scan_entries(
                    &root.id,
                    &[ScannedModel {
                        relative_path: "models.zip".into(),
                        content_hash: format!("{:x}", Sha256::digest(&inner)),
                        bytes: inner.len() as u64,
                        format: "3mf".into(),
                        modified_at: 1,
                        display_name: None,
                        archive_member: Some("part.3mf".into()),
                        outer_fingerprint: Some(inventory.outer_fingerprint.clone()),
                    }],
                )?;
                db.complete_scan_sources_with_new_models(
                    &root.id,
                    &[SourceRef {
                        relative_path: "models.zip".into(),
                        archive_member: Some("part.3mf".into()),
                    }],
                    &added.added_model_ids,
                    "1",
                )?;
                Ok(db.get_model(&added.added_model_ids[0])?.unwrap())
            })
            .unwrap();
        (runtime, model, inner, model_xml_len)
    }

    #[test]
    fn verified_source_and_default_thumbnail_keep_nested_validation_work() {
        use stl_core::package_bounds::PackageUsage;
        for method in [
            zip::CompressionMethod::Stored,
            zip::CompressionMethod::Deflated,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let (runtime, model, inner, model_xml_len) = nested_package_fixture(&temp, method);
            let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
            let bytes = runtime
                .verified_model_source_bytes_in_context(&model, &mut context)
                .unwrap();
            assert_eq!(bytes, inner);
            assert_eq!(
                context.usage(),
                PackageUsage {
                    inflation_bytes: model_xml_len,
                    retained_input_bytes: 0,
                },
                "the archived source's hidden validation must spend model work"
            );
            stl_core::parse_model_in_context(&bytes, &mut context).unwrap();
            assert_eq!(context.usage().inflation_bytes, 2 * model_xml_len);
            let mut render_context = PackageProcessingContext::new(ZipResourceObserver::new());
            render_default_thumbnail_for_model(&runtime, &model, &mut render_context).unwrap();
            assert_eq!(
                render_context.usage(),
                PackageUsage {
                    inflation_bytes: 2 * model_xml_len,
                    retained_input_bytes: 0,
                },
                "default job must keep validation and geometry on the same counter"
            );
            assert!(runtime
                .read_db(|db| db.thumbnail_cache_entry_for_model(&model.id))
                .unwrap()
                .is_some());
            // A cached return still cannot hide a stop already recorded by this operation.
            let _ = render_context
                .observer()
                .stop_io(stl_core::ResourceStop::ReadWorkExceeded);
            let error = render_default_thumbnail_for_model(&runtime, &model, &mut render_context)
                .unwrap_err();
            assert_eq!(error.code, RuntimeErrorCode::Unsupported);
            assert!(!error.retryable);
        }
    }

    #[test]
    fn plain_verified_source_leaves_package_usage_unchanged_and_latches_changed_hash() {
        use stl_core::package_bounds::PackageUsage;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("source.stl");
        let bytes = triangle_stl(0.0);
        fs::write(&path, &bytes).unwrap();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
        assert_eq!(
            read_verified_content_in_context(&path, bytes.len() as u64, &hash, &mut context)
                .unwrap(),
            bytes
        );
        assert_eq!(
            context.usage(),
            PackageUsage {
                inflation_bytes: 0,
                retained_input_bytes: 0
            }
        );
        let error = read_verified_content_in_context(
            &path,
            bytes.len() as u64,
            &"0".repeat(64),
            &mut context,
        )
        .unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::Stale);
        assert!(error.retryable);
        assert!(matches!(
            context.observer().check(),
            Err(stl_core::zip_bounds::ZipOpenError::ResourceStopped(
                stl_core::ResourceStop::EnvelopeChanged
            ))
        ));
        // The same terminal state wins before even looking up this absent path.
        let later =
            read_verified_content_in_context(&temp.path().join("absent"), 0, "", &mut context)
                .unwrap_err();
        assert_eq!(later.code, RuntimeErrorCode::Stale);
        assert!(later.retryable);
    }

    #[test]
    fn archive_resource_stop_is_not_stale_or_corrupt_and_issues_no_download() {
        use library_core::{ScannedModel, SourceRef};

        let temp = tempfile::tempdir().unwrap();
        let sources = temp.path().join("sources");
        fs::create_dir(&sources).unwrap();
        let outer = sources.join("models.zip");
        let archive_bytes = |padding_entries: usize, offset: f32| {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            writer.start_file("part.stl", options).unwrap();
            writer.write_all(&triangle_stl(offset)).unwrap();
            for index in 0..padding_entries {
                writer
                    .start_file(format!("padding-{index}.txt"), options)
                    .unwrap();
            }
            writer.finish().unwrap().into_inner()
        };
        let original = archive_bytes(0, 0.0);
        fs::write(&outer, &original).unwrap();
        let inventory =
            archives::archive_inventory_with_cancel(&outer, 1024 * 1024, &|| false).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| {
                let root = db.add_index_root(&sources, 60, 16 * 1024 * 1024)?;
                let added = db.record_scan_entries(
                    &root.id,
                    &[ScannedModel {
                        relative_path: "models.zip".into(),
                        content_hash: format!("{:x}", Sha256::digest(triangle_stl(0.0))),
                        bytes: triangle_stl(0.0).len() as u64,
                        format: "stl-binary".into(),
                        modified_at: 1,
                        display_name: None,
                        archive_member: Some("part.stl".into()),
                        outer_fingerprint: Some(inventory.outer_fingerprint.clone()),
                    }],
                )?;
                db.complete_scan_sources_with_new_models(
                    &root.id,
                    &[SourceRef {
                        relative_path: "models.zip".into(),
                        archive_member: Some("part.stl".into()),
                    }],
                    &added.added_model_ids,
                    "1",
                )?;
                Ok(db.get_model(&added.added_model_ids[0])?.unwrap())
            })
            .unwrap();
        let context = RequestContext {
            principal_id: "archive-resource-reader".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "archive-resource-stop".into(),
        };
        let staging_entries = || {
            fs::read_dir(runtime.root.join("staging"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<BTreeSet<_>>()
        };
        let before = staging_entries();
        let oversized_directory = archive_bytes(10_000, 0.0);
        fs::write(&outer, &oversized_directory).unwrap();
        for error in [
            runtime.verified_model_source_bytes(&model).unwrap_err(),
            runtime
                .issue_model_download(&context, &model.id, None)
                .unwrap_err(),
        ] {
            assert_eq!(error.code, RuntimeErrorCode::Unsupported);
            assert!(!error.retryable);
            assert_eq!(
                error.message,
                stl_core::ResourceStop::InputLimitExceeded.to_string()
            );
        }
        assert!(runtime.leases.lock().unwrap().is_empty());
        assert_eq!(runtime.temp_resources.lock().unwrap().owned_bytes, 0);
        assert_eq!(staging_entries(), before);
        assert_eq!(fs::read(&outer).unwrap(), oversized_directory);

        // A genuine changed source within the metadata budget is still Stale.
        let changed = archive_bytes(0, 9.0);
        fs::write(&outer, &changed).unwrap();
        let error = runtime
            .issue_model_download(&context, &model.id, None)
            .unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::Stale);
        assert!(error.retryable);
        assert!(runtime.leases.lock().unwrap().is_empty());
        assert_eq!(staging_entries(), before);
        assert_eq!(fs::read(&outer).unwrap(), changed);
    }

    #[test]
    fn protocol_print_history_cannot_set_local_creation_time() {
        let protocol_input: PrintAttemptInput = serde_json::from_value(serde_json::json!({
            "printedAt": "2026-10-10T12:00:00Z",
            "createdAt": "2020-01-01T00:00:00Z",
            "quantity": 1,
            "notes": "external protocol input",
            "filamentIds": [],
            "outcome": "printed"
        }))
        .unwrap();
        let serialized = serde_json::to_value(&protocol_input).unwrap();
        assert!(serialized.get("createdAt").is_none());

        let local_input = core_print_attempt_input(protocol_input);
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Protocol print history", None).unwrap();
        let attempt = db.create_print_attempt(&model.id, &local_input).unwrap();
        assert_ne!(attempt.created_at, "2020-01-01T00:00:00Z");
        assert_eq!(attempt.created_at, attempt.updated_at);
    }

    #[test]
    fn print_history_printer_patch_round_trips_and_unknown_ids_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let db = LibraryDb::open(&root).unwrap();
        let model = db.create_model("Printer-attributed print", None).unwrap();
        let printer = db
            .create_printer(
                &library_core::models::PrinterInput {
                    nickname: "Workshop printer".into(),
                    make: None,
                    model: None,
                    printer_type: None,
                    firmware: None,
                    host: None,
                    web_url: None,
                    bed_mm: None,
                    notes: String::new(),
                },
                false,
            )
            .unwrap();
        drop(db);

        let runtime = LibraryRuntime::open(config(root)).unwrap();
        let context = RequestContext {
            principal_id: "test".into(),
            capabilities: [Capability::CatalogRead, Capability::CatalogWrite]
                .into_iter()
                .collect(),
            request_id: "printer-history-test".into(),
        };
        let linked: PrintAttemptInputWithPrinter = serde_json::from_value(serde_json::json!({
            "printedAt": "2026-10-14T12:00:00Z",
            "quantity": 1,
            "notes": "Initial",
            "filamentIds": [],
            "outcome": "printed",
            "printerId": printer.id
        }))
        .unwrap();
        let created = runtime
            .create_print_attempt_with_printer(&context, &model.id, linked)
            .unwrap();
        assert_eq!(created.printer_id.as_deref(), Some(printer.id.as_str()));
        assert_eq!(created.printer_name.as_deref(), Some("Workshop printer"));

        let omitted: PrintAttemptInputWithPrinter = serde_json::from_value(serde_json::json!({
            "printedAt": "2026-10-14T12:00:00Z",
            "quantity": 2,
            "notes": "Keep printer",
            "filamentIds": [],
            "outcome": "printed"
        }))
        .unwrap();
        let preserved = runtime
            .update_print_attempt_with_printer(&context, &created.id, omitted)
            .unwrap();
        assert_eq!(preserved.printer_id.as_deref(), Some(printer.id.as_str()));
        assert_eq!(preserved.printer_name.as_deref(), Some("Workshop printer"));

        let invalid_link: PrintAttemptInputWithPrinter =
            serde_json::from_value(serde_json::json!({
                "printedAt": "2026-10-14T12:00:00Z",
                "quantity": 3,
                "notes": "Must roll back",
                "filamentIds": [],
                "outcome": "printed",
                "printerId": "not-a-catalog-printer"
            }))
            .unwrap();
        let error = runtime
            .update_print_attempt_with_printer(&context, &created.id, invalid_link)
            .unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::InvalidInput);
        let unchanged = runtime
            .list_print_attempts(&context, &model.id)
            .unwrap()
            .remove(0);
        assert_eq!(unchanged.notes, "Keep printer");
        assert_eq!(unchanged.printer_id.as_deref(), Some(printer.id.as_str()));

        let clear: PrintAttemptInputWithPrinter = serde_json::from_value(serde_json::json!({
            "printedAt": "2026-10-14T12:00:00Z",
            "quantity": 2,
            "notes": "Clear printer",
            "filamentIds": [],
            "outcome": "printed",
            "printerId": null
        }))
        .unwrap();
        let cleared = runtime
            .update_print_attempt_with_printer(&context, &created.id, clear)
            .unwrap();
        assert_eq!(cleared.printer_id, None);
        assert_eq!(cleared.printer_name, None);
    }

    #[test]
    fn part_stock_protocol_gates_writes_and_preserves_transactional_event_usage() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let runtime = LibraryRuntime::open(config(root)).unwrap();
        let model = runtime
            .mutate_db(|db| db.create_model("Parts protocol model", None))
            .unwrap();
        let writer = RequestContext {
            principal_id: "parts-writer".into(),
            capabilities: [Capability::CatalogRead, Capability::CatalogWrite]
                .into_iter()
                .collect(),
            request_id: "parts-writer".into(),
        };
        let reader = RequestContext {
            principal_id: "parts-reader".into(),
            capabilities: [Capability::CatalogRead].into_iter().collect(),
            request_id: "parts-reader".into(),
        };
        let stock = runtime
            .create_part_stock(
                &writer,
                PartStockInput {
                    name: "M3 heat-set insert".into(),
                    category: Some("hardware".into()),
                    quantity: 10,
                    unit: "pcs".into(),
                    notes: "brass".into(),
                    low_stock: 2,
                },
            )
            .unwrap();
        assert_eq!(stock.metadata_version, 1);
        assert_eq!(
            runtime.list_part_stock(&reader).unwrap(),
            vec![stock.clone()]
        );
        assert_eq!(
            runtime
                .create_part_stock(
                    &reader,
                    PartStockInput {
                        name: "forbidden".into(),
                        category: None,
                        quantity: 0,
                        unit: "pcs".into(),
                        notes: String::new(),
                        low_stock: 0,
                    },
                )
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );

        let cancelled: PrintAttemptInput = serde_json::from_value(serde_json::json!({
            "printedAt": "2026-10-15T12:00:00Z",
            "quantity": 2,
            "notes": "cancelled during setup",
            "filamentIds": [],
            "outcome": "cancelled",
            "parts": [{"id": stock.id, "quantity": 3}]
        }))
        .unwrap();
        let created = runtime
            .create_print_attempt(&writer, &model.id, cancelled)
            .unwrap();
        assert_eq!(created.outcome, PrintOutcome::Cancelled);
        assert_eq!(created.parts[0].quantity_per_copy, 3);
        assert_eq!(created.parts[0].quantity, 6);
        assert_eq!(runtime.list_part_stock(&reader).unwrap()[0].quantity, 4);

        let edit_without_parts: PrintAttemptInput = serde_json::from_value(serde_json::json!({
            "printedAt": "2026-10-15T12:00:00Z",
            "quantity": 1,
            "notes": "copy count corrected",
            "filamentIds": [],
            "outcome": "cancelled"
        }))
        .unwrap();
        let updated = runtime
            .update_print_attempt(&writer, &created.id, edit_without_parts)
            .unwrap();
        assert_eq!(updated.parts[0].quantity, 3);
        assert_eq!(runtime.list_part_stock(&reader).unwrap()[0].quantity, 7);

        let batch_input: PrintAttemptInput = serde_json::from_value(serde_json::json!({
            "printedAt": "2026-10-15T13:00:00Z",
            "quantity": 2,
            "notes": "batch print",
            "filamentIds": [],
            "outcome": "printed",
            "parts": [{"id": stock.id, "quantity": 1}]
        }))
        .unwrap();
        let batch_item = PrintAttemptBatchItem {
            model_id: model.id.clone(),
            input: batch_input,
        };
        let first = runtime
            .create_print_attempts(&writer, "parts-replay-key", vec![batch_item.clone()])
            .unwrap();
        let replay = runtime
            .create_print_attempts(&writer, "parts-replay-key", vec![batch_item])
            .unwrap();
        assert_eq!(first, replay);
        assert_eq!(runtime.list_part_stock(&reader).unwrap()[0].quantity, 5);

        runtime.delete_print_attempt(&writer, &created.id).unwrap();
        runtime.delete_print_attempt(&writer, &first[0].id).unwrap();
        assert_eq!(runtime.list_part_stock(&reader).unwrap()[0].quantity, 10);
    }

    #[test]
    fn ordinary_print_history_input_ignores_import_creation_time() {
        let input: CorePrintAttemptInput = serde_json::from_value(serde_json::json!({
            "printed_at": "2026-10-10T12:00:00Z",
            "created_at": "2000-01-01T00:00:00Z",
            "quantity": 1,
            "notes": "ordinary local create",
            "filament_ids": [],
            "outcome": "printed"
        }))
        .unwrap();
        assert!(serde_json::to_value(&input)
            .unwrap()
            .get("created_at")
            .is_none());

        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Ordinary print history", None).unwrap();
        let attempt = db.create_print_attempt(&model.id, &input).unwrap();
        assert_ne!(attempt.created_at, "2000-01-01T00:00:00Z");
        assert_eq!(attempt.created_at, attempt.updated_at);
    }

    fn config(root: PathBuf) -> RuntimeConfig {
        let settings = root
            .parent()
            .unwrap()
            .join("runtime-fixture-integrations.json");
        RuntimeConfig::desktop(root, settings)
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn runtime_rejects_catalog_path_replacement_after_open() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let runtime = LibraryRuntime::open(config(root.clone())).unwrap();
        runtime
            .mutate_db(|db| db.create_model("Original catalog", None))
            .unwrap();

        let catalog = root.join("library.sqlite3");
        let displaced = root.join("library.sqlite3.displaced");
        std::fs::rename(&catalog, &displaced).unwrap();
        drop(LibraryDb::open(&root).unwrap());

        let result = runtime.with_db::<(), RuntimeError>(|_| Ok(()));
        assert!(matches!(
            result,
            Err(RuntimeError {
                code: RuntimeErrorCode::Stale,
                ..
            })
        ));
    }

    #[test]
    fn backup_is_atomically_published_and_never_overwrites_existing_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        drop(LibraryDb::open(&root).unwrap());
        let backup_directory = temp.path().join("backups");
        std::fs::create_dir(&backup_directory).unwrap();
        let destination = backup_directory.join("library.zip");

        let result = create_backup_file(&root, &destination).unwrap();
        assert_eq!(
            hash_management_file(&destination, library_backup::MAX_BACKUP_ARCHIVE_BYTES)
                .unwrap()
                .1,
            result.sha256
        );
        assert_eq!(std::fs::read_dir(&backup_directory).unwrap().count(), 1);

        let original = std::fs::read(&destination).unwrap();
        assert!(create_backup_file(&root, &destination).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), original);
        assert_eq!(std::fs::read_dir(&backup_directory).unwrap().count(), 1);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn backup_asset_source_stays_in_its_open_root_after_path_swap() {
        use library_backup::BackupAssetSource as _;

        let temp = tempfile::tempdir().unwrap();
        let library_path = temp.path().join("library");
        drop(LibraryDb::open(&library_path).unwrap());
        let library = library_path.canonicalize().unwrap();
        let original_asset = library.join("originals/owned.stl");
        std::fs::write(&original_asset, b"original managed bytes").unwrap();

        let outside = temp.path().join("outside");
        std::fs::create_dir_all(outside.join("originals")).unwrap();
        std::fs::write(outside.join("originals/decoy.stl"), b"outside sentinel").unwrap();

        let source = GuardedBackupAssetSource {
            directory: secure_fs::Directory::open(&library).unwrap(),
        };
        let displaced = temp.path().join("library.displaced");
        std::fs::rename(&library, &displaced).unwrap();
        std::os::unix::fs::symlink(&outside, &library).unwrap();

        let paths = source.asset_paths().unwrap();
        assert!(paths.contains(&PathBuf::from("originals/owned.stl")));
        assert!(!paths.contains(&PathBuf::from("originals/decoy.stl")));
        let mut opened = source.open_asset(Path::new("originals/owned.stl")).unwrap();
        let mut bytes = Vec::new();
        opened.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"original managed bytes");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn backup_asset_walk_bounds_directory_entries_and_depth() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        drop(LibraryDb::open(&library).unwrap());
        std::fs::write(library.join("originals/a.stl"), b"a").unwrap();
        std::fs::write(library.join("originals/b.stl"), b"b").unwrap();
        std::fs::create_dir_all(library.join("manuals/one/two/three")).unwrap();
        let source = GuardedBackupAssetSource {
            directory: secure_fs::Directory::open(&library.canonicalize().unwrap()).unwrap(),
        };

        assert!(source.asset_paths_with_limits(1, 64, 1024).is_err());
        assert!(source.asset_paths_with_limits(100, 1, 1024).is_err());
        assert!(source.asset_paths_with_limits(100, 64, 1).is_err());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn oversized_backup_retains_the_handle_created_temporary_file_in_a_recovery_hold() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        drop(LibraryDb::open(&root).unwrap());
        let backup_directory = temp.path().join("backups");
        std::fs::create_dir(&backup_directory).unwrap();
        let destination = backup_directory.join("library.zip");

        assert!(create_backup_file_with_limit(&root, &destination, 32).is_err());
        assert!(!destination.exists());
        let names = std::fs::read_dir(&backup_directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        let hold = names
            .iter()
            .find(|name| name.to_string_lossy().ends_with(".data"))
            .expect("temporary archive bytes should remain recoverable");
        assert!(!std::fs::read(backup_directory.join(hold))
            .unwrap()
            .is_empty());
        let journal = names
            .iter()
            .find(|name| name.to_string_lossy().ends_with(".journal"))
            .expect("temporary archive hold should have a durable journal");
        assert!(backup_directory
            .join(format!("{}.verified", journal.to_string_lossy()))
            .exists());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn backup_rename_preserves_a_destination_created_after_preflight() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        drop(LibraryDb::open(&root).unwrap());
        let backup_directory = temp.path().join("backups");
        std::fs::create_dir(&backup_directory).unwrap();
        let destination = backup_directory.join("library.zip");
        let sentinel = b"created after backup preflight";

        let result = create_backup_file_with_limit_and_hook(
            &root,
            &destination,
            library_backup::MAX_BACKUP_ARCHIVE_BYTES,
            || std::fs::write(&destination, sentinel).unwrap(),
        );

        assert!(result.is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), sentinel);
        let names = std::fs::read_dir(&backup_directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            names
                .iter()
                .filter(|name| name.to_string_lossy() == "library.zip")
                .count(),
            1
        );
        let hold = names
            .iter()
            .find(|name| name.to_string_lossy().ends_with(".data"))
            .expect("failed backup bytes should remain recoverable");
        assert!(!std::fs::read(backup_directory.join(hold))
            .unwrap()
            .is_empty());
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn backup_fallback_keeps_the_path_based_no_clobber_behavior() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        drop(LibraryDb::open(&root).unwrap());
        let backup_directory = temp.path().join("backups");
        std::fs::create_dir(&backup_directory).unwrap();
        let destination = backup_directory.join("library.zip");

        let result = create_backup_file(&root, &destination).unwrap();
        assert!(destination.is_file());
        assert_eq!(
            hash_management_file(&destination, library_backup::MAX_BACKUP_ARCHIVE_BYTES)
                .unwrap()
                .1,
            result.sha256
        );
        assert!(create_backup_file(&root, &destination).is_err());
    }

    #[test]
    fn library_json_export_includes_safe_printer_maintenance_and_reminder_records() {
        let db = LibraryDb::open_in_memory().unwrap();
        let printer = db
            .create_printer(
                &library_core::models::PrinterInput {
                    nickname: "Workshop P1".into(),
                    make: Some("Prusa Research".into()),
                    model: Some("MK4S".into()),
                    printer_type: Some("FDM".into()),
                    firmware: Some("Prusa firmware".into()),
                    host: Some("192.0.2.10".into()),
                    web_url: Some("http://192.0.2.10/".into()),
                    bed_mm: Some([250.0, 210.0, 220.0]),
                    notes: "Nozzle serviced".into(),
                },
                true,
            )
            .unwrap();
        let maintenance_log = db
            .create_printer_maintenance_log(
                &printer.id,
                &library_core::models::PrinterMaintenanceInput {
                    title: "Replace nozzle".into(),
                    performed_at: "2026-10-12T12:00:00Z".into(),
                    notes: "Installed 0.4 mm nozzle".into(),
                },
            )
            .unwrap();
        let reminder = db
            .create_printer_reminder(
                &printer.id,
                &library_core::models::PrinterReminderInput {
                    title: "Clean build plate".into(),
                    notes: "Use mild soap".into(),
                    due_at: "2026-10-20T12:00:00Z".into(),
                    interval_days: Some(30),
                },
            )
            .unwrap();

        let (export, model_count) =
            build_library_export_value(&db, "2026-10-14T00:00:00Z").unwrap();

        let assert_rfc3339 = |value: &serde_json::Value, field: &str| {
            let timestamp = value.as_str().unwrap_or_default();
            assert!(
                time::OffsetDateTime::parse(
                    timestamp,
                    &time::format_description::well_known::Rfc3339
                )
                .is_ok(),
                "{field} must be RFC3339, got {timestamp}"
            );
        };
        assert_eq!(model_count, 0);
        let exported_printer = &export["printers"][0];
        assert_eq!(exported_printer["gogglelab_printer_id"], printer.id);
        assert_eq!(exported_printer["nickname"], "Workshop P1");
        assert_eq!(exported_printer["manufacturer"], "Prusa Research");
        assert_eq!(exported_printer["model"], "MK4S");
        assert_eq!(exported_printer["printer_type"], "FDM");
        assert_eq!(exported_printer["firmware_type"], "Prusa firmware");
        assert_eq!(exported_printer["host"], "192.0.2.10");
        assert_eq!(exported_printer["web_url"], "http://192.0.2.10");
        assert_eq!(exported_printer["source_kind"], "manual");
        assert_rfc3339(&exported_printer["created_at"], "printer created_at");
        assert_rfc3339(&exported_printer["updated_at"], "printer updated_at");
        assert_eq!(
            exported_printer["bed_mm"],
            serde_json::json!([250.0, 210.0, 220.0])
        );
        assert_eq!(exported_printer["notes"], "Nozzle serviced");
        for excluded in [
            "credentials",
            "password",
            "api_key",
            "token",
            "endpoint_approved",
            "connection_status",
            "job_status",
            "last_checked_at",
            "connection_error_code",
        ] {
            assert!(
                exported_printer.get(excluded).is_none(),
                "unexpected {excluded}"
            );
        }

        let exported_log = &export["printer_maintenance_logs"][0];
        assert_eq!(
            exported_log["gogglelab_maintenance_log_id"],
            maintenance_log.id
        );
        assert_eq!(exported_log["gogglelab_printer_id"], printer.id);
        assert_eq!(
            exported_log["printer_nickname"],
            maintenance_log.printer_nickname
        );
        assert_eq!(exported_log["title"], "Replace nozzle");
        assert_eq!(exported_log["performed_at"], "2026-10-12T12:00:00.000Z");
        assert_eq!(exported_log["notes"], "Installed 0.4 mm nozzle");
        assert_rfc3339(&exported_log["created_at"], "maintenance log created_at");
        assert!(exported_log.get("printer_id").is_none());

        let exported_reminder = &export["printer_maintenance_reminders"][0];
        assert_eq!(
            exported_reminder["gogglelab_printer_reminder_id"],
            reminder.id
        );
        assert_eq!(exported_reminder["gogglelab_printer_id"], printer.id);
        assert_eq!(
            exported_reminder["printer_nickname"],
            reminder.printer_nickname
        );
        assert_eq!(exported_reminder["title"], "Clean build plate");
        assert_eq!(exported_reminder["notes"], "Use mild soap");
        assert_eq!(exported_reminder["due_at"], "2026-10-20T12:00:00.000Z");
        assert_eq!(exported_reminder["interval_days"], 30);
        assert_eq!(exported_reminder["status"], "active");
        assert_eq!(
            exported_reminder["last_completed_at"],
            serde_json::Value::Null
        );
        assert_rfc3339(&exported_reminder["created_at"], "reminder created_at");
        assert_rfc3339(&exported_reminder["updated_at"], "reminder updated_at");
        assert!(exported_reminder.get("printer_id").is_none());

        let mut writer = CappedJsonWriter::new(Vec::new(), MAX_PRINTVENTORY_JSON_BYTES);
        stream_library_json_export(&db, "2026-10-15T00:00:00Z", &mut writer).unwrap();
        let streamed: serde_json::Value = serde_json::from_slice(&writer.into_inner()).unwrap();
        assert_rfc3339(
            &streamed["printers"][0]["created_at"],
            "streamed printer created_at",
        );
        assert_rfc3339(
            &streamed["printers"][0]["updated_at"],
            "streamed printer updated_at",
        );
        assert_rfc3339(
            &streamed["printer_maintenance_logs"][0]["created_at"],
            "streamed maintenance log created_at",
        );
        assert_rfc3339(
            &streamed["printer_maintenance_reminders"][0]["created_at"],
            "streamed reminder created_at",
        );
        assert_rfc3339(
            &streamed["printer_maintenance_reminders"][0]["updated_at"],
            "streamed reminder updated_at",
        );
    }

    #[test]
    fn imported_printer_history_round_trips_through_the_printventory_json_export() {
        let db = LibraryDb::open_in_memory().unwrap();
        let part = db
            .create_part_stock(&CorePartStockInput {
                name: "M3 heat-set insert".into(),
                category: Some("hardware".into()),
                quantity: 10,
                unit: "pcs".into(),
                notes: "brass".into(),
                low_stock: 2,
            })
            .unwrap();
        let mut printer_data = library_core::models::PrinterDataImport::default();
        printer_data
            .printers
            .push(library_core::models::PrinterDataImportPrinter {
                external_ref: "external-printer-1".into(),
                map_to_local_printer_id: None,
                input: library_core::models::PrinterInput {
                    nickname: "Round-trip printer".into(),
                    make: Some("Fixture maker".into()),
                    model: Some("Fixture model".into()),
                    printer_type: Some("FDM".into()),
                    firmware: None,
                    host: None,
                    web_url: None,
                    bed_mm: Some([220.0, 220.0, 250.0]),
                    notes: "Imported for round-trip verification".into(),
                },
                created_at: Some("2026-10-13T11:00:00Z".into()),
                updated_at: Some("2026-10-14T10:00:00Z".into()),
            });
        printer_data
            .maintenance_logs
            .push(library_core::models::PrinterDataImportMaintenanceLog {
                external_ref: "external-maintenance-1".into(),
                printer_external_ref: Some("external-printer-1".into()),
                printer_nickname: Some("Round-trip printer before rename".into()),
                input: library_core::models::PrinterMaintenanceInput {
                    title: "Clean nozzle".into(),
                    performed_at: "2026-10-14T12:00:00Z".into(),
                    notes: "Imported maintenance".into(),
                },
                created_at: Some("2026-10-14T12:00:00Z".into()),
            });
        printer_data
            .reminders
            .push(library_core::models::PrinterDataImportReminder {
                external_ref: "external-reminder-1".into(),
                printer_external_ref: Some("external-printer-1".into()),
                printer_nickname: Some("Round-trip printer before rename".into()),
                input: library_core::models::PrinterReminderInput {
                    title: "Inspect belts".into(),
                    notes: "Imported reminder".into(),
                    due_at: "2026-11-14T12:00:00Z".into(),
                    interval_days: Some(30),
                },
                status: library_core::models::PrinterReminderStatus::Active,
                last_completed_at: None,
                created_at: "2026-10-14T12:00:00Z".into(),
                updated_at: "2026-10-14T12:00:00Z".into(),
            });

        let imported = db.import_printer_data(&printer_data).unwrap();
        let printer_id = imported.printer_ref_map[0].local_printer_id.clone();
        let imported_log = db
            .list_printer_maintenance_logs(Some(&printer_id))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let imported_reminder = db
            .list_printer_reminders(Some(&printer_id), true)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let model = db.create_model("Round-trip model", None).unwrap();
        db.import_print_attempt_with_printer(
            &model.id,
            &library_core::models::PrintAttemptInput {
                printed_at: "2026-10-14T13:00:00Z".into(),
                quantity: 1,
                notes: "Linked history".into(),
                filament_ids: vec![],
                outcome: library_core::models::PrintOutcome::Printed,
                parts: Some(vec![CorePartUsageInput {
                    id: part.id,
                    quantity: 2,
                    snapshot: None,
                }]),
            },
            Some("2026-10-14T13:00:00.000Z"),
            Some(&printer_id),
            None,
        )
        .unwrap();

        let (export, model_count) = export_library_json_value_with_slicers(
            &db,
            &[SlicerJsonExportEntry {
                name: "Round-trip slicer".into(),
                path: "C:\\Apps\\FixtureSlicer.exe".into(),
            }],
        )
        .unwrap();
        assert_eq!(model_count, 1);
        let exported_bytes = serialize_library_json_export(&export).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&exported_bytes).unwrap();
        let mut streamed = CappedJsonWriter::new(Vec::new(), MAX_PRINTVENTORY_JSON_BYTES);
        stream_library_json_export_with_slicers(
            &db,
            export["exportDate"].as_str().unwrap(),
            &mut streamed,
            &[SlicerJsonExportEntry {
                name: "Round-trip slicer".into(),
                path: "C:\\Apps\\FixtureSlicer.exe".into(),
            }],
        )
        .unwrap();
        assert_eq!(streamed.into_inner(), exported_bytes);

        assert_eq!(parsed["printers"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["printers"][0]["gogglelab_printer_id"], printer_id);
        assert!(parsed["printers"][0].get("endpoint_approved").is_none());
        assert_eq!(
            parsed["printer_maintenance_logs"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            parsed["printer_maintenance_logs"][0]["gogglelab_maintenance_log_id"],
            imported_log.id
        );
        assert_eq!(
            parsed["printer_maintenance_logs"][0]["gogglelab_printer_id"],
            printer_id
        );
        assert_eq!(
            parsed["printer_maintenance_reminders"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            parsed["printer_maintenance_reminders"][0]["gogglelab_printer_reminder_id"],
            imported_reminder.id
        );
        assert_eq!(
            parsed["models"][0]["printEvents"][0]["gogglelab_printer_id"],
            printer_id
        );

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/printventory-printer-history.json"
        ))
        .unwrap();
        let exported_events = parsed["models"][0]["printEvents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| {
                serde_json::json!({
                    "printed_at": event["printed_at"],
                    "outcome": event["outcome"],
                    "quantity": event["quantity"],
                    "notes": event["notes"],
                    "created_at": event["created_at"],
                    "printer_nickname": event["printer_nickname"],
                    "printer_manufacturer": event["printer_manufacturer"],
                    "printer_model": event["printer_model"],
                    "printer_type": event["printer_type"],
                    "gogglelab_printer_id": "printer-external-1",
                    "filaments": event["filaments"],
                    "parts": event["parts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|part| serde_json::json!({
                            "id": "part-external-1",
                            "name": part["name"],
                            "category": part["category"],
                            "unit": part["unit"],
                            "quantity": part["quantity"],
                            "quantityPerCopy": part["quantityPerCopy"],
                        }))
                        .collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        let export_contract = serde_json::json!({
            "version": parsed["version"],
            "models": [{ "printEvents": exported_events }],
            "parts": [{
                "gogglelab_part_id": "part-external-1",
                "name": parsed["parts"][0]["name"],
                "category": parsed["parts"][0]["category"],
                "quantity": parsed["parts"][0]["quantity"],
                "unit": parsed["parts"][0]["unit"],
                "notes": parsed["parts"][0]["notes"],
                "low_stock": parsed["parts"][0]["low_stock"],
            }],
            "printers": [{
                "gogglelab_printer_id": "printer-external-1",
                "nickname": parsed["printers"][0]["nickname"],
                "manufacturer": parsed["printers"][0]["manufacturer"],
                "model": parsed["printers"][0]["model"],
                "printer_type": parsed["printers"][0]["printer_type"],
                "created_at": parsed["printers"][0]["created_at"],
                "updated_at": parsed["printers"][0]["updated_at"],
            }],
            "printer_maintenance_logs": [{
                "gogglelab_maintenance_log_id": "maintenance-external-1",
                "gogglelab_printer_id": "printer-external-1",
                "printer_nickname": parsed["printer_maintenance_logs"][0]["printer_nickname"],
                "title": parsed["printer_maintenance_logs"][0]["title"],
                "notes": parsed["printer_maintenance_logs"][0]["notes"],
                "performed_at": parsed["printer_maintenance_logs"][0]["performed_at"],
                "created_at": parsed["printer_maintenance_logs"][0]["created_at"],
            }],
            "printer_maintenance_reminders": [{
                "gogglelab_printer_reminder_id": "reminder-external-1",
                "gogglelab_printer_id": "printer-external-1",
                "printer_nickname": parsed["printer_maintenance_reminders"][0]["printer_nickname"],
                "title": parsed["printer_maintenance_reminders"][0]["title"],
                "notes": parsed["printer_maintenance_reminders"][0]["notes"],
                "due_at": parsed["printer_maintenance_reminders"][0]["due_at"],
                "interval_days": parsed["printer_maintenance_reminders"][0]["interval_days"],
                "status": parsed["printer_maintenance_reminders"][0]["status"],
                "last_completed_at": parsed["printer_maintenance_reminders"][0]["last_completed_at"],
                "created_at": parsed["printer_maintenance_reminders"][0]["created_at"],
                "updated_at": parsed["printer_maintenance_reminders"][0]["updated_at"],
            }],
            "slicers": parsed["slicers"],
        });
        assert_eq!(export_contract, fixture);
    }

    #[test]
    fn printventory_export_keeps_unlinked_printer_history_and_empty_snapshots() {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut batch = library_core::models::PrinterDataImport::default();
        batch
            .maintenance_logs
            .push(library_core::models::PrinterDataImportMaintenanceLog {
                external_ref: "unlinked-log-null".into(),
                printer_external_ref: None,
                printer_nickname: None,
                input: library_core::models::PrinterMaintenanceInput {
                    title: "Imported service without a name".into(),
                    performed_at: "2026-10-15T12:00:00Z".into(),
                    notes: "Keep this history".into(),
                },
                created_at: Some("2026-10-15T12:30:00Z".into()),
            });
        batch
            .maintenance_logs
            .push(library_core::models::PrinterDataImportMaintenanceLog {
                external_ref: "unlinked-log-dangling".into(),
                printer_external_ref: Some("skipped-printer".into()),
                printer_nickname: Some("Retired P1".into()),
                input: library_core::models::PrinterMaintenanceInput {
                    title: "Imported service with a snapshot".into(),
                    performed_at: "2026-10-15T13:00:00Z".into(),
                    notes: "Keep the printer label".into(),
                },
                created_at: Some("2026-10-15T13:30:00Z".into()),
            });
        batch.reminders.extend([
            library_core::models::PrinterDataImportReminder {
                external_ref: "unlinked-reminder-null".into(),
                printer_external_ref: None,
                printer_nickname: None,
                input: library_core::models::PrinterReminderInput {
                    title: "Imported reminder without a name".into(),
                    notes: "Keep this reminder".into(),
                    due_at: "2026-11-15T12:00:00Z".into(),
                    interval_days: None,
                },
                status: library_core::models::PrinterReminderStatus::Paused,
                last_completed_at: None,
                created_at: "2026-10-15T12:00:00Z".into(),
                updated_at: "2026-10-15T12:00:00Z".into(),
            },
            library_core::models::PrinterDataImportReminder {
                external_ref: "unlinked-reminder-dangling".into(),
                printer_external_ref: Some("skipped-printer".into()),
                printer_nickname: Some("Retired P1".into()),
                input: library_core::models::PrinterReminderInput {
                    title: "Imported reminder with a snapshot".into(),
                    notes: "Keep the printer label".into(),
                    due_at: "2026-11-16T12:00:00Z".into(),
                    interval_days: Some(90),
                },
                status: library_core::models::PrinterReminderStatus::Completed,
                last_completed_at: Some("2026-10-14T12:00:00Z".into()),
                created_at: "2026-10-15T13:00:00Z".into(),
                updated_at: "2026-10-15T13:00:00Z".into(),
            },
        ]);

        let result = db.import_printer_data(&batch).unwrap();
        assert_eq!(result.imported_maintenance_log_count, 2);
        assert_eq!(result.imported_reminder_count, 2);
        assert_eq!(db.list_printers().unwrap().len(), 0);

        let (export, _) = build_library_export_value(&db, "2026-10-16T00:00:00Z").unwrap();
        assert_eq!(export["printers"].as_array().unwrap().len(), 0);
        let logs = export["printer_maintenance_logs"].as_array().unwrap();
        assert_eq!(logs.len(), 2);
        let empty_log = logs
            .iter()
            .find(|record| record["title"] == "Imported service without a name")
            .unwrap();
        assert_eq!(empty_log["gogglelab_printer_id"], serde_json::Value::Null);
        assert_eq!(empty_log["printer_nickname"], "");
        let snapshot_log = logs
            .iter()
            .find(|record| record["title"] == "Imported service with a snapshot")
            .unwrap();
        assert_eq!(
            snapshot_log["gogglelab_printer_id"],
            serde_json::Value::Null
        );
        assert_eq!(snapshot_log["printer_nickname"], "Retired P1");
        let reminders = export["printer_maintenance_reminders"].as_array().unwrap();
        assert_eq!(reminders.len(), 2);
        let empty_reminder = reminders
            .iter()
            .find(|record| record["title"] == "Imported reminder without a name")
            .unwrap();
        assert_eq!(
            empty_reminder["gogglelab_printer_id"],
            serde_json::Value::Null
        );
        assert_eq!(empty_reminder["printer_nickname"], "");
        assert_eq!(empty_reminder["status"], "paused");
        let snapshot_reminder = reminders
            .iter()
            .find(|record| record["title"] == "Imported reminder with a snapshot")
            .unwrap();
        assert_eq!(
            snapshot_reminder["gogglelab_printer_id"],
            serde_json::Value::Null
        );
        assert_eq!(snapshot_reminder["printer_nickname"], "Retired P1");
        assert_eq!(snapshot_reminder["status"], "completed");
        assert_eq!(
            snapshot_reminder["last_completed_at"],
            "2026-10-14T12:00:00.000Z"
        );
    }

    #[test]
    fn library_json_export_keeps_maintenance_logs_above_ui_limit() {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut batch = library_core::models::PrinterDataImport::default();
        batch
            .printers
            .push(library_core::models::PrinterDataImportPrinter {
                external_ref: "export-printer".into(),
                map_to_local_printer_id: None,
                input: library_core::models::PrinterInput {
                    nickname: "Export P-1".into(),
                    make: None,
                    model: None,
                    printer_type: None,
                    firmware: None,
                    host: None,
                    web_url: None,
                    bed_mm: None,
                    notes: String::new(),
                },
                created_at: None,
                updated_at: None,
            });
        for index in 0..10_001 {
            batch
                .maintenance_logs
                .push(library_core::models::PrinterDataImportMaintenanceLog {
                    external_ref: format!("maintenance-{index}"),
                    printer_external_ref: Some("export-printer".into()),
                    printer_nickname: Some("Export P-1".into()),
                    input: library_core::models::PrinterMaintenanceInput {
                        title: "Service printer".into(),
                        performed_at: "2026-10-14T12:00:00Z".into(),
                        notes: String::new(),
                    },
                    created_at: None,
                });
            if index < 10_000 {
                batch
                    .reminders
                    .push(library_core::models::PrinterDataImportReminder {
                        external_ref: format!("reminder-{index}"),
                        printer_external_ref: Some("export-printer".into()),
                        printer_nickname: Some("Export P-1".into()),
                        input: library_core::models::PrinterReminderInput {
                            title: "Clean nozzle".into(),
                            notes: String::new(),
                            due_at: "2026-11-01T00:00:00Z".into(),
                            interval_days: None,
                        },
                        status: library_core::models::PrinterReminderStatus::Active,
                        last_completed_at: None,
                        created_at: "1700000000".into(),
                        updated_at: "1700000000".into(),
                    });
            }
        }
        db.import_printer_data(&batch).unwrap();

        let (export, _) = export_library_json_value(&db).unwrap();

        let logs = export["printer_maintenance_logs"].as_array().unwrap();
        let reminders = export["printer_maintenance_reminders"].as_array().unwrap();
        assert_eq!(logs.len(), 10_001);
        assert_eq!(reminders.len(), 10_000);
        assert!(logs.windows(2).all(|pair| {
            pair[0]["gogglelab_maintenance_log_id"].as_str().unwrap()
                < pair[1]["gogglelab_maintenance_log_id"].as_str().unwrap()
        }));
        assert!(reminders.windows(2).all(|pair| {
            pair[0]["gogglelab_printer_reminder_id"].as_str().unwrap()
                < pair[1]["gogglelab_printer_reminder_id"].as_str().unwrap()
        }));
    }

    #[test]
    fn library_json_export_drops_legacy_web_url_path_query_and_fragment() {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut printer = db
            .create_printer(
                &library_core::models::PrinterInput {
                    nickname: "Legacy URL P-1".into(),
                    make: None,
                    model: None,
                    printer_type: None,
                    firmware: None,
                    host: None,
                    web_url: None,
                    bed_mm: None,
                    notes: String::new(),
                },
                false,
            )
            .unwrap();
        printer.web_url = Some(
            "https://printer.local:8443/ui/secret-token-value?token=query-secret#fragment-secret"
                .into(),
        );

        let exported = export_printer_value(&printer).unwrap();
        let serialized = exported.to_string();

        assert_eq!(exported["web_url"], "https://printer.local:8443");
        for secret in ["secret-token-value", "query-secret", "fragment-secret"] {
            assert!(!serialized.contains(secret));
        }
    }

    #[test]
    fn oversized_library_json_export_is_rejected_before_creating_a_file() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("oversized-export.json");
        let export = serde_json::json!({
            "payload": "x".repeat(MAX_PRINTVENTORY_JSON_BYTES + 1)
        });

        let error = write_library_json_export(&destination, &export).unwrap_err();

        assert_eq!(error.code, RuntimeErrorCode::InvalidInput);
        assert!(!destination.exists());
    }

    #[test]
    fn slicer_json_export_stream_and_value_preserve_only_name_and_path() {
        let db = LibraryDb::open_in_memory().unwrap();
        let slicers = vec![SlicerJsonExportEntry {
            name: "Local Slicer".into(),
            path: "C:\\Slicers\\Local Slicer.exe".into(),
        }];
        let expected = serde_json::json!([{ "name": "Local Slicer", "path": "C:\\Slicers\\Local Slicer.exe" }]);
        let (value, count) =
            build_library_export_value_with_slicers(&db, "2026-10-17T00:00:00Z", &slicers).unwrap();
        assert_eq!(value["slicers"], expected);
        assert_eq!(count, 0);
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("slicer-export.json");
        let stage = stage_library_json_export_with_limit_and_slicers(
            &db,
            "2026-10-17T00:00:00Z",
            &destination,
            MAX_PRINTVENTORY_JSON_BYTES,
            &slicers,
        )
        .unwrap();
        stage.publish(&destination).unwrap();
        let bytes = std::fs::read(destination).unwrap();
        assert_eq!(bytes, serde_json::to_vec_pretty(&value).unwrap());
        assert_eq!(
            export_library_json_value(&db).unwrap().0["slicers"],
            serde_json::json!([])
        );
    }

    #[test]
    fn slicer_json_export_rejects_invalid_or_unbounded_profiles_before_publish() {
        let db = LibraryDb::open_in_memory().unwrap();
        let valid = SlicerJsonExportEntry {
            name: "Slicer".into(),
            path: "/Applications/Slicer.app".into(),
        };
        let mut invalid = vec![
            SlicerJsonExportEntry {
                name: " ".into(),
                ..valid.clone()
            },
            SlicerJsonExportEntry {
                name: "x".repeat(129),
                ..valid.clone()
            },
            SlicerJsonExportEntry {
                name: "Bad\nName".into(),
                ..valid.clone()
            },
            SlicerJsonExportEntry {
                path: " ".into(),
                ..valid.clone()
            },
            SlicerJsonExportEntry {
                path: "x".repeat(4097),
                ..valid.clone()
            },
            SlicerJsonExportEntry {
                path: "/tmp/bad\0path".into(),
                ..valid.clone()
            },
        ]
        .into_iter()
        .map(|entry| vec![entry])
        .collect::<Vec<_>>();
        invalid.push(vec![valid.clone(); 65]);
        invalid.push(vec![valid.clone(), valid]);
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("invalid-export.json");
        for profiles in invalid {
            assert!(export_library_json_value_with_slicers(&db, &profiles).is_err());
            assert!(stage_library_json_export_with_slicers(&db, &destination, &profiles).is_err());
            assert!(!destination.exists());
        }
    }

    #[test]
    fn staged_library_json_export_enforces_exact_limit_before_publish() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Staged export", None).unwrap();
        db.create_print_attempt(
            &model.id,
            &CorePrintAttemptInput {
                printed_at: "2026-10-14T12:00:00Z".into(),
                quantity: 1,
                notes: "A print event streamed independently".into(),
                filament_ids: Vec::new(),
                outcome: CorePrintOutcome::Printed,
                parts: Some(Vec::new()),
            },
        )
        .unwrap();
        let (expected_value, expected_models) =
            build_library_export_value(&db, "2026-10-14T00:00:00Z").unwrap();

        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("exact-limit.json");
        let expected_bytes = serde_json::to_vec_pretty(&expected_value).unwrap();
        let stage = stage_library_json_export_with_limit(
            &db,
            "2026-10-14T00:00:00Z",
            &destination,
            expected_bytes.len(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_dir(temp.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gogglelab-printventory-"))
                .count(),
            0,
            "export preparation must not create a sibling staging pathname"
        );
        let result = stage.publish(&destination).unwrap();

        assert_eq!(result.models, expected_models);
        assert_eq!(result.bytes as usize, expected_bytes.len());
        let actual_bytes = std::fs::read(&destination).unwrap();
        let actual: serde_json::Value = serde_json::from_slice(&actual_bytes).unwrap();
        assert_eq!(actual, expected_value);
        assert_eq!(actual_bytes, expected_bytes);

        let overflow_destination = temp.path().join("overflow.json");
        let error = stage_library_json_export_with_limit(
            &db,
            "2026-10-14T00:00:00Z",
            &overflow_destination,
            expected_bytes.len() - 1,
        )
        .unwrap_err();
        assert!(matches!(error, LibraryError::InvalidInput(message) if message.contains("limit")));
        assert!(!overflow_destination.exists());

        let existing_destination = temp.path().join("existing.json");
        let stage = stage_library_json_export_with_limit(
            &db,
            "2026-10-14T00:00:00Z",
            &existing_destination,
            expected_bytes.len(),
        )
        .unwrap();
        std::fs::write(&existing_destination, b"preserve this file").unwrap();
        assert!(stage.publish(&existing_destination).is_err());
        assert_eq!(
            std::fs::read(&existing_destination).unwrap(),
            b"preserve this file"
        );
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);

        let replaced_destination = temp.path().join("replaced-stage.json");
        let stage = stage_library_json_export_with_limit(
            &db,
            "2026-10-14T00:00:00Z",
            &replaced_destination,
            MAX_PRINTVENTORY_JSON_BYTES,
        )
        .unwrap();
        std::fs::write(&replaced_destination, b"created concurrently").unwrap();
        assert!(stage.publish(&replaced_destination).is_err());
        assert_eq!(
            std::fs::read(&replaced_destination).unwrap(),
            b"created concurrently"
        );
    }

    #[test]
    fn export_destination_verification_rejects_equal_bytes_on_a_different_file() {
        let temp = tempfile::tempdir().unwrap();
        let selected = temp.path().join("export.json");
        let displaced = temp.path().join("displaced-export.json");
        let destination = export_destination(&selected).unwrap();
        let bytes = b"same-size export bytes";
        let mut original = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&selected)
            .unwrap();
        original.write_all(bytes).unwrap();
        original.sync_all().unwrap();
        let fingerprint = ExportFingerprint {
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        };
        original.seek(std::io::SeekFrom::Start(3)).unwrap();
        assert!(destination_matches_open_file(
            &destination,
            &original,
            &fingerprint
        ));
        assert_eq!(original.stream_position().unwrap(), 3);
        fs::rename(&selected, &displaced).unwrap();
        fs::write(&selected, bytes).unwrap();
        assert!(!destination_matches_open_file(
            &destination,
            &original,
            &fingerprint
        ));
        assert_eq!(fs::read(&selected).unwrap(), bytes);
        assert_eq!(fs::read(&displaced).unwrap(), bytes);
    }

    #[test]
    fn partial_export_write_failure_leaves_exact_bytes_for_user_choice() {
        let db = LibraryDb::open_in_memory().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("retry-export.json");

        let stage = stage_library_json_export_with_limit(
            &db,
            "2026-10-14T00:00:00Z",
            &destination,
            MAX_PRINTVENTORY_JSON_BYTES,
        )
        .unwrap();
        assert!(stage.bytes.len() > 8);
        let expected_partial = stage.bytes[..8].to_vec();
        let error = stage
            .publish_with_failure_after(&destination, 8)
            .unwrap_err();
        assert!(
            matches!(error, LibraryError::InvalidInput(message) if message.contains("partial file may remain"))
        );
        assert_eq!(std::fs::read(&destination).unwrap(), expected_partial);
        std::fs::remove_file(&destination).unwrap();

        let retry = stage_library_json_export_with_limit(
            &db,
            "2026-10-14T00:00:00Z",
            &destination,
            MAX_PRINTVENTORY_JSON_BYTES,
        )
        .unwrap();
        let result = retry.publish(&destination).unwrap();
        assert_eq!(std::fs::metadata(&destination).unwrap().len(), result.bytes);
    }

    #[cfg(unix)]
    #[test]
    fn partial_export_sync_failure_leaves_file_for_user_choice() {
        let db = LibraryDb::open_in_memory().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("sync-failed-export.json");
        let stage = stage_library_json_export_with_limit(
            &db,
            "2026-10-14T00:00:00Z",
            &destination,
            MAX_PRINTVENTORY_JSON_BYTES,
        )
        .unwrap();
        let expected = stage.bytes.clone();

        let error = stage.publish_with_sync_failure(&destination).unwrap_err();

        assert!(
            matches!(error, LibraryError::InvalidInput(message) if message.contains("partial file may remain"))
        );
        assert_eq!(std::fs::read(&destination).unwrap(), expected);
    }

    #[cfg(unix)]
    #[test]
    fn replacement_during_export_write_is_preserved_with_displaced_bytes() {
        let db = LibraryDb::open_in_memory().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("replacement-export.json");
        let displaced_output = temp.path().join("displaced-output.json");
        let replacement = b"replacement owned by another process";
        let stage = stage_library_json_export_with_limit(
            &db,
            "2026-10-14T00:00:00Z",
            &destination,
            MAX_PRINTVENTORY_JSON_BYTES,
        )
        .unwrap();
        let expected_output = stage.bytes.clone();

        let error = stage
            .publish_with_replacement_during_write(&destination, 8, replacement, &displaced_output)
            .unwrap_err();

        assert!(
            matches!(error, LibraryError::InvalidInput(message) if message.contains("could not be verified"))
        );
        assert_eq!(std::fs::read(&destination).unwrap(), replacement);
        assert_eq!(std::fs::read(&displaced_output).unwrap(), expected_output);
    }

    #[test]
    fn json_export_snapshot_blocks_concurrent_catalog_writes_across_pages() {
        let temp = tempfile::tempdir().unwrap();
        let db = LibraryDb::open(temp.path()).unwrap();
        db.create_model("Snapshot model", None).unwrap();
        let other_connection = LibraryDb::open(temp.path()).unwrap();

        let count = db
            .with_json_export_snapshot(|snapshot| {
                let count = snapshot.count_models_for_json_export()?;
                assert_eq!(count, 1);
                assert!(other_connection
                    .create_model("Concurrent model", None)
                    .is_err());
                let page = snapshot.list_models_for_json_export_page(0, 1)?;
                assert_eq!(page.len(), count);
                Ok(count)
            })
            .unwrap();

        assert_eq!(count, 1);
        assert_eq!(other_connection.count_models_for_json_export().unwrap(), 1);
    }

    #[test]
    fn library_json_export_maps_linked_and_unlinked_print_events_without_printer_id() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Printer-attributed model", None).unwrap();
        let printer = db
            .create_printer(
                &library_core::models::PrinterInput {
                    nickname: "Workshop P2".into(),
                    make: Some("Prusa Research".into()),
                    model: Some("MK4S".into()),
                    printer_type: Some("FDM".into()),
                    firmware: Some("Prusa firmware".into()),
                    host: None,
                    web_url: None,
                    bed_mm: None,
                    notes: String::new(),
                },
                false,
            )
            .unwrap();
        db.create_print_attempt_with_printer(
            &model.id,
            &CorePrintAttemptInputWithPrinter {
                input: CorePrintAttemptInput {
                    printed_at: "2026-10-13T12:00:00Z".into(),
                    quantity: 1,
                    notes: "Linked print".into(),
                    filament_ids: Vec::new(),
                    outcome: CorePrintOutcome::Printed,
                    parts: Some(Vec::new()),
                },
                printer_id: Some(Some(printer.id.clone())),
            },
        )
        .unwrap();
        db.create_print_attempt(
            &model.id,
            &CorePrintAttemptInput {
                printed_at: "2026-10-14T12:00:00Z".into(),
                quantity: 1,
                notes: "Unlinked print".into(),
                filament_ids: Vec::new(),
                outcome: CorePrintOutcome::Printed,
                parts: Some(Vec::new()),
            },
        )
        .unwrap();

        let (export, _) = build_library_export_value(&db, "2026-10-14T00:00:00Z").unwrap();
        let events = export["models"][0]["printEvents"].as_array().unwrap();
        let linked = events
            .iter()
            .find(|event| event["notes"] == "Linked print")
            .unwrap();
        assert_eq!(linked["gogglelab_printer_id"], printer.id);
        assert_eq!(linked["printer_nickname"], "Workshop P2");
        assert_eq!(linked["printer_manufacturer"], "Prusa Research");
        assert_eq!(linked["printer_model"], "MK4S");
        assert_eq!(linked["printer_type"], "FDM");
        let linked_created_at = linked["created_at"].as_str().unwrap();
        assert!(
            time::OffsetDateTime::parse(
                linked_created_at,
                &time::format_description::well_known::Rfc3339
            )
            .is_ok(),
            "print event created_at must be RFC3339, got {linked_created_at}"
        );
        assert!(linked.get("printer_id").is_none());

        let unlinked = events
            .iter()
            .find(|event| event["notes"] == "Unlinked print")
            .unwrap();
        for field in [
            "gogglelab_printer_id",
            "printer_nickname",
            "printer_manufacturer",
            "printer_model",
            "printer_type",
        ] {
            assert_eq!(
                unlinked[field],
                serde_json::Value::Null,
                "unexpected {field}"
            );
        }
        assert!(unlinked.get("printer_id").is_none());

        let mut writer = CappedJsonWriter::new(Vec::new(), MAX_PRINTVENTORY_JSON_BYTES);
        stream_library_json_export(&db, "2026-10-15T00:00:00Z", &mut writer).unwrap();
        let streamed: serde_json::Value = serde_json::from_slice(&writer.into_inner()).unwrap();
        for event in streamed["models"][0]["printEvents"].as_array().unwrap() {
            let created_at = event["created_at"].as_str().unwrap();
            assert!(
                time::OffsetDateTime::parse(
                    created_at,
                    &time::format_description::well_known::Rfc3339
                )
                .is_ok(),
                "streamed print event created_at must be RFC3339, got {created_at}"
            );
        }
    }

    #[test]
    fn printventory_json_export_includes_parts_and_scaled_event_usage_in_both_paths() {
        let db = LibraryDb::open_in_memory().unwrap();
        let part = db
            .create_part_stock(&CorePartStockInput {
                name: "M3 heat-set insert".into(),
                category: Some("hardware".into()),
                quantity: 10,
                unit: "pcs".into(),
                notes: "brass".into(),
                low_stock: 2,
            })
            .unwrap();
        let model = db.create_model("Parts export model", None).unwrap();
        db.create_print_attempt(
            &model.id,
            &CorePrintAttemptInput {
                printed_at: "2026-10-15T12:00:00Z".into(),
                quantity: 2,
                notes: "Cancelled assembly".into(),
                filament_ids: Vec::new(),
                outcome: CorePrintOutcome::Cancelled,
                parts: Some(vec![CorePartUsageInput {
                    id: part.id.clone(),
                    quantity: 3,
                    snapshot: None,
                }]),
            },
        )
        .unwrap();

        let (value, _) = build_library_export_value(&db, "2026-10-16T00:00:00Z").unwrap();
        assert_eq!(value["parts"].as_array().unwrap().len(), 1);
        assert_eq!(value["parts"][0]["gogglelab_part_id"], part.id);
        assert_eq!(value["parts"][0]["name"], "M3 heat-set insert");
        assert_eq!(value["parts"][0]["category"], "hardware");
        assert_eq!(value["parts"][0]["quantity"], 4);
        assert_eq!(value["parts"][0]["unit"], "pcs");
        assert_eq!(value["parts"][0]["notes"], "brass");
        assert_eq!(value["parts"][0]["low_stock"], 2);
        let event = &value["models"][0]["printEvents"][0];
        assert_eq!(event["outcome"], "cancelled");
        assert_eq!(event["parts"][0]["id"], part.id);
        assert_eq!(event["parts"][0]["name"], "M3 heat-set insert");
        assert_eq!(event["parts"][0]["quantity"], 6);
        assert_eq!(event["parts"][0]["quantityPerCopy"], 3);

        let mut writer = CappedJsonWriter::new(Vec::new(), MAX_PRINTVENTORY_JSON_BYTES);
        stream_library_json_export(&db, "2026-10-16T00:00:00Z", &mut writer).unwrap();
        let streamed: serde_json::Value = serde_json::from_slice(&writer.into_inner()).unwrap();
        assert_eq!(streamed["parts"], value["parts"]);
        assert_eq!(
            streamed["models"][0]["printEvents"][0]["parts"],
            event["parts"]
        );
        assert_eq!(
            streamed["models"][0]["printEvents"][0]["outcome"],
            "cancelled"
        );
    }

    #[test]
    fn library_json_export_public_wrapper_uses_runtime_utc_timestamp() {
        let db = LibraryDb::open_in_memory().unwrap();

        let (export, model_count) = export_library_json_value(&db).unwrap();

        assert_eq!(model_count, 0);
        let export_date = export["exportDate"].as_str().unwrap();
        let parsed = time::OffsetDateTime::parse(
            export_date,
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        assert_eq!(parsed.offset(), time::UtcOffset::UTC);
    }

    #[test]
    fn library_json_export_accepts_5000_models_and_rejects_the_next_model() {
        let db = LibraryDb::open_in_memory().unwrap();
        for index in 0..library_core::MAX_LIBRARY_JSON_EXPORT_MODELS {
            db.create_model(&format!("Export cap model {index}"), None)
                .unwrap();
        }

        let (export, model_count) = export_library_json_value(&db).unwrap();
        assert_eq!(model_count, 5_000);
        assert_eq!(export["models"].as_array().unwrap().len(), 5_000);

        db.create_model("Export cap model 5000", None).unwrap();
        let error = export_library_json_value(&db).unwrap_err();
        assert!(matches!(
            error,
            LibraryError::InvalidInput(message)
                if message.contains("models")
                    && message.contains("supported maximum of 5000")
        ));
    }

    #[test]
    fn library_json_export_fails_closed_for_unresolved_print_filament_ids() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Legacy filament model", None).unwrap();
        db.create_print_attempt(
            &model.id,
            &library_core::PrintAttemptInput {
                printed_at: "2026-10-10T12:00:00Z".into(),
                quantity: 1,
                notes: String::new(),
                filament_ids: vec!["legacy-unknown".into()],
                outcome: library_core::PrintOutcome::Printed,
                parts: Some(Vec::new()),
            },
        )
        .unwrap();

        let error = build_library_export_value(&db, "2026-10-12T00:00:00Z").unwrap_err();
        assert!(matches!(
            error,
            LibraryError::InvalidInput(message) if message.contains("unresolved filament")
        ));
    }

    fn triangle_stl(x: f32) -> Vec<u8> {
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&1_u32.to_le_bytes());
        for value in [
            0.0_f32,
            0.0,
            1.0,
            x,
            0.0,
            0.0,
            x + 1.0,
            0.0,
            0.0,
            x,
            1.0,
            0.0,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes
    }

    fn metadata_3mf() -> Vec<u8> {
        let model = r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
<metadata name="Designer">Embedded Designer</metadata>
<metadata name="Description">Embedded notes</metadata>
<metadata name="License">CC0</metadata>
<metadata name="Source">https://example.test/source</metadata>
<metadata name="ParentModel">Parent Family</metadata>
<resources><object id="1" type="model"><mesh><vertices>
<vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/>
</vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources>
<build><item objectid="1"/></build></model>"#;
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        archive
            .start_file("3D/3dmodel.model", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(model.as_bytes()).unwrap();
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn exclusive_lock_survives_library_root_swap_and_releases_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let runtime = LibraryRuntime::open(config(root.clone())).unwrap();
        let second = LibraryRuntime::open(config(root.clone())).err().unwrap();
        assert_eq!(second.code, RuntimeErrorCode::Busy);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::exclusive_lock_child_probe",
                "--nocapture",
            ])
            .env("GOGGLELAB_LOCK_PROBE", &root)
            .status()
            .unwrap();
        assert!(child.success());

        let previous = temp.path().join("previous");
        fs::rename(&root, &previous).unwrap();
        fs::create_dir(&root).unwrap();
        let while_swapped = LibraryRuntime::open(config(root.clone())).err().unwrap();
        assert_eq!(while_swapped.code, RuntimeErrorCode::Busy);
        drop(runtime);
        assert!(LibraryRuntime::open(config(root)).is_ok());
    }

    #[test]
    fn desktop_open_defers_corrupt_database_failure_but_headless_fails_eagerly() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        fs::create_dir(&root).unwrap();
        let db_path = root.join("library.sqlite3");
        let corrupt = b"not a sqlite database".to_vec();
        fs::write(&db_path, &corrupt).unwrap();

        let desktop = LibraryRuntime::open(config(root.clone())).unwrap();
        assert_eq!(fs::read(&db_path).unwrap(), corrupt);
        assert_eq!(
            desktop
                .read_db(|db| db.list_models(None, None))
                .unwrap_err()
                .code,
            RuntimeErrorCode::Corrupt
        );
        assert_eq!(fs::read(&db_path).unwrap(), corrupt);
        drop(desktop);

        let mut headless = config(root);
        headless.local_catalog_root_access = false;
        assert_eq!(
            LibraryRuntime::open(headless).err().unwrap().code,
            RuntimeErrorCode::Corrupt
        );
        assert_eq!(fs::read(&db_path).unwrap(), corrupt);
    }

    #[test]
    fn desktop_open_defers_newer_schema_failure_without_mutating_database() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        drop(LibraryDb::open(&root).unwrap());
        let db_path = root.join("library.sqlite3");
        let connection = rusqlite::Connection::open(&db_path).unwrap();
        connection
            .pragma_update(
                None,
                "user_version",
                library_core::CURRENT_SCHEMA_VERSION + 1,
            )
            .unwrap();
        drop(connection);
        let before = fs::read(&db_path).unwrap();

        let desktop = LibraryRuntime::open(config(root.clone())).unwrap();
        assert_eq!(
            desktop
                .read_db(|db| db.list_models(None, None))
                .unwrap_err()
                .code,
            RuntimeErrorCode::Unsupported
        );
        assert_eq!(fs::read(&db_path).unwrap(), before);
        drop(desktop);

        let mut headless = config(root);
        headless.local_catalog_root_access = false;
        assert_eq!(
            LibraryRuntime::open(headless).err().unwrap().code,
            RuntimeErrorCode::Unsupported
        );
        assert_eq!(fs::read(&db_path).unwrap(), before);
    }

    #[test]
    fn exclusive_lock_child_probe() {
        let Some(root) = std::env::var_os("GOGGLELAB_LOCK_PROBE") else {
            return;
        };
        let error = LibraryRuntime::open(config(PathBuf::from(root)))
            .err()
            .unwrap();
        assert_eq!(error.code, RuntimeErrorCode::Busy);
    }

    #[test]
    fn maintenance_and_io_admission_share_one_epoch_and_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let io = runtime.begin_io().unwrap();
        assert_eq!(
            runtime.begin_maintenance().err().unwrap().code,
            RuntimeErrorCode::Busy
        );
        drop(io);
        let maintenance = runtime.begin_maintenance().unwrap();
        let before = runtime.library_epoch();
        assert_eq!(maintenance.invalidate(), before + 1);
        assert_eq!(
            runtime.begin_io().err().unwrap().code,
            RuntimeErrorCode::Maintenance
        );
        maintenance.remain_armed();
        drop(maintenance);
        assert_eq!(
            runtime.begin_io().err().unwrap().code,
            RuntimeErrorCode::Maintenance
        );
    }

    #[test]
    fn admitted_roots_are_opaque_and_capability_checked() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("models");
        fs::create_dir(&source).unwrap();
        let mut config = config(temp.path().join("library"));
        config.roots.push(RuntimeRootPolicy {
            id: "models".into(),
            catalog_root_id: None,
            label: "Models".into(),
            path: source.clone(),
            access: RootAccess::ReadOnly,
        });
        let runtime = LibraryRuntime::open(config).unwrap();
        let context = RequestContext {
            principal_id: "operator".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead]),
            request_id: "request".into(),
        };
        assert_eq!(runtime.root_handles(&context).unwrap()[0].id, "models");
        assert_eq!(
            runtime
                .admitted_root_path(&context, "models", RootAccess::Writable)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert_eq!(
            runtime
                .admitted_root_path(&context, "models", RootAccess::ReadOnly)
                .unwrap(),
            source.canonicalize().unwrap()
        );
    }

    #[test]
    fn backup_only_root_allows_backup_access_without_physical_source_writes() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let backup = temp.path().join("backups");
        fs::create_dir(&backup).unwrap();
        let mut config = config(library);
        config.local_catalog_root_access = false;
        config.capabilities = BTreeSet::from([
            Capability::CatalogRead,
            serde_json::from_value(serde_json::json!("backupWrite")).unwrap(),
        ]);
        config.roots.push(RuntimeRootPolicy {
            id: "backups".into(),
            catalog_root_id: None,
            label: "Backups".into(),
            path: backup.clone(),
            access: serde_json::from_value(serde_json::json!("backupOnly")).unwrap(),
        });
        let runtime = LibraryRuntime::open(config).unwrap();
        let context = RequestContext {
            principal_id: "backup-operator".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                serde_json::from_value(serde_json::json!("backupWrite")).unwrap(),
            ]),
            request_id: "backup-access".into(),
        };

        assert!(runtime
            .root_handles(&context)
            .unwrap()
            .iter()
            .all(|root| root.catalog_root_id.is_none()));
        let admitted = runtime
            .admitted_root_path(
                &context,
                "backups",
                serde_json::from_value(serde_json::json!("backupOnly")).unwrap(),
            )
            .unwrap();
        assert_eq!(admitted, backup.canonicalize().unwrap());
        assert_eq!(
            runtime
                .admitted_root_path(&context, "backups", RootAccess::ReadOnly)
                .unwrap(),
            backup.canonicalize().unwrap()
        );
        assert_eq!(
            runtime
                .admitted_root_path(&context, "backups", RootAccess::Writable)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert!(!context
            .capabilities
            .contains(&Capability::PhysicalSourceWrite));
    }

    #[test]
    fn backup_write_does_not_grant_backup_access_to_a_writable_source_root() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let mut config = config(library);
        config.local_catalog_root_access = false;
        config.capabilities = BTreeSet::from([Capability::CatalogRead, Capability::BackupWrite]);
        config.roots.push(RuntimeRootPolicy {
            id: "source".into(),
            catalog_root_id: None,
            label: "Writable source".into(),
            path: source,
            access: RootAccess::Writable,
        });
        let runtime = LibraryRuntime::open(config).unwrap();
        let context = RequestContext {
            principal_id: "backup-only".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead, Capability::BackupWrite]),
            request_id: "source-root-denied".into(),
        };

        assert_eq!(
            runtime
                .admitted_root_path(&context, "source", RootAccess::BackupOnly)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert_eq!(
            runtime
                .admitted_root_path(&context, "source", RootAccess::Writable)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
    }

    #[test]
    fn backup_only_root_cannot_map_to_an_indexed_catalog_root() {
        let temp = tempfile::tempdir().unwrap();
        let backup = temp.path().join("backups");
        fs::create_dir(&backup).unwrap();
        let mut config = config(temp.path().join("library"));
        config.local_catalog_root_access = false;
        config.capabilities = BTreeSet::from([Capability::CatalogRead, Capability::BackupWrite]);
        config.roots.push(RuntimeRootPolicy {
            id: "backups".into(),
            catalog_root_id: Some("catalog-root".into()),
            label: "Backups".into(),
            path: backup,
            access: RootAccess::BackupOnly,
        });

        assert!(LibraryRuntime::open(config).is_err());
    }

    #[test]
    fn backup_only_root_cannot_overlap_a_scannable_root() {
        for backup_is_child in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            let backup = if backup_is_child {
                source.join("backups")
            } else {
                temp.path().join("backups")
            };
            let models = if backup_is_child {
                source.clone()
            } else {
                backup.join("models")
            };
            fs::create_dir_all(&backup).unwrap();
            fs::create_dir_all(&models).unwrap();

            let mut config = config(temp.path().join("library"));
            config.roots.extend([
                RuntimeRootPolicy {
                    id: "models".into(),
                    catalog_root_id: None,
                    label: "Models".into(),
                    path: models,
                    access: RootAccess::ReadOnly,
                },
                RuntimeRootPolicy {
                    id: "backups".into(),
                    catalog_root_id: None,
                    label: "Backups".into(),
                    path: backup,
                    access: RootAccess::BackupOnly,
                },
            ]);

            assert!(LibraryRuntime::open(config).is_err());
        }
    }

    #[test]
    fn backup_only_root_cannot_overlap_the_library_root() {
        for backup_is_child in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let (library, backup) = if backup_is_child {
                let library = temp.path().join("library");
                (library.clone(), library.join("backups"))
            } else {
                let backup = temp.path().join("backups");
                (backup.join("library"), backup)
            };
            fs::create_dir_all(&backup).unwrap();
            let models = temp.path().join("models");
            fs::create_dir(&models).unwrap();

            let mut config = config(library.clone());
            config.local_catalog_root_access = false;
            config.capabilities = BTreeSet::from([Capability::CatalogRead, Capability::Scan]);
            config.roots.extend([
                RuntimeRootPolicy {
                    id: "models".into(),
                    catalog_root_id: None,
                    label: "Models".into(),
                    path: models,
                    access: RootAccess::ReadOnly,
                },
                RuntimeRootPolicy {
                    id: "backups".into(),
                    catalog_root_id: None,
                    label: "Backups".into(),
                    path: backup,
                    access: RootAccess::BackupOnly,
                },
            ]);

            assert!(LibraryRuntime::open(config).is_err());
            assert!(!library.join("library.sqlite3").exists());
            let db = LibraryDb::open(&library).unwrap();
            assert!(db.list_index_roots().unwrap().is_empty());
        }
    }

    #[test]
    fn backup_preflight_normalizes_parent_segments_without_false_overlap() {
        let temp = tempfile::tempdir().unwrap();
        let backup = temp.path().join("backups");
        let unrelated = temp.path().join("unrelated");
        fs::create_dir(&backup).unwrap();
        fs::create_dir(&unrelated).unwrap();
        let mut disjoint = config(unrelated.join("..").join("library"));
        disjoint.roots.push(RuntimeRootPolicy {
            id: "backups".into(),
            catalog_root_id: None,
            label: "Backups".into(),
            path: backup.clone(),
            access: RootAccess::BackupOnly,
        });
        assert!(LibraryRuntime::open(disjoint).is_ok());

        let nested = backup.join("nested");
        fs::create_dir(&nested).unwrap();
        let mut overlapping = config(nested.join("..").join("library"));
        overlapping.roots.push(RuntimeRootPolicy {
            id: "backups".into(),
            catalog_root_id: None,
            label: "Backups".into(),
            path: backup.clone(),
            access: RootAccess::BackupOnly,
        });
        assert!(LibraryRuntime::open(overlapping).is_err());
        assert!(!backup.join("library").exists());
    }

    #[test]
    fn scan_rejects_restored_root_when_operator_path_mapping_does_not_match() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        let source = temp.path().join("source");
        let other = temp.path().join("other");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&other).unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        let root = db
            .add_index_root(&source, 60, library_core::MAX_INDEXED_FILE_BYTES)
            .unwrap();
        drop(db);
        let error = LibraryRuntime::open(RuntimeConfig {
            library_root,
            integration_settings_path: temp.path().join("integrations.json"),
            roots: vec![RuntimeRootPolicy {
                id: "operator-root".into(),
                catalog_root_id: Some(root.id.clone()),
                label: "Other".into(),
                path: other,
                access: RootAccess::ReadOnly,
            }],
            capabilities: BTreeSet::from([Capability::CatalogRead, Capability::Scan]),
            local_catalog_root_access: false,
        })
        .err()
        .unwrap();
        assert_eq!(error.code, RuntimeErrorCode::Forbidden);
    }

    #[test]
    fn fresh_headless_read_only_root_maps_scans_and_downloads_without_path_input() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("part.stl"), triangle_stl(0.0)).unwrap();
        let runtime = LibraryRuntime::open(RuntimeConfig {
            library_root,
            integration_settings_path: temp.path().join("integrations.json"),
            roots: vec![RuntimeRootPolicy {
                id: "operator-root".into(),
                catalog_root_id: None,
                label: "Models".into(),
                path: source,
                access: RootAccess::ReadOnly,
            }],
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::MediaRead,
                Capability::Scan,
            ]),
            local_catalog_root_access: false,
        })
        .unwrap();
        let context = RequestContext {
            principal_id: "headless".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::MediaRead,
                Capability::Scan,
            ]),
            request_id: "fresh-root".into(),
        };
        let handle = runtime.root_handles(&context).unwrap().remove(0);
        let catalog_id = handle.catalog_root_id.unwrap();
        let root = runtime
            .read_db(|db| db.list_index_roots())
            .unwrap()
            .into_iter()
            .find(|root| root.id == catalog_id)
            .unwrap();
        scanner::scan_root(&runtime, &root).unwrap();
        let model = runtime
            .read_db(|db| db.list_models(None, None))
            .unwrap()
            .remove(0);
        assert!(runtime
            .issue_model_download(&context, &model.id, model.content_hash.as_deref())
            .is_ok());
    }

    #[test]
    fn root_relative_subdirectory_scan_marks_only_missing_sources_in_its_scope() {
        let temp = tempfile::tempdir().unwrap();
        let root_path = temp.path().join("models");
        let parts = root_path.join("parts/nested");
        fs::create_dir_all(&parts).unwrap();
        fs::create_dir_all(root_path.join("other")).unwrap();
        let existing_part = parts.join("existing.stl");
        let outside = root_path.join("other/outside.stl");
        fs::write(&existing_part, triangle_stl(0.0)).unwrap();
        fs::write(&outside, triangle_stl(2.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let root = runtime
            .mutate_db(|db| db.add_index_root(&root_path, 60, library_core::MAX_INDEXED_FILE_BYTES))
            .unwrap();
        scanner::scan_root(&runtime, &root).unwrap();
        let before = runtime.read_db(|db| db.list_models(None, None)).unwrap();
        let outside_model = before
            .iter()
            .find(|model| {
                model
                    .source_path
                    .as_deref()
                    .is_some_and(|path| path.ends_with("outside.stl"))
            })
            .unwrap()
            .clone();
        let inside_model = before
            .iter()
            .find(|model| {
                model
                    .source_path
                    .as_deref()
                    .is_some_and(|path| path.ends_with("existing.stl"))
            })
            .unwrap()
            .clone();
        fs::remove_file(&outside).unwrap();
        fs::remove_file(&existing_part).unwrap();
        let new_part = parts.join("new.stl");
        fs::write(&new_part, triangle_stl(4.0)).unwrap();

        scanner::scan_subdirectory(&runtime, &root, Path::new("parts")).unwrap();
        let after = runtime.read_db(|db| db.list_models(None, None)).unwrap();
        let outside_after = after
            .iter()
            .find(|model| model.id == outside_model.id)
            .unwrap();
        assert!(!outside_after.source_missing);
        let inside_after = after
            .iter()
            .find(|model| model.id == inside_model.id)
            .unwrap();
        assert!(inside_after.source_missing);
        assert!(after.iter().any(|model| {
            model
                .source_path
                .as_deref()
                .is_some_and(|path| path.ends_with("new.stl"))
        }));
        assert!(scanner::scan_subdirectory(&runtime, &root, Path::new("../other")).is_err());
        assert!(scanner::scan_subdirectory(&runtime, &root, Path::new("/tmp")).is_err());

        #[cfg(unix)]
        {
            let outside_directory = temp.path().join("outside-directory");
            fs::create_dir(&outside_directory).unwrap();
            std::os::unix::fs::symlink(&outside_directory, root_path.join("linked")).unwrap();
            assert!(scanner::scan_subdirectory(&runtime, &root, Path::new("linked")).is_err());
        }
    }

    #[test]
    fn readonly_headless_startup_does_not_create_or_scan_catalog_roots() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let file = source.join("part.stl");
        let bytes = triangle_stl(0.0);
        fs::write(&file, &bytes).unwrap();
        let runtime = LibraryRuntime::open(RuntimeConfig {
            library_root: temp.path().join("library"),
            integration_settings_path: temp.path().join("integrations.json"),
            roots: vec![RuntimeRootPolicy {
                id: "readonly".into(),
                catalog_root_id: None,
                label: "Read only".into(),
                path: source,
                access: RootAccess::ReadOnly,
            }],
            capabilities: BTreeSet::from([Capability::CatalogRead, Capability::MediaRead]),
            local_catalog_root_access: false,
        })
        .unwrap();
        scanner::start_background_scanner(&runtime);
        assert!(!runtime.scan_control().status().running);
        assert!(runtime
            .read_db(|db| db.list_index_roots())
            .unwrap()
            .is_empty());
        assert_eq!(fs::read(file).unwrap(), bytes);
    }

    #[test]
    fn opaque_root_and_catalog_settings_round_trip_with_capability_checks() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let runtime = LibraryRuntime::open(RuntimeConfig {
            library_root: temp.path().join("library"),
            integration_settings_path: temp.path().join("integrations.json"),
            roots: vec![RuntimeRootPolicy {
                id: "models".into(),
                catalog_root_id: None,
                label: "Models".into(),
                path: source,
                access: RootAccess::ReadOnly,
            }],
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::CatalogWrite,
                Capability::Scan,
                Capability::MediaWrite,
            ]),
            local_catalog_root_access: false,
        })
        .unwrap();
        let context = RequestContext {
            principal_id: "operator".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::CatalogWrite,
                Capability::Scan,
            ]),
            request_id: "settings".into(),
        };
        let root = runtime
            .list_index_root_settings(&context)
            .unwrap()
            .remove(0);
        let updated = runtime
            .update_index_root_settings(
                &context,
                &root.handle_id,
                IndexRootSettingsPatch {
                    enabled: false,
                    scan_interval_minutes: 30,
                    max_file_bytes: 20 * 1024 * 1024,
                    scan_zip: true,
                },
            )
            .unwrap();
        assert!(!updated.enabled);
        assert!(updated.scan_zip);
        assert_eq!(updated.catalog_root_id, root.catalog_root_id);
        let mut settings = runtime.get_catalog_settings(&context).unwrap();
        settings.thumbnail_size = 128;
        settings.thumbnail_background = "#FFFFFF".into();
        assert_eq!(
            runtime
                .update_catalog_settings(&context, settings.clone())
                .unwrap(),
            settings
        );
    }

    #[test]
    fn headless_completed_scan_persists_default_thumbnail_without_source_writes_or_staging_holds() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let file = source.join("part.stl");
        let bytes = triangle_stl(0.0);
        fs::write(&file, &bytes).unwrap();
        let runtime = LibraryRuntime::open(RuntimeConfig {
            library_root: temp.path().join("library"),
            integration_settings_path: temp.path().join("integrations.json"),
            roots: vec![RuntimeRootPolicy {
                id: "models".into(),
                catalog_root_id: None,
                label: "Models".into(),
                path: source,
                access: RootAccess::ReadOnly,
            }],
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::MediaRead,
                Capability::MediaWrite,
                Capability::Scan,
            ]),
            local_catalog_root_access: false,
        })
        .unwrap();
        let root = runtime
            .read_db(|db| db.list_index_roots())
            .unwrap()
            .remove(0);
        let staging = runtime.root.join("staging");
        let unknown = staging.join("thumbnail-source-foreign.part");
        fs::write(&unknown, b"unrelated staging bytes").unwrap();
        let staging_names = || {
            fs::read_dir(&staging)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<BTreeSet<_>>()
        };
        let before = staging_names();
        scanner::scan_root(&runtime, &root).unwrap();
        assert_eq!(
            staging_names(),
            before,
            "default rendering must not materialize source copies or generated recovery holds"
        );
        assert_eq!(fs::read(&unknown).unwrap(), b"unrelated staging bytes");
        let model = runtime
            .read_db(|db| db.list_models(None, None))
            .unwrap()
            .remove(0);
        assert!(runtime
            .read_db(|db| db.thumbnail_cache_entry_for_model(&model.id))
            .unwrap()
            .is_some());
        assert_eq!(fs::read(file).unwrap(), bytes);
        let status = runtime.scan_control().status();
        assert_eq!(status.thumbnails_total, 1);
        assert_eq!(status.thumbnails_completed, 1);
    }

    #[test]
    fn remote_scan_status_redacts_host_paths_without_explicit_path_capability() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        runtime
            .scan_control()
            .error("/secret/models/part.stl: permission denied".into());
        let remote = RequestContext {
            principal_id: "remote".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead]),
            request_id: "status".into(),
        };
        let status = runtime.scan_status(&remote).unwrap();
        assert!(!status.errors.join(" ").contains("/secret"));
        let local = RequestContext {
            principal_id: "desktop".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead, Capability::HostPaths]),
            request_id: "local-status".into(),
        };
        assert!(runtime
            .scan_status(&local)
            .unwrap()
            .errors
            .join(" ")
            .contains("/secret"));
    }

    #[test]
    fn bundle_preview_is_principal_bound_and_revalidates_membership() {
        let temp = tempfile::tempdir().unwrap();
        let first_path = temp.path().join("first.stl");
        let second_path = temp.path().join("second.stl");
        fs::write(&first_path, triangle_stl(0.0)).unwrap();
        fs::write(&second_path, triangle_stl(2.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let first = runtime
            .mutate_db(|db| {
                db.import_file(&first_path, None, None)
                    .map(|value| value.model)
            })
            .unwrap();
        let second = runtime
            .mutate_db(|db| {
                db.import_file(&second_path, None, None)
                    .map(|value| value.model)
            })
            .unwrap();
        let bundle_id = runtime
            .mutate_db(|db| db.create_bundle(&[first.id, second.id]))
            .unwrap();
        let context = RequestContext {
            principal_id: "bundle-reader".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "bundle-preview".into(),
        };
        let summary = runtime
            .prepare_bundle_preview(&context, &bundle_id)
            .unwrap();
        assert_eq!(summary.member_count, 2);
        assert_eq!(summary.triangle_count, 2);
        let descriptor = runtime
            .describe_bundle_geometry(&context, &summary.token)
            .unwrap();
        let mut geometry = Vec::new();
        runtime
            .stream_bundle_geometry(&context, &summary.token, &mut geometry)
            .unwrap();
        assert_eq!(geometry.len() as u64, descriptor.bytes);
        assert_eq!(&geometry[..4], b"STL2");
        let other = RequestContext {
            principal_id: "other".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "other".into(),
        };
        assert_eq!(
            runtime
                .describe_bundle_geometry(&other, &summary.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        runtime
            .mutate_db(|db| db.create_model("Changed", None))
            .unwrap();
        assert_eq!(
            runtime
                .stream_bundle_geometry(&context, &summary.token, &mut Vec::new())
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
    }

    #[test]
    fn bundle_download_includes_all_thirty_three_members_and_epoch_cleans_owned_zip() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        fs::write(&source, triangle_stl(0.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let mut model_ids = Vec::new();
        for index in 0..33 {
            let model = runtime
                .mutate_db(|db| {
                    db.import_file(&source, None, Some(&format!("Part {index:02}")))
                        .map(|value| value.model)
                })
                .unwrap();
            model_ids.push(model.id);
        }
        let bundle_id = runtime
            .mutate_db(|db| db.create_bundle(&model_ids))
            .unwrap();
        let context = RequestContext {
            principal_id: "bundle-download".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "bundle-download".into(),
        };
        let preview = runtime
            .prepare_bundle_preview(&context, &bundle_id)
            .unwrap();
        assert_eq!(preview.member_count, 33);
        assert_eq!(preview.previewed_count, 32);
        assert_eq!(preview.omitted_count, 1);
        let lease = runtime
            .issue_bundle_download(&context, &preview.token)
            .unwrap();
        assert_eq!(lease.kind, ContentKind::BundleArchive);
        let managed_path = runtime
            .read_db(|db| db.resolve_model_source_ref(&model_ids[0]))
            .unwrap()
            .outer_path;
        fs::write(&managed_path, triangle_stl(9.0)).unwrap();
        let reused = runtime
            .issue_bundle_download(&context, &preview.token)
            .unwrap();
        assert_eq!(reused.id, lease.id);
        let owned_path = runtime
            .leases
            .lock()
            .unwrap()
            .get(&lease.id)
            .unwrap()
            .backing
            .path()
            .unwrap()
            .to_path_buf();
        let mut archive_bytes = Vec::new();
        runtime
            .stream_content(&context, &lease.id, &mut archive_bytes)
            .unwrap();
        assert_eq!(
            zip::ZipArchive::new(std::io::Cursor::new(archive_bytes))
                .unwrap()
                .len(),
            33
        );
        assert_eq!(
            fs::read_dir(runtime.root.join("staging/bundle-downloads"))
                .unwrap()
                .count(),
            1
        );
        let other = RequestContext {
            principal_id: "other".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "other".into(),
        };
        assert_eq!(
            runtime
                .describe_content(&other, &lease.id)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        let maintenance = runtime.begin_maintenance().unwrap();
        maintenance.invalidate();
        drop(maintenance);
        assert!(!owned_path.exists());
        assert_eq!(runtime.temp_resources.lock().unwrap().owned_bytes, 0);
    }

    #[test]
    fn geometry_expiry_retires_packet_without_named_files_or_source_holds() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        let original = triangle_stl(0.0);
        fs::write(&source, &original).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        let context = RequestContext {
            principal_id: "geometry".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "geometry".into(),
        };
        let lease = runtime
            .issue_model_geometry(&context, &model.id, None, None)
            .unwrap();
        let mut packet = Vec::new();
        runtime
            .stream_content(&context, &lease.content.id, &mut packet)
            .unwrap();
        assert_eq!(&packet[..4], b"STL2");
        runtime
            .leases
            .lock()
            .unwrap()
            .get_mut(&lease.content.id)
            .unwrap()
            .expires = SystemTime::UNIX_EPOCH;
        assert_eq!(
            runtime
                .describe_content(&context, &lease.content.id)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
        let namespace = runtime.root.join("staging/geometry");
        assert_eq!(
            fs::read_dir(&namespace).unwrap().count(),
            0,
            "Expired geometry must leave no named packet, hold, or journal"
        );
        assert_eq!(runtime.temp_resources.lock().unwrap().owned_bytes, 0);
        drop(runtime);
        assert_eq!(fs::read_dir(namespace).unwrap().count(), 0);
        assert_eq!(fs::read(source).unwrap(), original);
    }

    #[test]
    fn geometry_reader_keeps_its_byte_charge_after_token_expiry() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        fs::write(&source, triangle_stl(0.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        let context = RequestContext {
            principal_id: "geometry".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "geometry".into(),
        };
        let lease = runtime
            .issue_model_geometry(&context, &model.id, None, None)
            .unwrap();
        let reader = runtime
            .admitted_content_entry(&context, &lease.content.id)
            .unwrap();
        runtime
            .leases
            .lock()
            .unwrap()
            .get_mut(&lease.content.id)
            .unwrap()
            .expires = SystemTime::UNIX_EPOCH;
        assert!(runtime
            .describe_content(&context, &lease.content.id)
            .is_err());
        assert_eq!(
            runtime.temp_resources.lock().unwrap().owned_bytes,
            lease.content.bytes,
            "Reader must retain backing quota after token removal"
        );
        drop(reader);
        assert_eq!(runtime.temp_resources.lock().unwrap().owned_bytes, 0);
    }

    #[test]
    fn owned_temp_reservation_full_cap_refuses_before_side_effects() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        runtime.set_temp_limits_for_test(8, 8);
        let owner = runtime
            .reserve_temp(ReservationKind::Owned, 8)
            .unwrap()
            .into_owned_reservation()
            .unwrap();
        let output = runtime.root.join("must-not-be-created");
        let result = (|| -> Result<(), RuntimeError> {
            let _reservation = runtime
                .reserve_temp(ReservationKind::Owned, 1)?
                .into_owned_reservation()?;
            fs::write(&output, b"unadmitted output").map_err(|_| io_error())?;
            Ok(())
        })();
        assert_eq!(result.unwrap_err().code, RuntimeErrorCode::Busy);
        assert!(!output.exists());
        assert_eq!(
            runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
            8
        );
        assert_eq!(runtime.temp_resources.lock().unwrap().owned_bytes, 0);
        drop(owner);
        assert_eq!(
            runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
            0
        );
    }

    #[test]
    fn owned_temp_reservation_wrong_kind_rolls_back_original_category() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        runtime.set_temp_limits_for_test(8, 8);
        for kind in [
            ReservationKind::Upload,
            ReservationKind::Bundle,
            ReservationKind::Geometry,
        ] {
            assert!(runtime
                .reserve_temp(kind, 4)
                .unwrap()
                .into_owned_reservation()
                .is_err());
            let state = runtime.temp_resources.lock().unwrap();
            assert_eq!(state.reserved_owned_bytes, 0);
            assert_eq!(state.reserved_upload_bytes, 0);
            assert_eq!(state.active_bundle_builds, 0);
            assert_eq!(state.owned_bytes, 0);
            assert_eq!(state.staged_upload_bytes, 0);
        }
    }

    #[test]
    fn owned_temp_reservation_outlives_runtime_until_last_arc_owner() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        runtime.set_temp_limits_for_test(8, 8);
        let owner = Arc::new(
            runtime
                .reserve_temp(ReservationKind::Owned, 8)
                .unwrap()
                .into_owned_reservation()
                .unwrap(),
        );
        let last = Arc::clone(&owner);
        let state = Arc::clone(&runtime.temp_resources);
        let weak = Arc::downgrade(&runtime);
        drop(owner);
        drop(runtime);
        assert!(weak.upgrade().is_none());
        assert_eq!(state.lock().unwrap().reserved_owned_bytes, 8);
        assert_eq!(state.lock().unwrap().owned_bytes, 0);
        drop(last);
        assert_eq!(state.lock().unwrap().reserved_owned_bytes, 0);
    }

    #[test]
    fn owned_temp_reservation_contends_with_shared_owned_and_bundle_admission() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        runtime.set_temp_limits_for_test(12, 8);
        let owner = runtime
            .reserve_temp(ReservationKind::Owned, 8)
            .unwrap()
            .into_owned_reservation()
            .unwrap();
        let bundle = runtime.reserve_temp(ReservationKind::Bundle, 4).unwrap();
        assert!(runtime.reserve_temp(ReservationKind::Owned, 1).is_err());
        assert!(runtime.reserve_temp(ReservationKind::Geometry, 1).is_err());
        drop(bundle);
        assert_eq!(
            runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
            8
        );
        assert_eq!(
            runtime.temp_resources.lock().unwrap().active_bundle_builds,
            0
        );
        drop(owner);
        assert!(runtime.reserve_temp(ReservationKind::Owned, 12).is_ok());
        assert_eq!(
            runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
            0
        );
    }

    #[test]
    fn owned_temp_reservation_shared_threads_cannot_exceed_one_cap() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        runtime.set_temp_limits_for_test(8, 8);
        let attempted = std::sync::Barrier::new(3);
        let release = std::sync::Barrier::new(3);
        let winners = std::sync::atomic::AtomicUsize::new(0);
        let observed = std::thread::scope(|scope| {
            for _ in 0..2 {
                let runtime = &runtime;
                let attempted = &attempted;
                let release = &release;
                let winners = &winners;
                scope.spawn(move || {
                    let owner = runtime
                        .reserve_temp(ReservationKind::Owned, 8)
                        .and_then(TempReservation::into_owned_reservation);
                    if owner.is_ok() {
                        winners.fetch_add(1, Ordering::SeqCst);
                    }
                    attempted.wait();
                    release.wait();
                    drop(owner);
                });
            }
            attempted.wait();
            let observed = (
                winners.load(Ordering::SeqCst),
                runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
            );
            release.wait();
            observed
        });
        assert_eq!(observed, (1, 8));
        assert_eq!(
            runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
            0
        );
    }

    #[test]
    fn owned_temp_reservation_transfer_rechecks_limit_and_errors_do_not_leak() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        runtime.set_temp_limits_for_test(8, 8);
        let reservation = runtime.reserve_temp(ReservationKind::Owned, 8).unwrap();
        runtime.temp_limits.lock().unwrap().owned_bytes = 7;
        assert_eq!(
            reservation.into_owned_reservation().unwrap_err().code,
            RuntimeErrorCode::Busy
        );
        assert_eq!(
            runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
            0
        );
        let result = (|| -> Result<(), RuntimeError> {
            let _owner = runtime
                .reserve_temp(ReservationKind::Owned, 7)?
                .into_owned_reservation()?;
            Err(invalid("Synthetic failure after admission"))
        })();
        assert!(result.is_err());
        assert_eq!(
            runtime.temp_resources.lock().unwrap().reserved_owned_bytes,
            0
        );
        assert_eq!(runtime.temp_resources.lock().unwrap().owned_bytes, 0);
        assert!(runtime.reserve_temp(ReservationKind::Owned, 8).is_err());
    }

    #[test]
    fn geometry_reader_outlives_runtime_without_a_runtime_cycle_or_early_quota_release() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        fs::write(&source, triangle_stl(0.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        let context = RequestContext {
            principal_id: "geometry".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "geometry".into(),
        };
        let lease = runtime
            .issue_model_geometry(&context, &model.id, None, None)
            .unwrap();
        let reader = runtime
            .admitted_content_entry(&context, &lease.content.id)
            .unwrap();
        let state = Arc::clone(&runtime.temp_resources);
        let weak = Arc::downgrade(&runtime);
        drop(runtime);
        assert!(weak.upgrade().is_none());
        assert_eq!(state.lock().unwrap().owned_bytes, lease.content.bytes);
        let mut packet = Vec::new();
        reader
            .backing
            .stream_to(&reader.descriptor, &mut packet)
            .unwrap();
        assert_eq!(&packet[..4], b"STL2");
        drop(reader);
        assert_eq!(state.lock().unwrap().owned_bytes, 0);
        assert_eq!(
            fs::read_dir(temp.path().join("library/staging/geometry"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn geometry_owner_epoch_and_active_reader_rules_remain_enforced() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        fs::write(&source, triangle_stl(0.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        let context = RequestContext {
            principal_id: "geometry".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "geometry".into(),
        };
        let lease = runtime
            .issue_model_geometry(&context, &model.id, None, None)
            .unwrap();
        let mut other = context.clone();
        other.principal_id = "different-owner".into();
        let mut denied = Vec::new();
        assert_eq!(
            runtime
                .stream_content(&other, &lease.content.id, &mut denied)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert!(denied.is_empty());
        assert!(runtime.active_content_lease_paths().is_empty());
        let reader = runtime
            .admitted_content_entry(&context, &lease.content.id)
            .unwrap();
        let maintenance = runtime.begin_maintenance().unwrap();
        maintenance.invalidate();
        drop(maintenance);
        assert_eq!(
            runtime
                .describe_content(&context, &lease.content.id)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
        assert_eq!(
            runtime.temp_resources.lock().unwrap().owned_bytes,
            lease.content.bytes
        );
        drop(reader);
        assert_eq!(runtime.temp_resources.lock().unwrap().owned_bytes, 0);
    }

    #[test]
    fn geometry_admission_failure_releases_packet_charge_without_a_hold() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        fs::write(&source, triangle_stl(0.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        let context = RequestContext {
            principal_id: "geometry".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "geometry".into(),
        };
        let original = runtime
            .issue_model_download(&context, &model.id, None)
            .unwrap();
        let entry = runtime
            .leases
            .lock()
            .unwrap()
            .get(&original.id)
            .unwrap()
            .clone();
        for index in 0..MAX_LEASES {
            runtime
                .leases
                .lock()
                .unwrap()
                .insert(format!("fixture-{index}"), entry.clone());
        }
        assert_eq!(
            runtime
                .issue_model_geometry(&context, &model.id, None, None)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        let state = runtime.temp_resources.lock().unwrap();
        assert_eq!(state.owned_bytes, 0);
        assert_eq!(state.reserved_owned_bytes, 0);
        drop(state);
        assert_eq!(
            fs::read_dir(runtime.root.join("staging/geometry"))
                .unwrap()
                .count(),
            0
        );
        assert!(source.is_file());
    }

    #[test]
    fn retained_geometry_files_are_preserved_counted_and_bound_new_geometry_only() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        fs::write(&source, triangle_stl(0.0)).unwrap();
        let library = temp.path().join("library");
        let runtime = LibraryRuntime::open(config(library.clone())).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        drop(runtime);
        let namespace = library.join("staging/geometry");
        fs::create_dir(&namespace).unwrap();
        fs::write(namespace.join("geometry-old.stl2"), b"legacy").unwrap();
        fs::write(namespace.join(".geometry-capture-unknown"), b"unknown").unwrap();
        let runtime = LibraryRuntime::open(config(library.clone())).unwrap();
        assert!(runtime
            .read_db(|db| db.get_model(&model.id))
            .unwrap()
            .is_some());
        assert_eq!(
            runtime
                .temp_resources
                .lock()
                .unwrap()
                .retained_geometry_bytes,
            13
        );
        runtime.temp_limits.lock().unwrap().owned_bytes = 13 + 51;
        let context = RequestContext {
            principal_id: "geometry".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "geometry".into(),
        };
        assert_eq!(
            runtime
                .issue_model_geometry(&context, &model.id, None, None)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        assert_eq!(
            fs::read(namespace.join("geometry-old.stl2")).unwrap(),
            b"legacy"
        );
        assert_eq!(
            fs::read(namespace.join(".geometry-capture-unknown")).unwrap(),
            b"unknown"
        );
        assert_eq!(fs::read_dir(&namespace).unwrap().count(), 2);
        runtime.temp_limits.lock().unwrap().owned_bytes = DEFAULT_OWNED_TEMP_BYTES;
        for index in 0..255 {
            fs::write(namespace.join(format!("unknown-{index}")), b"").unwrap();
        }
        assert_eq!(
            runtime
                .issue_model_geometry(&context, &model.id, None, None)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        assert_eq!(fs::read_dir(&namespace).unwrap().count(), 257);
        assert!(runtime
            .read_db(|db| db.get_model(&model.id))
            .unwrap()
            .is_some());
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_geometry_storage_does_not_block_explicit_thumbnail_jobs() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        let original = triangle_stl(0.0);
        fs::write(&source, &original).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel"), b"preserve").unwrap();
        std::os::unix::fs::symlink(&outside, runtime.root.join("staging/geometry")).unwrap();
        let context = RequestContext {
            principal_id: "thumbnail".into(),
            capabilities: BTreeSet::from([
                Capability::MediaRead,
                Capability::MediaWrite,
                Capability::CatalogWrite,
            ]),
            request_id: "thumbnail".into(),
        };
        assert_eq!(
            runtime
                .issue_model_geometry(&context, &model.id, None, None)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        runtime
            .start_thumbnail_job(&context, vec![model.id.clone()], true)
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while runtime.scan_control().status().running && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let status = runtime.scan_control().status();
        assert!(!status.running);
        assert!(status.errors.is_empty(), "{:?}", status.errors);
        assert_eq!(status.thumbnails_completed, 1);
        assert!(runtime
            .read_db(|db| db.thumbnail_cache_entry_for_model(&model.id))
            .unwrap()
            .is_some());
        assert_eq!(fs::read(source).unwrap(), original);
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"preserve");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn concurrent_geometry_inventory_waits_for_the_empty_named_birth_window() {
        use std::sync::mpsc;
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        fs::write(&source, triangle_stl(0.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        let namespace = runtime.root.join("staging/geometry");
        fs::create_dir(&namespace).unwrap();
        let (born_tx, born_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let birth = std::thread::spawn(move || {
            let directory = secure_fs::Directory::open(&namespace).unwrap();
            let writer = directory.create_anonymous_geometry_writer_with_hook(|stage, _, _| {
                if stage == "after_empty_create" {
                    born_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }
                Ok(())
            });
            writer.map(drop)
        });
        born_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let reader_runtime = Arc::clone(&runtime);
        let reader = std::thread::spawn(move || {
            let context = RequestContext {
                principal_id: "parallel-geometry".into(),
                capabilities: BTreeSet::from([Capability::MediaRead]),
                request_id: "parallel".into(),
            };
            started_tx.send(()).unwrap();
            result_tx
                .send(reader_runtime.issue_model_geometry(&context, &model.id, None, None))
                .unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let premature = result_rx.recv_timeout(Duration::from_millis(200));
        release_tx.send(()).unwrap();
        birth.join().unwrap().unwrap();
        reader.join().unwrap();
        assert!(matches!(premature,Err(mpsc::RecvTimeoutError::Timeout)),"Inventory must wait rather than interpret the creator's mode-000 file as foreign storage");
        assert!(result_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .is_ok());
    }

    #[test]
    fn one_packet_budget_is_retained_until_the_last_reader_closes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        fs::write(&source, triangle_stl(0.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        runtime.set_temp_limits_for_test(52, 1024);
        let context = RequestContext {
            principal_id: "geometry".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "geometry".into(),
        };
        let first = runtime
            .issue_model_geometry(&context, &model.id, None, None)
            .unwrap();
        assert_eq!(first.content.bytes, 52);
        let reader = runtime
            .admitted_content_entry(&context, &first.content.id)
            .unwrap();
        runtime
            .leases
            .lock()
            .unwrap()
            .get_mut(&first.content.id)
            .unwrap()
            .expires = SystemTime::UNIX_EPOCH;
        assert_eq!(
            runtime
                .issue_model_geometry(&context, &model.id, None, None)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        assert!(!runtime
            .leases
            .lock()
            .unwrap()
            .contains_key(&first.content.id));
        drop(reader);
        assert!(runtime
            .issue_model_geometry(&context, &model.id, None, None)
            .is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn expired_geometry_without_readers_releases_budget_on_the_next_geometry_request() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        fs::write(&source, triangle_stl(0.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        runtime.set_temp_limits_for_test(52, 1024);
        let context = RequestContext {
            principal_id: "geometry".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "geometry".into(),
        };
        let first = runtime
            .issue_model_geometry(&context, &model.id, None, None)
            .unwrap();
        runtime
            .leases
            .lock()
            .unwrap()
            .get_mut(&first.content.id)
            .unwrap()
            .expires = SystemTime::UNIX_EPOCH;
        let next = runtime
            .issue_model_geometry(&context, &model.id, None, None)
            .unwrap();
        assert_ne!(next.content.id, first.content.id);
        assert_eq!(next.content.sha256, first.content.sha256);
        assert!(!runtime
            .leases
            .lock()
            .unwrap()
            .contains_key(&first.content.id));
        assert_eq!(runtime.temp_resources.lock().unwrap().owned_bytes, 52);
        assert_eq!(
            fs::read_dir(runtime.root.join("staging/geometry"))
                .unwrap()
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_geometry_namespace_blocks_geometry_without_blocking_the_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        fs::write(&source, triangle_stl(0.0)).unwrap();
        let library = temp.path().join("library");
        let runtime = LibraryRuntime::open(config(library.clone())).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        drop(runtime);
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel"), b"preserve").unwrap();
        std::os::unix::fs::symlink(&outside, library.join("staging/geometry")).unwrap();
        let runtime = LibraryRuntime::open(config(library)).unwrap();
        assert!(runtime
            .read_db(|db| db.get_model(&model.id))
            .unwrap()
            .is_some());
        let context = RequestContext {
            principal_id: "geometry".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "geometry".into(),
        };
        assert_eq!(
            runtime
                .issue_model_geometry(&context, &model.id, None, None)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"preserve");
    }

    #[test]
    fn model_geometry_uses_stl2_lease_and_rejects_stale_hash() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        fs::write(&source, triangle_stl(0.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        let context = RequestContext {
            principal_id: "geometry".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "geometry".into(),
        };
        assert_eq!(
            runtime
                .issue_model_geometry(&context, &model.id, Some("00"), None)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
        let geometry = runtime
            .issue_model_geometry(&context, &model.id, model.content_hash.as_deref(), None)
            .unwrap();
        assert_eq!(geometry.content.kind, ContentKind::Geometry);
        assert_eq!(geometry.packet_format, "stl2");
        assert_eq!(geometry.packet_version, 2);
        assert_eq!(geometry.triangle_count, 1);
        assert_eq!(geometry.plates.len(), 1);
        let mut packet = Vec::new();
        runtime
            .stream_content(&context, &geometry.content.id, &mut packet)
            .unwrap();
        assert_eq!(&packet[..4], b"STL2");
        assert_eq!(u32::from_le_bytes(packet[8..12].try_into().unwrap()), 1);
        drop(runtime);
        assert_eq!(
            fs::read_dir(temp.path().join("library/staging/geometry"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn first_database_use_prunes_bundle_orphans_and_preserves_unverified_geometry() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let runtime = LibraryRuntime::open(config(root.clone())).unwrap();
        runtime.read_db(|db| db.list_models(None, None)).unwrap();
        let bundle_dir = root.join("staging/bundle-downloads");
        let geometry_dir = root.join("staging/geometry");
        fs::create_dir(&bundle_dir).unwrap();
        fs::create_dir(&geometry_dir).unwrap();
        fs::write(bundle_dir.join("bundle-orphan.zip"), b"orphan").unwrap();
        fs::write(geometry_dir.join("geometry-orphan.stl2"), b"orphan").unwrap();
        fs::write(bundle_dir.join("operator-note.txt"), b"preserve").unwrap();
        drop(runtime);

        let reopened = LibraryRuntime::open(config(root)).unwrap();
        reopened.read_db(|db| db.list_models(None, None)).unwrap();
        assert!(!bundle_dir.join("bundle-orphan.zip").exists());
        assert_eq!(
            fs::read(geometry_dir.join("geometry-orphan.stl2")).unwrap(),
            b"orphan"
        );
        assert!(bundle_dir.join("operator-note.txt").exists());
    }

    #[test]
    fn bundle_download_source_tamper_retains_partial_owned_archive_in_a_recovery_hold() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        fs::write(&source, triangle_stl(0.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let first = runtime
            .mutate_db(|db| {
                db.import_file(&source, None, Some("First"))
                    .map(|value| value.model)
            })
            .unwrap();
        let second = runtime
            .mutate_db(|db| {
                db.import_file(&source, None, Some("Second"))
                    .map(|value| value.model)
            })
            .unwrap();
        let bundle_id = runtime
            .mutate_db(|db| db.create_bundle(&[first.id.clone(), second.id]))
            .unwrap();
        let context = RequestContext {
            principal_id: "tamper".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "tamper".into(),
        };
        let preview = runtime
            .prepare_bundle_preview(&context, &bundle_id)
            .unwrap();
        let managed_path = runtime
            .read_db(|db| db.resolve_model_source_ref(&first.id))
            .unwrap()
            .outer_path;
        fs::write(&managed_path, triangle_stl(5.0)).unwrap();
        assert_eq!(
            runtime
                .issue_bundle_download(&context, &preview.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
        let namespace = runtime.root.join("staging/bundle-downloads");
        let names = fs::read_dir(&namespace)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert!(!names.iter().any(|name| {
            name.to_string_lossy().starts_with("bundle-")
                && name.to_string_lossy().ends_with(".zip")
        }));
        let held_name = names
            .iter()
            .find(|name| name.to_string_lossy().ends_with(".data"))
            .expect("partial archive bytes should remain recoverable");
        assert!(fs::metadata(namespace.join(held_name)).unwrap().is_file());
        let journal_name = std::ffi::OsString::from(format!(
            "{}.journal",
            held_name.to_string_lossy().trim_end_matches(".data")
        ));
        assert!(namespace.join(&journal_name).exists());
        assert!(namespace
            .join(format!("{}.verified", journal_name.to_string_lossy()))
            .exists());
    }

    #[test]
    fn bundle_download_quota_refuses_before_file_creation_and_releases_reservation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        fs::write(&source, triangle_stl(0.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        let bundle_id = runtime
            .mutate_db(|db| db.create_bundle(&[model.id]))
            .unwrap();
        let context = RequestContext {
            principal_id: "quota".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "quota".into(),
        };
        let preview = runtime
            .prepare_bundle_preview(&context, &bundle_id)
            .unwrap();
        runtime.set_temp_limits_for_test(1, 1024);
        assert_eq!(
            runtime
                .issue_bundle_download(&context, &preview.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        assert!(!runtime.root.join("staging/bundle-downloads").exists());
        let resources = runtime.temp_resources.lock().unwrap();
        assert_eq!(resources.reserved_owned_bytes, 0);
        assert_eq!(resources.active_bundle_builds, 0);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn standard_metadata_proposal_reads_five_fields_without_applying_or_source_writes() {
        fn package(name: &str, bytes: &[u8], method: zip::CompressionMethod) -> Vec<u8> {
            let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            archive
                .start_file(
                    name,
                    zip::write::SimpleFileOptions::default().compression_method(method),
                )
                .unwrap();
            archive.write_all(bytes).unwrap();
            archive.finish().unwrap().into_inner()
        }
        let root = r#"<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
<metadata name="AuThOr"> Embedded Designer </metadata>
<metadata name="URL"> https://example.test/source </metadata>
<metadata name="Notes"> Embedded notes </metadata>
<metadata name="TERMS"> CC0 </metadata>
<metadata name="Parent_Model"> Parent Family </metadata>
<metadata name="Unknown">Ignored value</metadata>
<metadata name="Designer">Must not replace first value</metadata>
<resources><object id="1" type="model"><mesh><vertices>
<vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/>
</vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources>
<build><item objectid="1"/></build></model>"#;
        for method in [
            zip::CompressionMethod::Stored,
            zip::CompressionMethod::Deflated,
        ] {
            let inner = package("3D/3dmodel.model", root.as_bytes(), method);
            for archived in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let base = temp.path().canonicalize().unwrap();
                let sources = base.join("sources");
                fs::create_dir(&sources).unwrap();
                let (name, original) = if archived {
                    ("models.zip", package("nested.3mf", &inner, method))
                } else {
                    ("loose.3mf", inner.clone())
                };
                let source = sources.join(name);
                fs::write(&source, &original).unwrap();
                let runtime = LibraryRuntime::open(RuntimeConfig {
                    library_root: base.join("library"),
                    integration_settings_path: base.join("integrations.json"),
                    roots: vec![RuntimeRootPolicy {
                        id: "metadata-fixture".into(),
                        label: "Synthetic metadata".into(),
                        path: sources,
                        access: RootAccess::ReadOnly,
                        catalog_root_id: None,
                    }],
                    capabilities: BTreeSet::from([
                        Capability::CatalogRead,
                        Capability::CatalogWrite,
                        Capability::MediaRead,
                        Capability::MediaWrite,
                        Capability::Scan,
                    ]),
                    local_catalog_root_access: false,
                })
                .unwrap();
                let mut index = runtime
                    .read_db(|db| db.list_index_roots())
                    .unwrap()
                    .remove(0);
                index.scan_zip = archived;
                let index = runtime
                    .mutate_db(|db| db.update_index_root(&index))
                    .unwrap();
                crate::scanner::scan_root(&runtime, &index).unwrap();
                let model = runtime
                    .read_db(|db| db.list_models(None, None))
                    .unwrap()
                    .into_iter()
                    .find(|model| {
                        if archived {
                            model.source_archive_member.as_deref() == Some("nested.3mf")
                        } else {
                            model.display_name == "loose"
                        }
                    })
                    .unwrap();
                let context = RequestContext {
                    principal_id: "metadata-reader".into(),
                    capabilities: BTreeSet::from([Capability::CatalogRead]),
                    request_id: "metadata-reader".into(),
                };
                let proposal = {
                    let probe = embedded::StandardMetadataProbeGuard::install();
                    let proposal = runtime
                        .read_embedded_metadata(&context, &model.id, model.content_hash.as_deref())
                        .unwrap();
                    assert_eq!(probe.copies(), 5);
                    assert_eq!(probe.reserve_attempts(), 5);
                    assert_eq!(
                        probe.requested_lengths(),
                        vec![
                            "Embedded Designer".len(),
                            "https://example.test/source".len(),
                            "Embedded notes".len(),
                            "CC0".len(),
                            "Parent Family".len(),
                        ]
                    );
                    proposal
                };
                assert_eq!(
                    proposal.embedded.designer.as_deref(),
                    Some("Embedded Designer")
                );
                assert_eq!(
                    proposal.embedded.source_url.as_deref(),
                    Some("https://example.test/source")
                );
                assert_eq!(proposal.embedded.notes.as_deref(), Some("Embedded notes"));
                assert_eq!(proposal.embedded.model_license.as_deref(), Some("CC0"));
                assert_eq!(
                    proposal.embedded.parent_model.as_deref(),
                    Some("Parent Family")
                );
                assert!(proposal.current.designer.is_none());
                assert!(proposal.current.source_url.is_none());
                assert!(proposal.current.notes.is_none());
                assert!(proposal.current.model_license.is_none());
                assert!(proposal.current.parent_model.is_none());
                {
                    let probe = embedded::StandardMetadataProbeGuard::install();
                    probe.fail_reserve_at(1);
                    probe.stop_at_finish(embedded::StandardMetadataTestStop::Resource(
                        stl_core::ResourceStop::EnvelopeChanged,
                    ));
                    let error = runtime
                        .read_embedded_metadata(&context, &model.id, model.content_hash.as_deref())
                        .unwrap_err();
                    assert_eq!(error.code, RuntimeErrorCode::Internal);
                    assert!(!error.retryable);
                    assert_eq!(probe.copies(), 0);
                    assert_eq!(probe.reserve_attempts(), 1);
                }
                for (stop, code, retryable) in [
                    (
                        stl_core::ResourceStop::InputLimitExceeded,
                        RuntimeErrorCode::Unsupported,
                        false,
                    ),
                    (
                        stl_core::ResourceStop::ReadWorkExceeded,
                        RuntimeErrorCode::Unsupported,
                        false,
                    ),
                    (
                        stl_core::ResourceStop::EnvelopeChanged,
                        RuntimeErrorCode::Stale,
                        true,
                    ),
                ] {
                    let probe = embedded::StandardMetadataProbeGuard::install();
                    probe.stop_after_reserve(embedded::StandardMetadataTestStop::Resource(stop));
                    let error = runtime
                        .read_embedded_metadata(&context, &model.id, model.content_hash.as_deref())
                        .unwrap_err();
                    assert_eq!(error.code, code);
                    assert_eq!(error.retryable, retryable);
                    assert_eq!(probe.copies(), 0);
                    assert_eq!(probe.reserve_attempts(), 1);
                }
                {
                    let probe = embedded::StandardMetadataProbeGuard::install();
                    probe.stop_at_finish(embedded::StandardMetadataTestStop::Resource(
                        stl_core::ResourceStop::ReadWorkExceeded,
                    ));
                    let error = runtime
                        .read_embedded_metadata(&context, &model.id, model.content_hash.as_deref())
                        .unwrap_err();
                    assert_eq!(error.code, RuntimeErrorCode::Unsupported);
                    assert!(!error.retryable);
                    assert_eq!(probe.copies(), 5);
                }
                let stored = runtime
                    .read_db(|db| db.get_model(&model.id))
                    .unwrap()
                    .unwrap();
                assert_eq!(stored.metadata_version, model.metadata_version);
                assert!(stored.metadata.designer.is_none());
                assert!(stored.metadata.source_url.is_none());
                assert!(stored.metadata.notes.is_none());
                assert!(stored.metadata.model_license.is_none());
                assert!(stored.metadata.parent_model_id.is_none());
                assert_eq!(fs::read(&source).unwrap(), original);
            }
        }
    }

    #[test]
    fn embedded_3mf_metadata_requires_explicit_field_selection_and_source_hash() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("metadata.3mf");
        fs::write(&source, metadata_3mf()).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let parent = runtime
            .mutate_db(|db| db.create_model("Parent Family", None))
            .unwrap();
        let model = runtime
            .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
            .unwrap();
        let context = RequestContext {
            principal_id: "metadata".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
            request_id: "metadata".into(),
        };
        assert_eq!(
            runtime
                .read_embedded_metadata(&context, &model.id, Some("00"))
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
        let proposal = runtime
            .read_embedded_metadata(&context, &model.id, model.content_hash.as_deref())
            .unwrap();
        assert_eq!(
            proposal.embedded.designer.as_deref(),
            Some("Embedded Designer")
        );
        assert_eq!(proposal.embedded.notes.as_deref(), Some("Embedded notes"));
        assert_eq!(
            proposal.embedded.parent_model.as_deref(),
            Some("Parent Family")
        );
        let changed = runtime
            .apply_embedded_metadata(
                &context,
                VersionedTarget {
                    model_id: model.id.clone(),
                    expected_version: model.metadata_version,
                },
                model.content_hash.as_deref().unwrap(),
                EmbeddedMetadataSelection {
                    designer: true,
                    notes: true,
                    parent_model: true,
                    ..EmbeddedMetadataSelection::default()
                },
            )
            .unwrap();
        let stored = runtime
            .read_db(|db| db.get_model(&model.id))
            .unwrap()
            .unwrap();
        assert_eq!(changed.metadata_version, stored.metadata_version);
        assert_eq!(
            stored.metadata.designer.as_deref(),
            Some("Embedded Designer")
        );
        assert_eq!(stored.metadata.notes.as_deref(), Some("Embedded notes"));
        assert_eq!(
            stored.metadata.parent_model_id.as_deref(),
            Some(parent.id.as_str())
        );
        assert!(stored.metadata.model_license.is_none());
    }

    #[test]
    fn catalog_pages_are_bounded_path_free_and_stale_after_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        runtime.mutate_db(|db| db.create_model("A", None)).unwrap();
        runtime.mutate_db(|db| db.create_model("B", None)).unwrap();
        let context = RequestContext {
            principal_id: "reader".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead]),
            request_id: "page".into(),
        };
        let first = runtime
            .list_models(
                &context,
                ModelQuery::default(),
                PageRequest {
                    cursor: None,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].folder_id, library_core::ROOT_FOLDER_ID);
        let by_name = runtime
            .list_models(
                &context,
                ModelQuery {
                    sort: ModelSort::Name,
                    direction: SortDirection::Desc,
                    ..ModelQuery::default()
                },
                PageRequest {
                    cursor: None,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(by_name.items[0].display_name, "B");
        let by_name_second = runtime
            .list_models(
                &context,
                ModelQuery {
                    sort: ModelSort::Name,
                    direction: SortDirection::Desc,
                    ..ModelQuery::default()
                },
                PageRequest {
                    cursor: by_name.next_cursor,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(by_name_second.items[0].display_name, "A");
        let cursor = first.next_cursor.clone().unwrap();
        let second = runtime
            .list_models(
                &context,
                ModelQuery::default(),
                PageRequest {
                    cursor: Some(cursor.clone()),
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(second.items.len(), 1);
        assert_ne!(first.items[0].id, second.items[0].id);
        runtime.mutate_db(|db| db.create_model("C", None)).unwrap();
        let stale = runtime
            .list_models(
                &context,
                ModelQuery::default(),
                PageRequest {
                    cursor: Some(cursor),
                    limit: 1,
                },
            )
            .unwrap_err();
        assert_eq!(stale.code, RuntimeErrorCode::Stale);
        let status = runtime.runtime_status(&context).unwrap();
        assert!(status.catalog_revision > first.snapshot_revision);
        assert!(!runtime
            .events_since(&context, 0, PageRequest::default())
            .unwrap()
            .items
            .is_empty());
    }

    #[test]
    fn metadata_bulk_update_is_atomic_on_version_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let first = runtime.mutate_db(|db| db.create_model("A", None)).unwrap();
        let second = runtime.mutate_db(|db| db.create_model("B", None)).unwrap();
        let context = RequestContext {
            principal_id: "editor".into(),
            capabilities: BTreeSet::from([Capability::CatalogWrite]),
            request_id: "metadata".into(),
        };
        let patch = MetadataPatch {
            designer: Some("Designer".into()),
            ..MetadataPatch::default()
        };
        let error = runtime
            .bulk_update_metadata(
                &context,
                vec![
                    VersionedTarget {
                        model_id: first.id.clone(),
                        expected_version: first.metadata_version,
                    },
                    VersionedTarget {
                        model_id: second.id.clone(),
                        expected_version: second.metadata_version + 1,
                    },
                ],
                patch.clone(),
            )
            .unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::Conflict);
        assert!(runtime
            .read_db(|db| db.get_model(&first.id))
            .unwrap()
            .unwrap()
            .metadata
            .designer
            .is_none());

        let changed = runtime
            .bulk_update_metadata(
                &context,
                vec![
                    VersionedTarget {
                        model_id: first.id.clone(),
                        expected_version: first.metadata_version,
                    },
                    VersionedTarget {
                        model_id: second.id.clone(),
                        expected_version: second.metadata_version,
                    },
                ],
                patch,
            )
            .unwrap();
        assert_eq!(changed.len(), 2);
        assert!(changed
            .iter()
            .all(|model| model.metadata_version == first.metadata_version + 1));
    }

    #[test]
    fn parent_metadata_empty_string_clears_and_omission_preserves() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let parent = runtime
            .mutate_db(|db| db.create_model("Parent", None))
            .unwrap();
        let child = runtime
            .mutate_db(|db| db.create_model("Child", None))
            .unwrap();
        let context = RequestContext {
            principal_id: "editor".into(),
            capabilities: BTreeSet::from([Capability::CatalogWrite]),
            request_id: "parent".into(),
        };
        let set = runtime
            .update_metadata(
                &context,
                VersionedTarget {
                    model_id: child.id.clone(),
                    expected_version: child.metadata_version,
                },
                MetadataPatch {
                    parent_model_id: Some(parent.id.clone()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        let preserved = runtime
            .update_metadata(
                &context,
                VersionedTarget {
                    model_id: child.id.clone(),
                    expected_version: set.metadata_version,
                },
                MetadataPatch {
                    notes: Some("updated".into()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        assert_eq!(
            runtime
                .read_db(|db| db.get_model(&child.id))
                .unwrap()
                .unwrap()
                .metadata
                .parent_model_id,
            Some(parent.id)
        );
        runtime
            .update_metadata(
                &context,
                VersionedTarget {
                    model_id: child.id.clone(),
                    expected_version: preserved.metadata_version,
                },
                MetadataPatch {
                    parent_model_id: Some(String::new()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        assert!(runtime
            .read_db(|db| db.get_model(&child.id))
            .unwrap()
            .unwrap()
            .metadata
            .parent_model_id
            .is_none());
    }

    #[test]
    fn management_remove_restore_tokens_are_versioned_one_use_and_principal_bound() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.create_model("Managed", None))
            .unwrap();
        let context = RequestContext {
            principal_id: "manager".into(),
            capabilities: BTreeSet::from([Capability::CatalogWrite]),
            request_id: "remove".into(),
        };
        let plan = runtime
            .plan_action(
                &context,
                ManagementAction::RecoverableRemove {
                    targets: vec![VersionedTarget {
                        model_id: model.id.clone(),
                        expected_version: model.metadata_version,
                    }],
                },
            )
            .unwrap();
        let other = RequestContext {
            principal_id: "other".into(),
            capabilities: context.capabilities.clone(),
            request_id: "other".into(),
        };
        assert_eq!(
            runtime
                .execute_action(&other, &plan.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        let removed = runtime.execute_action(&context, &plan.token).unwrap();
        let snapshot_id = match removed {
            ManagementResult::Removed { snapshot_id, .. } => snapshot_id,
            _ => panic!("unexpected management result"),
        };
        assert_eq!(
            runtime
                .execute_action(&context, &plan.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
        let restore = runtime
            .plan_action(
                &context,
                ManagementAction::RestoreRemoval {
                    snapshot_id: snapshot_id.clone(),
                },
            )
            .unwrap();
        assert!(matches!(
            runtime.execute_action(&context, &restore.token).unwrap(),
            ManagementResult::RestoredRemoval { .. }
        ));
        assert!(runtime
            .read_db(|db| db.get_model(&model.id))
            .unwrap()
            .is_some());
    }

    #[cfg(unix)]
    fn writable_source_fixture(
        temp: &Path,
    ) -> (
        Arc<LibraryRuntime>,
        RequestContext,
        library_core::Model,
        PathBuf,
        String,
    ) {
        let source_root = temp.join("models");
        let destination_root = temp.join("destination");
        fs::create_dir(&source_root).unwrap();
        fs::create_dir(&destination_root).unwrap();
        let source = source_root.join("part.stl");
        let bytes = triangle_stl(0.0);
        fs::write(&source, &bytes).unwrap();

        let capabilities = BTreeSet::from([
            Capability::CatalogRead,
            Capability::CatalogWrite,
            Capability::Scan,
            Capability::PhysicalSourceWrite,
        ]);
        let mut runtime_config = config(temp.join("library"));
        runtime_config.local_catalog_root_access = false;
        runtime_config.roots = vec![
            RuntimeRootPolicy {
                id: "models".into(),
                catalog_root_id: None,
                label: "Models".into(),
                path: source_root,
                access: RootAccess::Writable,
            },
            RuntimeRootPolicy {
                id: "destination".into(),
                catalog_root_id: None,
                label: "Destination".into(),
                path: destination_root,
                access: RootAccess::Writable,
            },
        ];
        runtime_config.capabilities = capabilities.clone();
        let runtime = LibraryRuntime::open(runtime_config).unwrap();
        let context = RequestContext {
            principal_id: "source-manager".into(),
            capabilities,
            request_id: "physical-source-test".into(),
        };
        let source_catalog_id = runtime
            .root_handles(&context)
            .unwrap()
            .into_iter()
            .find(|root| root.label == "Models")
            .unwrap()
            .catalog_root_id
            .unwrap();
        let source_catalog = runtime
            .read_db(|db| {
                db.list_index_roots().map(|roots| {
                    roots
                        .into_iter()
                        .find(|root| root.id == source_catalog_id)
                        .unwrap()
                })
            })
            .unwrap();
        scanner::scan_root(&runtime, &source_catalog).unwrap();
        let model = runtime
            .read_db(|db| {
                db.list_models(None, None)
                    .map(|mut models| models.remove(0))
            })
            .unwrap();
        let destination_handle_id = runtime
            .root_handles(&context)
            .unwrap()
            .into_iter()
            .find(|root| root.label == "Destination")
            .unwrap()
            .id;
        (runtime, context, model, source, destination_handle_id)
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_rejects_catalog_replacement_after_plan() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, context, model, source, _) = writable_source_fixture(temp.path());
        let original_source = fs::read(&source).unwrap();
        let plan = runtime
            .plan_action(
                &context,
                ManagementAction::QuarantineLooseSource {
                    target: VersionedTarget {
                        model_id: model.id.clone(),
                        expected_version: model.metadata_version,
                    },
                },
            )
            .unwrap();

        let catalog = runtime.root.join("library.sqlite3");
        let displaced = runtime.root.join("library.sqlite3.displaced");
        runtime.install_before_action_dispatch_hook(move || {
            fs::rename(&catalog, &displaced).unwrap();
            fs::copy(&displaced, &catalog).unwrap();
        });
        let error = runtime.execute_action(&context, &plan.token).unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::Stale, "{error:?}");
        assert_eq!(fs::read(&source).unwrap(), original_source);
        assert!(LibraryDb::open(&runtime.root)
            .unwrap()
            .get_model(&model.id)
            .unwrap()
            .is_some());
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_rejects_catalog_replacement_after_opening_guarded_connection() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, context, model, source, _) = writable_source_fixture(temp.path());
        let original_source = fs::read(&source).unwrap();
        let plan = runtime
            .plan_action(
                &context,
                ManagementAction::QuarantineLooseSource {
                    target: VersionedTarget {
                        model_id: model.id.clone(),
                        expected_version: model.metadata_version,
                    },
                },
            )
            .unwrap();

        let catalog = runtime.root.join("library.sqlite3");
        let replacement = runtime.root.join("library.sqlite3.replacement");
        let displaced = runtime.root.join("library.sqlite3.displaced");
        fs::copy(&catalog, &replacement).unwrap();
        runtime.install_after_guarded_catalog_open_hook(move || {
            fs::rename(&catalog, &displaced).unwrap();
            fs::rename(&replacement, &catalog).unwrap();
        });

        let error = runtime.execute_action(&context, &plan.token).unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::Stale, "{error:?}");
        assert_eq!(fs::read(&source).unwrap(), original_source);
        assert!(LibraryDb::open(&runtime.root)
            .unwrap()
            .get_model(&model.id)
            .unwrap()
            .is_some());
    }

    #[cfg(unix)]
    #[test]
    fn source_quarantine_and_undo_use_one_use_versioned_plans() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, context, model, source, _) = writable_source_fixture(temp.path());
        let original = fs::read(&source).unwrap();
        let mut unprivileged = context.clone();
        unprivileged
            .capabilities
            .remove(&Capability::PhysicalSourceWrite);
        assert_eq!(
            runtime
                .plan_action(
                    &unprivileged,
                    ManagementAction::QuarantineLooseSource {
                        target: VersionedTarget {
                            model_id: model.id.clone(),
                            expected_version: model.metadata_version,
                        },
                    },
                )
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert_eq!(fs::read(&source).unwrap(), original);
        let plan = runtime
            .plan_action(
                &context,
                ManagementAction::QuarantineLooseSource {
                    target: VersionedTarget {
                        model_id: model.id.clone(),
                        expected_version: model.metadata_version,
                    },
                },
            )
            .unwrap();
        assert_eq!(plan.action_kind, "quarantineLooseSource");
        assert!(!serde_json::to_string(&plan)
            .unwrap()
            .contains(temp.path().to_string_lossy().as_ref()));

        let snapshot_id = match runtime.execute_action(&context, &plan.token).unwrap() {
            ManagementResult::Quarantined {
                snapshot_id,
                model_id,
                source_holds,
            } => {
                assert_eq!(model_id, model.id);
                assert_eq!(source_holds.len(), 1);
                assert_eq!(source_holds[0].model_id, model.id);
                assert_eq!(source_holds[0].relative_path, "part.stl");
                assert_eq!(source_holds[0].bytes, original.len() as u64);
                assert_eq!(
                    source_holds[0].sha256,
                    format!("{:x}", Sha256::digest(&original))
                );
                assert!(!source_holds[0].root_handle_id.is_empty());
                assert!(source_holds[0].hold_name.ends_with(".data"));
                assert!(source_holds[0].journal_name.ends_with(".journal"));
                assert!(!serde_json::to_string(&source_holds)
                    .unwrap()
                    .contains(temp.path().to_string_lossy().as_ref()));
                snapshot_id
            }
            other => panic!("unexpected quarantine result: {other:?}"),
        };
        assert!(!source.exists());
        assert_eq!(
            runtime
                .execute_action(&context, &plan.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );

        let undo = runtime
            .plan_action(
                &context,
                ManagementAction::UndoQuarantine {
                    snapshot_id: snapshot_id.clone(),
                },
            )
            .unwrap();
        assert_eq!(undo.action_kind, "undoQuarantine");
        assert!(matches!(
            runtime.execute_action(&context, &undo.token).unwrap(),
            ManagementResult::QuarantineUndone { .. }
        ));
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(runtime
            .read_db(|db| db.get_model(&model.id))
            .unwrap()
            .is_some());
        assert!(!runtime
            .root
            .join("trash")
            .join(format!("source-{snapshot_id}"))
            .exists());
    }

    #[cfg(unix)]
    #[test]
    fn source_quarantine_rejects_changed_bytes_and_leaves_replacement_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, context, model, source, _) = writable_source_fixture(temp.path());
        let plan = runtime
            .plan_action(
                &context,
                ManagementAction::QuarantineLooseSource {
                    target: VersionedTarget {
                        model_id: model.id.clone(),
                        expected_version: model.metadata_version,
                    },
                },
            )
            .unwrap();
        let replacement = b"changed after planning";
        fs::write(&source, replacement).unwrap();
        assert_eq!(
            runtime
                .execute_action(&context, &plan.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
        assert_eq!(fs::read(&source).unwrap(), replacement);
        assert!(runtime
            .read_db(|db| db.get_model(&model.id))
            .unwrap()
            .is_some());
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_undo_recovers_a_batch_interrupted_after_partial_source_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, context, first, first_source, _) = writable_source_fixture(temp.path());
        let second_source = first_source.with_file_name("part-two.stl");
        let original = fs::read(&first_source).unwrap();
        fs::write(&second_source, &original).unwrap();
        let source_catalog_id = runtime
            .root_handles(&context)
            .unwrap()
            .into_iter()
            .find(|root| root.label == "Models")
            .unwrap()
            .catalog_root_id
            .unwrap();
        let source_catalog = runtime
            .read_db(|db| {
                db.list_index_roots().map(|roots| {
                    roots
                        .into_iter()
                        .find(|root| root.id == source_catalog_id)
                        .unwrap()
                })
            })
            .unwrap();
        scanner::scan_root(&runtime, &source_catalog).unwrap();
        let models = runtime.read_db(|db| db.list_models(None, None)).unwrap();
        let second = models
            .iter()
            .find(|model| {
                model
                    .source_path
                    .as_deref()
                    .is_some_and(|path| path.ends_with("part-two.stl"))
            })
            .unwrap()
            .clone();
        let snapshot_id = "removal-crash-simulation";
        let trash_root = runtime.root.join("trash");
        fs::create_dir_all(&trash_root).unwrap();
        let owned = trash_root.join(format!("source-{snapshot_id}"));
        fs::create_dir(&owned).unwrap();
        for model in [&first, &second] {
            fs::write(owned.join(format!("{}.bin", model.id)), &original).unwrap();
        }
        let targets = [&first, &second]
            .into_iter()
            .map(|model| CoreVersionedTarget {
                model_id: model.id.clone(),
                expected_version: model.metadata_version,
            })
            .collect::<Vec<_>>();
        runtime
            .mutate_db(|db| db.remove_models_recoverably_with_snapshot_id(&targets, snapshot_id))
            .unwrap();
        fs::remove_file(&first_source).unwrap();

        let undo_plan = runtime
            .plan_action(
                &context,
                ManagementAction::UndoQuarantine {
                    snapshot_id: snapshot_id.into(),
                },
            )
            .unwrap();
        assert!(matches!(
            runtime.execute_action(&context, &undo_plan.token).unwrap(),
            ManagementResult::QuarantineUndoneBatch { .. }
        ));
        assert_eq!(fs::read(&first_source).unwrap(), original);
        assert_eq!(fs::read(&second_source).unwrap(), original);
        assert!(runtime
            .read_db(|db| db.get_model(&first.id))
            .unwrap()
            .is_some());
        assert!(runtime
            .read_db(|db| db.get_model(&second.id))
            .unwrap()
            .is_some());
        assert!(!owned.exists());
    }

    #[cfg(unix)]
    #[test]
    fn source_move_is_root_scoped_and_never_overwrites_a_destination() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, context, model, source, destination_handle_id) =
            writable_source_fixture(temp.path());
        let original = fs::read(&source).unwrap();
        let destination_root = temp.path().join("destination");
        let plan = runtime
            .plan_action(
                &context,
                ManagementAction::MoveLooseSource {
                    target: VersionedTarget {
                        model_id: model.id.clone(),
                        expected_version: model.metadata_version,
                    },
                    destination_handle_id: destination_handle_id.clone(),
                    file_name: "moved.stl".into(),
                },
            )
            .unwrap();
        let destination = destination_root.join("moved.stl");
        fs::write(&destination, b"preserve existing file").unwrap();
        assert!(runtime.execute_action(&context, &plan.token).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"preserve existing file");
        assert_eq!(fs::read(&source).unwrap(), original);

        fs::remove_file(&destination).unwrap();
        let plan = runtime
            .plan_action(
                &context,
                ManagementAction::MoveLooseSource {
                    target: VersionedTarget {
                        model_id: model.id.clone(),
                        expected_version: model.metadata_version,
                    },
                    destination_handle_id,
                    file_name: "moved.stl".into(),
                },
            )
            .unwrap();
        match runtime.execute_action(&context, &plan.token).unwrap() {
            ManagementResult::Moved {
                model: moved,
                source_holds,
            } => {
                assert_eq!(
                    moved,
                    MutationVersion {
                        model_id: model.id.clone(),
                        metadata_version: model.metadata_version + 1,
                    }
                );
                assert_eq!(source_holds.len(), 1);
                assert_eq!(source_holds[0].model_id, model.id);
                assert_eq!(source_holds[0].relative_path, "part.stl");
                assert_eq!(source_holds[0].bytes, original.len() as u64);
                assert_eq!(
                    source_holds[0].sha256,
                    format!("{:x}", Sha256::digest(&original))
                );
                assert!(!serde_json::to_string(&source_holds)
                    .unwrap()
                    .contains(temp.path().to_string_lossy().as_ref()));
            }
            other => panic!("unexpected move result: {other:?}"),
        }
        assert!(!source.exists());
        assert_eq!(fs::read(&destination).unwrap(), original);
        assert_eq!(
            runtime
                .read_db(|db| db.get_model(&model.id))
                .unwrap()
                .unwrap()
                .source_path
                .as_deref(),
            Some(
                destination
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .as_ref()
            )
        );
    }

    #[test]
    fn management_backup_export_and_restore_use_only_admitted_handle_files() {
        let temp = tempfile::tempdir().unwrap();
        let transfer = temp.path().join("transfer");
        fs::create_dir(&transfer).unwrap();
        let mut runtime_config = config(temp.path().join("library"));
        runtime_config.roots.push(RuntimeRootPolicy {
            id: "transfer".into(),
            catalog_root_id: None,
            label: "Transfer".into(),
            path: transfer.clone(),
            access: RootAccess::Writable,
        });
        let runtime = LibraryRuntime::open(runtime_config).unwrap();
        let first = runtime
            .mutate_db(|db| db.create_model("First", None))
            .unwrap();
        let context = RequestContext {
            principal_id: "manager".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::Maintenance,
                Capability::PhysicalSourceWrite,
            ]),
            request_id: "backup".into(),
        };
        let backup = runtime
            .plan_action(
                &context,
                ManagementAction::CreateBackup {
                    destination_handle_id: "transfer".into(),
                    file_name: "backup.zip".into(),
                },
            )
            .unwrap();
        assert!(matches!(
            runtime.execute_action(&context, &backup.token).unwrap(),
            ManagementResult::BackupCreated { .. }
        ));
        assert!(transfer.join("backup.zip").is_file());
        let export = runtime
            .plan_action(
                &context,
                ManagementAction::ExportSelection {
                    targets: vec![VersionedTarget {
                        model_id: first.id.clone(),
                        expected_version: first.metadata_version,
                    }],
                    destination_handle_id: "transfer".into(),
                    file_name: "selection.zip".into(),
                },
            )
            .unwrap();
        assert!(matches!(
            runtime.execute_action(&context, &export.token).unwrap(),
            ManagementResult::SelectionExported { models: 1, .. }
        ));
        runtime
            .mutate_db(|db| db.create_model("Later", None))
            .unwrap();
        let changed_restore = runtime
            .plan_action(
                &context,
                ManagementAction::RestoreBackup {
                    source_handle_id: "transfer".into(),
                    file_name: "backup.zip".into(),
                    root_mappings: vec![],
                },
            )
            .unwrap();
        let backup_bytes = fs::read(transfer.join("backup.zip")).unwrap();
        let mut changed_bytes = backup_bytes.clone();
        changed_bytes.push(0);
        fs::write(transfer.join("backup.zip"), changed_bytes).unwrap();
        assert_eq!(
            runtime
                .execute_action(&context, &changed_restore.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
        assert_eq!(
            runtime
                .read_db(|db| db.list_models(None, None))
                .unwrap()
                .len(),
            2
        );
        fs::write(transfer.join("backup.zip"), backup_bytes).unwrap();
        let restore = runtime
            .plan_action(
                &context,
                ManagementAction::RestoreBackup {
                    source_handle_id: "transfer".into(),
                    file_name: "backup.zip".into(),
                    root_mappings: vec![],
                },
            )
            .unwrap();
        assert!(matches!(
            runtime.execute_action(&context, &restore.token).unwrap(),
            ManagementResult::BackupRestored { models: 1, .. }
        ));
        assert_eq!(
            runtime
                .read_db(|db| db.list_models(None, None))
                .unwrap()
                .len(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn purge_recovery_uses_pinned_parent_after_parent_path_swap() {
        let temp = tempfile::tempdir().unwrap();
        let parent_path = temp.path().join("parent");
        fs::create_dir(&parent_path).unwrap();
        let library = parent_path.join("library");
        LibraryDb::open(&library)
            .unwrap()
            .create_model("Original sentinel", None)
            .unwrap();
        let canonical_root = library.canonicalize().unwrap();
        let nonce = "8".repeat(32);
        let staged_name = format!(".gogglelab-empty-{nonce}");
        let previous_name = format!("library.previous-purge-{nonce}");
        let staged = parent_path.join(&staged_name);
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("stage-sentinel"), b"owned stage").unwrap();
        let journal = PurgeJournal {
            version: 1,
            root_path: canonical_root.to_str().unwrap().to_owned(),
            nonce: nonce.clone(),
            staged_name,
            previous_name: previous_name.clone(),
            committed: false,
        };
        let journal_path = write_purge_journal(&canonical_root, &journal).unwrap();
        let previous = parent_path.join(&previous_name);
        fs::rename(&canonical_root, &previous).unwrap();

        let moved_parent = temp.path().join("pinned-parent");
        let decoy_parent = parent_path.clone();
        let decoy_previous = decoy_parent.join(&previous_name);
        let decoy_staged = decoy_parent.join(&journal.staged_name);
        let journal_name = journal_path.file_name().unwrap().to_os_string();
        let decoy_journal = decoy_parent.join(&journal_name);
        let encoded_journal = serde_json::to_vec(&journal).unwrap();

        recover_interrupted_catalog_purge_with_hook(&canonical_root, || {
            fs::rename(&parent_path, &moved_parent).unwrap();
            fs::create_dir(&decoy_parent).unwrap();
            let decoy_root = decoy_parent.join("library");
            LibraryDb::open(&decoy_root)
                .unwrap()
                .create_model("Decoy sentinel", None)
                .unwrap();
            fs::create_dir(&decoy_previous).unwrap();
            fs::write(decoy_previous.join("sentinel"), b"decoy previous").unwrap();
            fs::create_dir(&decoy_staged).unwrap();
            fs::write(decoy_staged.join("sentinel"), b"decoy stage").unwrap();
            fs::write(&decoy_journal, &encoded_journal).unwrap();
        })
        .unwrap();

        let restored = moved_parent.join("library");
        assert!(
            restored.is_dir(),
            "recovery must restore the pinned Library"
        );
        let restored_db = LibraryDb::open(&restored).unwrap();
        let restored_models = restored_db.list_models(None, None).unwrap();
        assert_eq!(restored_models.len(), 1);
        assert_eq!(restored_models[0].display_name, "Original sentinel");
        assert!(!moved_parent.join(&previous_name).exists());
        assert!(!moved_parent.join(&journal.staged_name).exists());
        assert!(!moved_parent.join(&journal_name).exists());

        let decoy_db = LibraryDb::open(decoy_parent.join("library")).unwrap();
        assert_eq!(
            decoy_db.list_models(None, None).unwrap()[0].display_name,
            "Decoy sentinel"
        );
        assert_eq!(
            fs::read(decoy_previous.join("sentinel")).unwrap(),
            b"decoy previous"
        );
        assert_eq!(
            fs::read(decoy_staged.join("sentinel")).unwrap(),
            b"decoy stage"
        );
        assert_eq!(fs::read(&decoy_journal).unwrap(), encoded_journal);
    }

    #[cfg(unix)]
    #[test]
    fn startup_recovers_catalog_purge_interrupted_after_moving_the_original_library() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        {
            let db = LibraryDb::open(&library).unwrap();
            db.create_model("Recovery sentinel", None).unwrap();
        }
        let canonical_root = library.canonicalize().unwrap();
        let parent = canonical_root.parent().unwrap();
        let nonce = "1".repeat(32);
        let staged_name = format!(".gogglelab-empty-{nonce}");
        let previous_name = format!("library.previous-purge-{nonce}");
        let staged = parent.join(&staged_name);
        let previous = parent.join(&previous_name);
        fs::create_dir(&staged).unwrap();
        drop(LibraryDb::open(&staged).unwrap());
        write_purge_stage_marker(&staged, &nonce).unwrap();
        let journal = PurgeJournal {
            version: 1,
            root_path: canonical_root.to_str().unwrap().to_owned(),
            nonce: nonce.clone(),
            staged_name,
            previous_name,
            committed: false,
        };
        let journal_path = write_purge_journal(&canonical_root, &journal).unwrap();
        fs::rename(&canonical_root, &previous).unwrap();

        let mut runtime_config = config(library.clone());
        runtime_config.local_catalog_root_access = false;
        runtime_config.capabilities.insert(Capability::CatalogRead);
        let runtime = LibraryRuntime::open(runtime_config).unwrap();

        let models = runtime.read_db(|db| db.list_models(None, None)).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].display_name, "Recovery sentinel");
        assert!(library.is_dir());
        assert!(!previous.exists());
        assert!(!staged.exists());
        assert!(!journal_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn startup_preserves_interrupted_empty_catalog_and_restores_previous_library() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        {
            let db = LibraryDb::open(&library).unwrap();
            db.create_model("Recovery sentinel", None).unwrap();
        }
        let canonical_root = library.canonicalize().unwrap();
        let parent = canonical_root.parent().unwrap();
        let nonce = "2".repeat(32);
        let staged_name = format!(".gogglelab-empty-{nonce}");
        let previous_name = format!("library.previous-purge-{nonce}");
        let staged = parent.join(&staged_name);
        let previous = parent.join(&previous_name);
        fs::create_dir(&staged).unwrap();
        drop(LibraryDb::open(&staged).unwrap());
        write_purge_stage_marker(&staged, &nonce).unwrap();
        let journal = PurgeJournal {
            version: 1,
            root_path: canonical_root.to_str().unwrap().to_owned(),
            nonce: nonce.clone(),
            staged_name,
            previous_name,
            committed: false,
        };
        let journal_path = write_purge_journal(&canonical_root, &journal).unwrap();
        fs::rename(&canonical_root, &previous).unwrap();
        fs::rename(&staged, &canonical_root).unwrap();
        let interrupted_empty = parent.join(format!(".gogglelab-interrupted-purge-{nonce}"));

        let mut runtime_config = config(library.clone());
        runtime_config.local_catalog_root_access = false;
        runtime_config.capabilities.insert(Capability::CatalogRead);
        let runtime = LibraryRuntime::open(runtime_config).unwrap();

        let models = runtime.read_db(|db| db.list_models(None, None)).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].display_name, "Recovery sentinel");
        assert!(library.is_dir());
        assert!(!previous.exists());
        assert!(interrupted_empty.is_dir());
        assert!(purge_stage_marker_matches(&interrupted_empty, &nonce).unwrap());
        assert!(!journal_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn startup_removes_an_unmarked_stage_owned_by_a_published_purge_journal() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        {
            let db = LibraryDb::open(&library).unwrap();
            db.create_model("Recovery sentinel", None).unwrap();
        }
        let canonical_root = library.canonicalize().unwrap();
        let nonce = "7".repeat(32);
        let staged_name = format!(".gogglelab-empty-{nonce}");
        let staged = canonical_root.parent().unwrap().join(&staged_name);
        let journal = PurgeJournal {
            version: 1,
            root_path: canonical_root.to_str().unwrap().to_owned(),
            nonce: nonce.clone(),
            staged_name,
            previous_name: format!("library.previous-purge-{nonce}"),
            committed: false,
        };
        let journal_path = write_purge_journal(&canonical_root, &journal).unwrap();
        fs::create_dir(&staged).unwrap();
        drop(LibraryDb::open(&staged).unwrap());

        recover_interrupted_catalog_purge(&canonical_root).unwrap();

        assert!(canonical_root.is_dir());
        assert!(!staged.exists());
        assert!(!journal_path.exists());
        let db = LibraryDb::open(&canonical_root).unwrap();
        assert_eq!(db.list_models(None, None).unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn startup_preserves_unknown_unpublished_purge_bytes_without_touching_library() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        {
            let db = LibraryDb::open(&library).unwrap();
            db.create_model("Recovery sentinel", None).unwrap();
        }
        let canonical_root = library.canonicalize().unwrap();
        let journal_path = purge_journal_path(&canonical_root).unwrap();
        let temporary = purge_journal_temp_path(&journal_path);
        fs::write(&temporary, b"partial unpublished journal").unwrap();

        assert!(recover_interrupted_catalog_purge(&canonical_root).is_err());

        assert_eq!(
            fs::read(&temporary).unwrap(),
            b"partial unpublished journal"
        );
        assert!(!journal_path.exists());
        let db = LibraryDb::open(&canonical_root).unwrap();
        assert_eq!(db.list_models(None, None).unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn purge_recovery_refuses_a_symlinked_configured_library_root() {
        let temp = tempfile::tempdir().unwrap();
        let actual_library = temp.path().join("actual-library");
        {
            let db = LibraryDb::open(&actual_library).unwrap();
            db.create_model("Protected sentinel", None).unwrap();
        }
        let configured_library = temp.path().join("library-link");
        std::os::unix::fs::symlink(&actual_library, &configured_library).unwrap();
        let configured_path_with_trailing_separator =
            PathBuf::from(format!("{}/", configured_library.display()));

        let error = recover_interrupted_catalog_purge(&configured_path_with_trailing_separator)
            .unwrap_err();

        assert_eq!(error.code, RuntimeErrorCode::Corrupt);
        let db = LibraryDb::open(&actual_library).unwrap();
        assert_eq!(db.list_models(None, None).unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn purge_recovery_rejects_a_previous_directory_name_not_bound_to_its_nonce() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        {
            let db = LibraryDb::open(&library).unwrap();
            db.create_model("Protected sentinel", None).unwrap();
        }
        let canonical_root = library.canonicalize().unwrap();
        let nonce = "3".repeat(32);
        let journal = PurgeJournal {
            version: 1,
            root_path: canonical_root.to_str().unwrap().to_owned(),
            nonce: nonce.clone(),
            staged_name: format!(".gogglelab-empty-{nonce}"),
            previous_name: format!("library.previous-purge-{nonce}"),
            committed: false,
        };
        let journal_path = write_purge_journal(&canonical_root, &journal).unwrap();
        let mut journal_value: serde_json::Value =
            serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        journal_value["previousName"] =
            serde_json::json!(format!("library.previous-purge-{nonce}-unexpected"));
        fs::write(&journal_path, serde_json::to_vec(&journal_value).unwrap()).unwrap();
        let previous = canonical_root
            .parent()
            .unwrap()
            .join(format!("library.previous-purge-{nonce}"));
        fs::rename(&canonical_root, &previous).unwrap();

        let error = recover_interrupted_catalog_purge(&canonical_root).unwrap_err();

        assert_eq!(error.code, RuntimeErrorCode::Corrupt);
        assert!(!canonical_root.exists());
        assert!(previous.is_dir());
        assert!(journal_path.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn generated_journal_cleanup_preserves_unowned_scratch_and_main_authority() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let parent = GuardedDirectoryParent::open(root.parent().unwrap()).unwrap();
        let nonce = "a".repeat(32);
        let document = PurgeJournal {
            version: 1,
            root_path: root.to_string_lossy().into_owned(),
            nonce: nonce.clone(),
            staged_name: format!(".gogglelab-empty-{nonce}"),
            previous_name: format!("library.previous-purge-{nonce}"),
            committed: false,
        };
        let mut journal = GeneratedTransactionJournal::create(
            &parent,
            &root,
            GeneratedControlKind::Purge,
            &serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();
        let scratch = root.parent().unwrap().join(
            journal
                .namespace
                .public_name(GeneratedControlRole::Commit)
                .unwrap(),
        );
        fs::write(&scratch, b"unknown replacement; preserve these bytes").unwrap();
        assert!(journal.clear(&parent).is_err());
        assert_eq!(
            fs::read(&scratch).unwrap(),
            b"unknown replacement; preserve these bytes"
        );
        assert!(root.parent().unwrap().join(journal.public_name()).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn finalized_journal_cleanup_cannot_readmit_a_reappearing_public_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let parent = GuardedDirectoryParent::open(root.parent().unwrap()).unwrap();
        let nonce = "f".repeat(32);
        let document = PurgeJournal {
            version: 1,
            root_path: root.to_string_lossy().into_owned(),
            nonce: nonce.clone(),
            staged_name: format!(".gogglelab-empty-{nonce}"),
            previous_name: format!("library.previous-purge-{nonce}"),
            committed: false,
        };
        let bytes = serde_json::to_vec(&document).unwrap();
        let mut journal = GeneratedTransactionJournal::create(
            &parent,
            &root,
            GeneratedControlKind::Purge,
            &bytes,
        )
        .unwrap();
        journal.clear(&parent).unwrap();
        let replacement = root.parent().unwrap().join(journal.public_name());
        fs::write(&replacement, &bytes).unwrap();
        assert!(journal.clear(&parent).is_err());
        assert_eq!(fs::read(replacement).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn generated_journal_recovery_handles_both_commit_scratch_forms_without_holds() {
        let temp = tempfile::tempdir().unwrap();
        for exchanged in [false, true] {
            let root = temp.path().join(if exchanged { "after" } else { "before" });
            fs::create_dir(&root).unwrap();
            fs::write(root.join("sentinel"), b"preserve catalog data").unwrap();
            let root = root.canonicalize().unwrap();
            let parent = GuardedDirectoryParent::open(root.parent().unwrap()).unwrap();
            let nonce = "b".repeat(32);
            let document = RestoreJournal {
                version: 1,
                root_path: root.to_string_lossy().into_owned(),
                nonce: nonce.clone(),
                staged_name: format!(".gogglelab-restore-{nonce}"),
                previous_name: format!(
                    "{}.previous-{nonce}",
                    root.file_name().unwrap().to_string_lossy()
                ),
                failed_name: format!(".gogglelab-failed-restore-{nonce}"),
                committed: false,
            };
            let mut journal = GeneratedTransactionJournal::create(
                &parent,
                &root,
                GeneratedControlKind::Restore,
                &serde_json::to_vec(&document).unwrap(),
            )
            .unwrap();
            if exchanged {
                journal.mark_committed(&parent).unwrap();
            } else {
                let mut committed = serde_json::to_value(&document).unwrap();
                committed["committed"] = serde_json::json!(true);
                fs::write(
                    root.parent().unwrap().join(
                        journal
                            .namespace
                            .public_name(GeneratedControlRole::Commit)
                            .unwrap(),
                    ),
                    serde_json::to_vec(&committed).unwrap(),
                )
                .unwrap();
            }
            drop(journal);
            let mut recovered =
                GeneratedTransactionJournal::recover(&parent, &root, GeneratedControlKind::Restore)
                    .unwrap()
                    .unwrap();
            assert_eq!(recovered.committed(), exchanged);
            recovered.clear(&parent).unwrap();
            assert_eq!(
                fs::read(root.join("sentinel")).unwrap(),
                b"preserve catalog data"
            );
        }
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn startup_recovers_a_privately_claimed_main_journal_and_marker() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("sentinel"), b"preserve current library").unwrap();
        let root = root.canonicalize().unwrap();
        let parent = GuardedDirectoryParent::open(root.parent().unwrap()).unwrap();
        let nonce = "c".repeat(32);
        let document = PurgeJournal {
            version: 1,
            root_path: root.to_string_lossy().into_owned(),
            nonce: nonce.clone(),
            staged_name: format!(".gogglelab-empty-{nonce}"),
            previous_name: format!("library.previous-purge-{nonce}"),
            committed: false,
        };
        let mut journal = GeneratedTransactionJournal::create(
            &parent,
            &root,
            GeneratedControlKind::Purge,
            &serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();
        journal.mark_committed(&parent).unwrap();
        let directory = GuardedDirectory::open(&root).unwrap();
        let marker = create_generated_purge_marker(&directory, &root, &nonce).unwrap();
        let marker_claim = format!(
            "{}.cleanup-{nonce}-marker-{}",
            journal.public_name().to_string_lossy(),
            "d".repeat(32)
        );
        fs::rename(
            root.join(marker.current_name().unwrap()),
            root.join(marker_claim),
        )
        .unwrap();
        let main_claim = format!(
            "{}.cleanup-{nonce}-main-{}",
            journal.public_name().to_string_lossy(),
            "e".repeat(32)
        );
        fs::rename(
            root.parent().unwrap().join(journal.public_name()),
            root.parent().unwrap().join(main_claim),
        )
        .unwrap();
        drop(marker);
        drop(journal);
        recover_interrupted_catalog_purge(&root).unwrap();
        assert_eq!(
            fs::read(root.join("sentinel")).unwrap(),
            b"preserve current library"
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn successful_transaction_journal_cycles_do_not_retain_generated_holds() {
        let temp = tempfile::tempdir().unwrap();
        for (kind, nonce) in [("purge", "a".repeat(32)), ("restore", "b".repeat(32))] {
            let root = temp.path().join(kind);
            fs::create_dir(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let parent = secure_fs::Directory::open(root.parent().unwrap()).unwrap();
            if kind == "purge" {
                let journal = PurgeJournal {
                    version: 1,
                    root_path: root.to_string_lossy().into_owned(),
                    nonce: nonce.clone(),
                    staged_name: format!(".gogglelab-empty-{nonce}"),
                    previous_name: format!("purge.previous-purge-{nonce}"),
                    committed: false,
                };
                let (_, mut name) = write_purge_journal_at(&parent, &root, &journal).unwrap();
                let directory = secure_fs::Directory::open(&root).unwrap();
                write_purge_stage_marker_at(&directory, &root, &nonce).unwrap();
                mark_purge_journal_committed_at(&parent, &root, &mut name, &nonce).unwrap();
                remove_purge_stage_marker_at(&directory, &root, &nonce).unwrap();
                clear_purge_journal_at(&parent, &mut name).unwrap();
                assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
            } else {
                let journal = RestoreJournal {
                    version: 1,
                    root_path: root.to_string_lossy().into_owned(),
                    nonce: nonce.clone(),
                    staged_name: format!(".gogglelab-restore-{nonce}"),
                    previous_name: format!("restore.previous-{nonce}"),
                    failed_name: format!(".gogglelab-failed-restore-{nonce}"),
                    committed: false,
                };
                let mut name = write_restore_journal_at(&parent, &root, &journal).unwrap();
                mark_restore_journal_committed_at(&parent, &root, &mut name, &nonce).unwrap();
                clear_restore_journal_at(&parent, &mut name).unwrap();
            }
        }
        let mut entries = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(
            entries,
            vec![
                std::ffi::OsString::from("purge"),
                std::ffi::OsString::from("restore")
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn purge_commit_marker_failure_keeps_runtime_maintenance_armed() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let mut runtime_config = config(library.clone());
        runtime_config.local_catalog_root_access = false;
        let runtime = LibraryRuntime::open(runtime_config).unwrap();
        let nonce = "4".repeat(32);
        write_purge_stage_marker(&runtime.root, &nonce).unwrap();
        let parent_path = runtime.root.parent().unwrap();
        let previous_name = std::ffi::OsString::from(format!(
            "{}.previous-purge-{nonce}",
            runtime.root.file_name().unwrap().to_string_lossy()
        ));
        let previous_path = parent_path.join(&previous_name);
        fs::create_dir(&previous_path).unwrap();
        fs::write(previous_path.join("sentinel"), b"preserve previous").unwrap();
        let journal = PurgeJournal {
            version: 1,
            root_path: runtime.root.to_string_lossy().into_owned(),
            nonce: nonce.clone(),
            staged_name: format!(".gogglelab-empty-{nonce}"),
            previous_name: previous_name.to_string_lossy().into_owned(),
            committed: false,
        };
        let journal_path = write_purge_journal(&runtime.root, &journal).unwrap();
        let parent_directory = secure_fs::Directory::open(parent_path).unwrap();
        let mut journal_name = GeneratedTransactionJournal::recover(
            &GuardedDirectoryParent(parent_directory.try_clone().unwrap()),
            &runtime.root,
            GeneratedControlKind::Purge,
        )
        .unwrap()
        .unwrap();
        let staged_directory = secure_fs::Directory::open(&runtime.root).unwrap();
        let mut guard = runtime.begin_maintenance().unwrap();

        let error = finish_catalog_purge_commit(
            &mut guard,
            &staged_directory,
            &parent_directory,
            &runtime.root,
            &mut journal_name,
            &nonce,
            |_, _| {
                Err(RuntimeError {
                    code: RuntimeErrorCode::Corrupt,
                    message: "Injected purge stage-marker cleanup failure".into(),
                    retryable: false,
                    current_version: None,
                    source_holds: Vec::new(),
                })
            },
            |_, _| panic!("journal cleanup must not run after marker failure"),
        )
        .unwrap_err();

        assert_eq!(error.code, RuntimeErrorCode::Corrupt);
        drop(guard);
        assert!(runtime.maintenance.load(Ordering::Acquire));
        assert!(purge_stage_marker_matches(&runtime.root, &nonce).unwrap());
        let journal_after_failure: PurgeJournal =
            serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        assert!(journal_after_failure.committed);
        assert!(runtime.read_db(|db| db.list_models(None, None)).is_err());
        drop(runtime);

        let mut restarted_config = config(library.clone());
        restarted_config.local_catalog_root_access = false;
        let restarted = LibraryRuntime::open(restarted_config).unwrap();
        assert_eq!(
            restarted
                .read_db(|db| db.list_models(None, None))
                .unwrap()
                .len(),
            0
        );
        assert!(previous_path.is_dir());
        assert_eq!(
            fs::read(previous_path.join("sentinel")).unwrap(),
            b"preserve previous"
        );
        assert!(!purge_stage_marker_path(&library).exists());
        assert!(!journal_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn purge_commit_journal_failure_keeps_runtime_maintenance_armed() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let mut runtime_config = config(library.clone());
        runtime_config.local_catalog_root_access = false;
        let runtime = LibraryRuntime::open(runtime_config).unwrap();
        let nonce = "6".repeat(32);
        write_purge_stage_marker(&runtime.root, &nonce).unwrap();
        let parent_path = runtime.root.parent().unwrap();
        let previous_name = std::ffi::OsString::from(format!(
            "{}.previous-purge-{nonce}",
            runtime.root.file_name().unwrap().to_string_lossy()
        ));
        let previous_path = parent_path.join(&previous_name);
        fs::create_dir(&previous_path).unwrap();
        fs::write(previous_path.join("sentinel"), b"preserve previous").unwrap();
        let journal = PurgeJournal {
            version: 1,
            root_path: runtime.root.to_string_lossy().into_owned(),
            nonce: nonce.clone(),
            staged_name: format!(".gogglelab-empty-{nonce}"),
            previous_name: previous_name.to_string_lossy().into_owned(),
            committed: false,
        };
        let journal_path = write_purge_journal(&runtime.root, &journal).unwrap();
        let parent_directory = secure_fs::Directory::open(parent_path).unwrap();
        let mut journal_name = GeneratedTransactionJournal::recover(
            &GuardedDirectoryParent(parent_directory.try_clone().unwrap()),
            &runtime.root,
            GeneratedControlKind::Purge,
        )
        .unwrap()
        .unwrap();
        let staged_directory = secure_fs::Directory::open(&runtime.root).unwrap();
        let mut guard = runtime.begin_maintenance().unwrap();

        let error = finish_catalog_purge_commit(
            &mut guard,
            &staged_directory,
            &parent_directory,
            &runtime.root,
            &mut journal_name,
            &nonce,
            |directory, nonce| remove_purge_stage_marker_at(directory, &runtime.root, nonce),
            |_, _| Err(io_error()),
        )
        .unwrap_err();

        assert_eq!(error.code, RuntimeErrorCode::Io);
        drop(guard);
        assert!(runtime.maintenance.load(Ordering::Acquire));
        assert!(!purge_stage_marker_path(&runtime.root).exists());
        let journal_after_failure: PurgeJournal =
            serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        assert!(journal_after_failure.committed);
        assert!(runtime.read_db(|db| db.list_models(None, None)).is_err());
        drop(runtime);

        let mut restarted_config = config(library.clone());
        restarted_config.local_catalog_root_access = false;
        let restarted = LibraryRuntime::open(restarted_config).unwrap();
        assert_eq!(
            restarted
                .read_db(|db| db.list_models(None, None))
                .unwrap()
                .len(),
            0
        );
        assert!(previous_path.is_dir());
        assert_eq!(
            fs::read(previous_path.join("sentinel")).unwrap(),
            b"preserve previous"
        );
        assert!(!journal_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn restore_recovery_uses_pinned_parent_for_child_state_after_path_swap() {
        let temp = tempfile::tempdir().unwrap();
        let parent_entry = temp.path().join("restore-parent");
        fs::create_dir(&parent_entry).unwrap();
        let parent_path = parent_entry.canonicalize().unwrap();
        let library = parent_path.join("library");
        LibraryDb::open(&library)
            .unwrap()
            .create_model("Original sentinel", None)
            .unwrap();
        let canonical_root = library.canonicalize().unwrap();
        let nonce = "a".repeat(32);
        let staged_name = std::ffi::OsString::from(format!(".gogglelab-restore-{nonce}"));
        let previous_name = std::ffi::OsString::from(format!("library.previous-{nonce}"));
        let failed_name = std::ffi::OsString::from(format!(".gogglelab-failed-restore-{nonce}"));
        let staged = parent_path.join(&staged_name);
        fs::create_dir(&staged).unwrap();
        LibraryDb::open(&staged)
            .unwrap()
            .create_model("Staged sentinel", None)
            .unwrap();

        let parent_handle = secure_fs::Directory::open(&parent_path).unwrap();
        let journal = RestoreJournal {
            version: 1,
            root_path: canonical_root.to_str().unwrap().to_owned(),
            nonce: nonce.clone(),
            staged_name: staged_name.to_string_lossy().into_owned(),
            previous_name: previous_name.to_string_lossy().into_owned(),
            failed_name: failed_name.to_string_lossy().into_owned(),
            committed: false,
        };
        let journal_name =
            write_restore_journal_at(&parent_handle, &canonical_root, &journal).unwrap();
        let previous = parent_path.join(&previous_name);
        fs::rename(&canonical_root, &previous).unwrap();
        drop(parent_handle);

        let moved_parent = parent_path.with_file_name("restore-parent-pinned");
        let decoy_parent = parent_path.clone();
        let decoy_root = decoy_parent.join("library");
        let decoy_staged = decoy_parent.join(&staged_name);
        recover_interrupted_library_restore_with_hook(&canonical_root, || {
            fs::rename(&parent_path, &moved_parent).unwrap();
            fs::create_dir(&decoy_parent).unwrap();
            LibraryDb::open(&decoy_root)
                .unwrap()
                .create_model("Decoy sentinel", None)
                .unwrap();
            fs::create_dir(&decoy_staged).unwrap();
            fs::write(decoy_staged.join("sentinel"), b"decoy stage").unwrap();
        })
        .unwrap();

        let restored = moved_parent.join("library");
        assert!(
            restored.is_dir(),
            "recovery must restore the pinned Library"
        );
        let restored_db = LibraryDb::open(&restored).unwrap();
        let restored_models = restored_db.list_models(None, None).unwrap();
        assert_eq!(restored_models.len(), 1);
        assert_eq!(restored_models[0].display_name, "Original sentinel");
        assert!(!moved_parent.join(&previous_name).exists());
        assert!(!moved_parent.join(&staged_name).exists());
        assert!(!moved_parent.join(&journal_name).exists());

        let decoy_db = LibraryDb::open(&decoy_root).unwrap();
        assert_eq!(
            decoy_db.list_models(None, None).unwrap()[0].display_name,
            "Decoy sentinel"
        );
        assert_eq!(
            fs::read(decoy_staged.join("sentinel")).unwrap(),
            b"decoy stage"
        );
    }

    #[cfg(unix)]
    #[test]
    fn restore_recovery_rolls_back_precommit_and_preserves_committed_root() {
        fn setup_library(path: &Path, model_name: &str) {
            let db = LibraryDb::open(path).unwrap();
            db.create_model(model_name, None).unwrap();
        }

        fn journal_for(root: &Path, nonce: &str) -> RestoreJournal {
            RestoreJournal {
                version: 1,
                root_path: root.to_string_lossy().into_owned(),
                nonce: nonce.into(),
                staged_name: format!(".gogglelab-restore-{nonce}"),
                previous_name: format!(
                    "{}.previous-{nonce}",
                    root.file_name().unwrap().to_string_lossy()
                ),
                failed_name: format!(".gogglelab-failed-restore-{nonce}"),
                committed: false,
            }
        }

        let temp = tempfile::tempdir().unwrap();

        // Crash after root -> previous but before staged -> root.
        let first_root = temp.path().join("first-library");
        setup_library(&first_root, "Original before install");
        let first_root = first_root.canonicalize().unwrap();
        let first_parent_path = first_root.parent().unwrap();
        let first_journal = journal_for(&first_root, &"a".repeat(32));
        let first_parent = secure_fs::Directory::open(first_parent_path).unwrap();
        let first_journal_name =
            write_restore_journal_at(&first_parent, &first_root, &first_journal).unwrap();
        let first_previous = first_parent_path.join(&first_journal.previous_name);
        let first_staged = first_parent_path.join(&first_journal.staged_name);
        fs::rename(&first_root, &first_previous).unwrap();
        setup_library(&first_staged, "Uncommitted staged library");

        recover_interrupted_library_restore(&first_root).unwrap();

        let first_db = LibraryDb::open(&first_root).unwrap();
        assert_eq!(
            first_db.list_models(None, None).unwrap()[0].display_name,
            "Original before install"
        );
        assert!(!first_previous.exists());
        assert!(!first_staged.exists());
        assert!(!first_parent_path
            .join(
                restore_journal_path(&first_root)
                    .unwrap()
                    .file_name()
                    .unwrap()
            )
            .exists());
        drop(first_journal_name);

        // Crash after staged -> root but before the committed state is durable.
        let second_root = temp.path().join("second-library");
        setup_library(&second_root, "Original before activation");
        let second_root = second_root.canonicalize().unwrap();
        let second_parent_path = second_root.parent().unwrap();
        let second_journal = journal_for(&second_root, &"b".repeat(32));
        let second_parent = secure_fs::Directory::open(second_parent_path).unwrap();
        let second_journal_name =
            write_restore_journal_at(&second_parent, &second_root, &second_journal).unwrap();
        let second_previous = second_parent_path.join(&second_journal.previous_name);
        let second_staged = second_parent_path.join(&second_journal.staged_name);
        fs::rename(&second_root, &second_previous).unwrap();
        setup_library(&second_staged, "Uncommitted activated library");
        fs::rename(&second_staged, &second_root).unwrap();

        recover_interrupted_library_restore(&second_root).unwrap();

        let second_db = LibraryDb::open(&second_root).unwrap();
        assert_eq!(
            second_db.list_models(None, None).unwrap()[0].display_name,
            "Original before activation"
        );
        let failed = second_parent_path.join(&second_journal.failed_name);
        assert!(LibraryDb::open(&failed).is_ok());
        assert!(!second_previous.exists());
        assert!(!second_parent_path.join(&second_journal_name).exists());

        // After the durable commit bit, startup must retain the new root.
        let third_root = temp.path().join("third-library");
        setup_library(&third_root, "Original retained as previous");
        let third_root = third_root.canonicalize().unwrap();
        let third_parent_path = third_root.parent().unwrap();
        let third_journal = journal_for(&third_root, &"c".repeat(32));
        let third_parent = secure_fs::Directory::open(third_parent_path).unwrap();
        let mut third_journal_name =
            write_restore_journal_at(&third_parent, &third_root, &third_journal).unwrap();
        let third_previous = third_parent_path.join(&third_journal.previous_name);
        let third_staged = third_parent_path.join(&third_journal.staged_name);
        fs::rename(&third_root, &third_previous).unwrap();
        setup_library(&third_staged, "Committed restored library");
        fs::rename(&third_staged, &third_root).unwrap();
        mark_restore_journal_committed_at(
            &third_parent,
            &third_root,
            &mut third_journal_name,
            &third_journal.nonce,
        )
        .unwrap();

        recover_interrupted_library_restore(&third_root).unwrap();

        let third_db = LibraryDb::open(&third_root).unwrap();
        assert_eq!(
            third_db.list_models(None, None).unwrap()[0].display_name,
            "Committed restored library"
        );
        let previous_db = LibraryDb::open(&third_previous).unwrap();
        assert_eq!(
            previous_db.list_models(None, None).unwrap()[0].display_name,
            "Original retained as previous"
        );
        assert!(!third_parent_path.join(&third_journal_name).exists());
    }

    #[test]
    fn purge_catalog_backs_up_before_swapping_and_preserves_external_sources_and_roots() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let external_models = temp.path().join("external-models");
        let backups = temp.path().join("backups");
        fs::create_dir(&external_models).unwrap();
        fs::create_dir(&backups).unwrap();
        let external_source = external_models.join("indexed.stl");
        fs::write(&external_source, triangle_stl(2.0)).unwrap();
        let external_bytes = fs::read(&external_source).unwrap();
        let managed_input = temp.path().join("managed.stl");
        fs::write(&managed_input, triangle_stl(0.0)).unwrap();
        let managed = {
            let db = LibraryDb::open(&library).unwrap();
            let root = db
                .add_index_root(&external_models, 60, 50 * 1024 * 1024)
                .unwrap();
            let model = db
                .import_file(&managed_input, None, Some("Managed source"))
                .unwrap()
                .model;
            (root, model)
        };
        let mut runtime_config = config(library.clone());
        runtime_config.local_catalog_root_access = false;
        runtime_config.roots = vec![
            RuntimeRootPolicy {
                id: "models".into(),
                catalog_root_id: Some(managed.0.id.clone()),
                label: "Models".into(),
                path: external_models.clone(),
                access: RootAccess::ReadOnly,
            },
            RuntimeRootPolicy {
                id: "backups".into(),
                catalog_root_id: None,
                label: "Backups".into(),
                path: backups.clone(),
                access: RootAccess::BackupOnly,
            },
        ];
        runtime_config.capabilities = BTreeSet::from([
            Capability::CatalogRead,
            Capability::CatalogWrite,
            Capability::Maintenance,
            Capability::BackupWrite,
            Capability::Scan,
        ]);
        let runtime = LibraryRuntime::open(runtime_config).unwrap();
        scanner::scan_root(&runtime, &managed.0).unwrap();
        assert_eq!(
            runtime
                .read_db(|db| db.list_models(None, None))
                .unwrap()
                .len(),
            2
        );
        let context = RequestContext {
            principal_id: "purge-operator".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::CatalogWrite,
                Capability::Maintenance,
                Capability::BackupWrite,
                Capability::Scan,
            ]),
            request_id: "purge-catalog".into(),
        };
        let managed_asset = runtime
            .read_db(|db| db.resolve_model_source_ref(&managed.1.id))
            .unwrap()
            .outer_path;
        let managed_bytes = fs::read(&managed_asset).unwrap();
        let changed_plan = runtime
            .plan_action(
                &context,
                ManagementAction::PurgeCatalog {
                    backup_destination_handle_id: "backups".into(),
                    backup_file_name: "changed-after-plan.zip".into(),
                },
            )
            .unwrap();
        fs::write(&managed_asset, b"modified after purge confirmation").unwrap();
        assert_eq!(
            runtime
                .execute_action(&context, &changed_plan.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
        assert!(!backups.join("changed-after-plan.zip").exists());
        assert_eq!(
            runtime
                .read_db(|db| db.list_models(None, None))
                .unwrap()
                .len(),
            2
        );
        fs::write(&managed_asset, &managed_bytes).unwrap();

        fs::write(backups.join("occupied.zip"), b"preserve this file").unwrap();
        assert!(runtime
            .plan_action(
                &context,
                ManagementAction::PurgeCatalog {
                    backup_destination_handle_id: "backups".into(),
                    backup_file_name: "occupied.zip".into(),
                },
            )
            .is_err());
        assert_eq!(
            fs::read(backups.join("occupied.zip")).unwrap(),
            b"preserve this file"
        );

        let plan = runtime
            .plan_action(
                &context,
                ManagementAction::PurgeCatalog {
                    backup_destination_handle_id: "backups".into(),
                    backup_file_name: "before-purge.zip".into(),
                },
            )
            .unwrap();
        assert_eq!(plan.action_kind, "purgeCatalog");
        assert!(plan.impact.iter().any(|item| item.contains("backup")));
        let backup_destination = backups.join("before-purge.zip");
        assert!(matches!(
            runtime.execute_action(&context, &plan.token).unwrap(),
            ManagementResult::Purged {
                backup_destination_handle_id,
                backup_file_name,
                ..
            } if backup_destination_handle_id == "backups" && backup_file_name == "before-purge.zip"
        ));
        assert!(backup_destination.is_file());
        let current_models = runtime.read_db(|db| db.list_models(None, None)).unwrap();
        assert!(current_models.is_empty());
        let roots_after = runtime.read_db(|db| db.list_index_roots()).unwrap();
        assert_eq!(roots_after.len(), 1);
        assert_eq!(roots_after[0].id, managed.0.id);
        assert_eq!(fs::read(&external_source).unwrap(), external_bytes);

        let restored = temp.path().join("verified-backup");
        library_backup::restore_backup_to_staging(&backup_destination, &restored).unwrap();
        let restored_db = LibraryDb::open(&restored).unwrap();
        assert_eq!(restored_db.list_models(None, None).unwrap().len(), 2);
        assert!(restored_db
            .list_index_roots()
            .unwrap()
            .iter()
            .any(|root| root.id == managed.0.id));
        let previous = fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("library.previous-purge-"))
            })
            .unwrap();
        assert!(previous.join("originals").is_dir());
        assert!(managed.1.content_hash.is_some());
    }

    #[test]
    fn thumbnail_cleanup_preserves_unproven_png_and_archive_source_copies() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let mut cfg = config(library.clone());
        cfg.local_catalog_root_access = false;
        cfg.capabilities
            .extend([Capability::MediaWrite, Capability::Maintenance]);
        let runtime = LibraryRuntime::open(cfg).unwrap();
        let png = library
            .join("thumbnails")
            .join(format!("{}-isometric-v1-256-ffffff.png", "a".repeat(64)));
        fs::create_dir_all(png.parent().unwrap()).unwrap();
        fs::write(&png, b"foreign PNG-looking bytes").unwrap();
        let original = b"extracted original model bytes";
        let source = library
            .join("indexed-cache")
            .join(format!("{:x}", Sha256::digest(original)))
            .join("original.stl");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, original).unwrap();
        let ctx = RequestContext {
            principal_id: "cleanup-proof".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::Maintenance,
                Capability::MediaWrite,
            ]),
            request_id: "proof".into(),
        };
        let plan = runtime
            .plan_action(
                &ctx,
                ManagementAction::CleanupOrphans { continuation: None },
            )
            .unwrap();
        let result = runtime.execute_action(&ctx, &plan.token).unwrap();
        assert!(matches!(
            result,
            ManagementResult::CleanupCompleted { files: 0, .. }
        ));
        assert_eq!(fs::read(png).unwrap(), b"foreign PNG-looking bytes");
        assert_eq!(fs::read(source).unwrap(), original);
    }

    #[test]
    fn cleanup_orphans_preserves_unproven_assets_without_recoverable_holds() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let mut runtime_config = config(library.clone());
        runtime_config.local_catalog_root_access = false;
        runtime_config
            .capabilities
            .extend([Capability::MediaWrite, Capability::Maintenance]);
        let runtime = LibraryRuntime::open(runtime_config).unwrap();
        let thumbnail_dir = library.join("thumbnails");
        fs::create_dir_all(&thumbnail_dir).unwrap();
        let thumbnail =
            thumbnail_dir.join(format!("{}-isometric-v1-256-ffffff.png", "a".repeat(64)));
        let note = thumbnail_dir.join("operator-note.png");
        let cache_bytes = b"orphan extracted archive member";
        let cache_hash = format!("{:x}", Sha256::digest(cache_bytes));
        let cache_dir = library.join("indexed-cache").join(&cache_hash);
        fs::create_dir_all(&cache_dir).unwrap();
        let cache = cache_dir.join("part.stl");
        fs::write(&thumbnail, b"generated thumbnail orphan").unwrap();
        fs::write(&cache, cache_bytes).unwrap();
        fs::write(&note, b"operator file").unwrap();
        let context = RequestContext {
            principal_id: "cleanup-operator".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::Maintenance,
                Capability::MediaWrite,
            ]),
            request_id: "cleanup-orphans".into(),
        };
        let plan = runtime
            .plan_action(
                &context,
                ManagementAction::CleanupOrphans { continuation: None },
            )
            .unwrap();
        assert_eq!(plan.action_kind, "cleanupOrphans");
        assert_eq!(plan.thumbnail_review.as_ref().unwrap().eligible_files, 0);
        assert_eq!(
            plan.thumbnail_review
                .as_ref()
                .unwrap()
                .protected_source_copies,
            1
        );
        let result = runtime.execute_action(&context, &plan.token).unwrap();
        assert!(matches!(
            result,
            ManagementResult::CleanupCompleted {
                files: 0,
                bytes: 0,
                ..
            }
        ));
        assert!(thumbnail.exists());
        assert!(cache.exists());
        assert!(cache_dir.exists());
        let remaining_thumbnail_files = fs::read_dir(&thumbnail_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(remaining_thumbnail_files.len(), 2);
        assert_eq!(fs::read(&note).unwrap(), b"operator file");
        assert_eq!(
            runtime
                .execute_action(&context, &plan.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
    }

    #[test]
    fn cleanup_orphans_preserves_changed_unproven_file_without_deleting_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let mut runtime_config = config(library.clone());
        runtime_config.local_catalog_root_access = false;
        runtime_config
            .capabilities
            .extend([Capability::MediaWrite, Capability::Maintenance]);
        let runtime = LibraryRuntime::open(runtime_config).unwrap();
        let thumbnails = library.join("thumbnails");
        fs::create_dir_all(&thumbnails).unwrap();
        let orphan = thumbnails.join(format!("{}-isometric-v1-256-ffffff.png", "b".repeat(64)));
        fs::write(&orphan, b"original generated bytes").unwrap();
        let context = RequestContext {
            principal_id: "cleanup-hash-operator".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::Maintenance,
                Capability::MediaWrite,
            ]),
            request_id: "cleanup-hash".into(),
        };
        let plan = runtime
            .plan_action(
                &context,
                ManagementAction::CleanupOrphans { continuation: None },
            )
            .unwrap();
        fs::write(&orphan, b"changed generated bytes").unwrap();
        assert!(matches!(
            runtime.execute_action(&context, &plan.token).unwrap(),
            ManagementResult::CleanupCompleted { files: 0, .. }
        ));
        assert_eq!(fs::read(&orphan).unwrap(), b"changed generated bytes");
    }

    #[test]
    fn backup_only_root_supports_backup_and_export_without_source_write_capability() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let backups = temp.path().join("backups");
        fs::create_dir(&backups).unwrap();
        let mut runtime_config = config(library);
        runtime_config.local_catalog_root_access = false;
        runtime_config.capabilities = BTreeSet::from([
            Capability::CatalogRead,
            Capability::Maintenance,
            serde_json::from_value(serde_json::json!("backupWrite")).unwrap(),
        ]);
        runtime_config.roots.push(RuntimeRootPolicy {
            id: "backups".into(),
            catalog_root_id: None,
            label: "Backups".into(),
            path: backups.clone(),
            access: serde_json::from_value(serde_json::json!("backupOnly")).unwrap(),
        });
        let runtime = LibraryRuntime::open(runtime_config).unwrap();
        let model = runtime
            .mutate_db(|db| db.create_model("Backup-only model", None))
            .unwrap();
        let context = RequestContext {
            principal_id: "backup-only".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::Maintenance,
                serde_json::from_value(serde_json::json!("backupWrite")).unwrap(),
            ]),
            request_id: "backup-only-plan".into(),
        };

        let backup = runtime
            .plan_action(
                &context,
                ManagementAction::CreateBackup {
                    destination_handle_id: "backups".into(),
                    file_name: "catalog.zip".into(),
                },
            )
            .unwrap();
        assert!(matches!(
            runtime.execute_action(&context, &backup.token).unwrap(),
            ManagementResult::BackupCreated { .. }
        ));
        assert!(backups.join("catalog.zip").is_file());

        let export = runtime
            .plan_action(
                &context,
                ManagementAction::ExportSelection {
                    targets: vec![VersionedTarget {
                        model_id: model.id,
                        expected_version: model.metadata_version,
                    }],
                    destination_handle_id: "backups".into(),
                    file_name: "selection.zip".into(),
                },
            )
            .unwrap();
        assert!(matches!(
            runtime.execute_action(&context, &export.token).unwrap(),
            ManagementResult::SelectionExported { models: 1, .. }
        ));
        assert!(backups.join("selection.zip").is_file());
        assert!(!context
            .capabilities
            .contains(&Capability::PhysicalSourceWrite));
        assert!(runtime
            .admitted_root_path(&context, "backups", RootAccess::Writable)
            .is_err());
    }

    #[test]
    fn filament_assignment_uses_one_atomic_per_target_transaction() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let first = runtime.mutate_db(|db| db.create_model("A", None)).unwrap();
        let second = runtime.mutate_db(|db| db.create_model("B", None)).unwrap();
        let context = RequestContext {
            principal_id: "editor".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
            request_id: "filaments".into(),
        };
        let filament = runtime
            .create_filament(
                &context,
                FilamentInput {
                    brand: "Brand".into(),
                    material: "PLA".into(),
                    color: "#abcdef".into(),
                    name: "Blue".into(),
                    notes: String::new(),
                },
            )
            .unwrap();
        let assignments = vec![
            ModelFilamentAssignment {
                target: VersionedTarget {
                    model_id: first.id.clone(),
                    expected_version: first.metadata_version,
                },
                filament_ids: vec![filament.id.clone()],
            },
            ModelFilamentAssignment {
                target: VersionedTarget {
                    model_id: second.id.clone(),
                    expected_version: second.metadata_version + 1,
                },
                filament_ids: vec![filament.id.clone()],
            },
        ];
        assert_eq!(
            runtime
                .bulk_assign_model_filaments(&context, assignments)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Conflict
        );
        for id in [&first.id, &second.id] {
            assert!(runtime
                .read_db(|db| db.get_model(id))
                .unwrap()
                .unwrap()
                .filament_ids
                .is_empty());
        }
    }

    #[test]
    fn page_capture_is_idempotent_and_creates_no_fake_source_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let context = RequestContext {
            principal_id: "capture".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
            request_id: "capture-request".into(),
        };
        let envelope = CaptureEnvelope {
            version: CAPTURE_ENVELOPE_VERSION,
            item_id: "item-1".into(),
            created_at: "2026-10-11T00:00:00Z".into(),
            page: CapturePage {
                url: "https://example.test/model".into(),
                title: "Captured model".into(),
                designer: None,
                license: None,
                notes: None,
                parent_external_id: None,
            },
            downloads: vec![],
        };
        let first = runtime
            .import_capture(&context, envelope.clone(), vec![])
            .unwrap();
        assert_eq!(first.status, CaptureItemStatus::Imported);
        let repeated = runtime
            .import_capture(&context, envelope.clone(), vec![])
            .unwrap();
        assert_eq!(repeated.status, CaptureItemStatus::AlreadyImported);
        let mut second_envelope = envelope.clone();
        second_envelope.item_id = "item-2".into();
        second_envelope.created_at = "2026-10-11T00:00:01Z".into();
        runtime
            .import_capture(&context, second_envelope, vec![])
            .unwrap();
        let first_page = runtime
            .list_capture_items(
                &context,
                PageRequest {
                    cursor: None,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(first_page.items[0].item_id, "item-2");
        let second_page = runtime
            .list_capture_items(
                &context,
                PageRequest {
                    cursor: first_page.next_cursor,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(second_page.items[0].item_id, "item-1");
        let model = runtime
            .read_db(|db| db.get_model(first.page_model_id.as_ref().unwrap()))
            .unwrap()
            .unwrap();
        assert_eq!(model.catalog_kind, "bookmark");
        assert!(model.content_hash.is_none());
        let mut conflict = envelope;
        conflict.created_at = "2026-10-11T00:00:01Z".into();
        assert_eq!(
            runtime
                .import_capture(&context, conflict, vec![])
                .unwrap_err()
                .code,
            RuntimeErrorCode::InvalidInput
        );
    }

    #[test]
    fn shared_process_state_is_single_install_and_same_arc_for_embedded_consumers() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let state = Arc::new(Mutex::new(7_u32));
        runtime.install_shared_state(Arc::clone(&state)).unwrap();
        let loaded = runtime.shared_state::<Mutex<u32>>().unwrap();
        assert!(Arc::ptr_eq(&state, &loaded));
        assert_eq!(
            runtime
                .install_shared_state(Arc::new(Mutex::new(9_u32)))
                .unwrap_err()
                .code,
            RuntimeErrorCode::Conflict
        );
        let retained = runtime.shared_state::<Mutex<u32>>().unwrap();
        assert!(Arc::ptr_eq(&state, &retained));
        assert_eq!(*retained.lock().unwrap(), 7);
    }

    #[test]
    fn gallery_uses_stable_ids_active_selection_and_guarded_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.create_model("Gallery", None))
            .unwrap();
        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[10, 20, 30, 255])
                .unwrap();
        }
        let hash = format!("{:x}", Sha256::digest(&png_bytes));
        let first_path = PathBuf::from("thumbnails/runtime-first.png");
        let second_path = PathBuf::from("thumbnails/runtime-second.png");
        fs::write(runtime.root.join(&first_path), &png_bytes).unwrap();
        fs::write(runtime.root.join(&second_path), &png_bytes).unwrap();
        let (model, first) = runtime
            .mutate_db(|db| {
                db.add_custom_thumbnail(&model.id, model.metadata_version, &hash, &first_path)
            })
            .unwrap();
        let (model, second) = runtime
            .mutate_db(|db| {
                db.add_custom_thumbnail(&model.id, model.metadata_version, &hash, &second_path)
            })
            .unwrap();
        let context = RequestContext {
            principal_id: "gallery".into(),
            capabilities: BTreeSet::from([Capability::MediaRead, Capability::MediaWrite]),
            request_id: "gallery-request".into(),
        };
        let gallery = runtime.list_thumbnails(&context, &model.id).unwrap();
        assert_eq!(gallery.len(), 2);
        assert_eq!(gallery.iter().filter(|item| item.active).count(), 1);
        let media = runtime
            .issue_thumbnail_media(&context, &model.id, &second.id)
            .unwrap();
        let mut streamed = Vec::new();
        runtime
            .stream_content(&context, &media.id, &mut streamed)
            .unwrap();
        assert_eq!(streamed, png_bytes);
        let upload = runtime
            .stage_upload(
                &context,
                UploadIntent {
                    item_id: "gallery-upload".into(),
                    capture_id: "gallery-image".into(),
                    file_name: "preview.png".into(),
                    bytes: png_bytes.len() as u64,
                    sha256: hash.clone(),
                },
                &mut png_bytes.as_slice(),
            )
            .unwrap();
        let version = runtime
            .add_thumbnail(&context, &model.id, model.metadata_version, &upload.id)
            .unwrap();
        assert_eq!(
            runtime.list_thumbnails(&context, &model.id).unwrap().len(),
            3
        );
        let version = runtime
            .set_active_thumbnail(&context, &model.id, version, &first.id)
            .unwrap();
        assert!(runtime
            .delete_thumbnail(&context, &model.id, version, &first.id)
            .is_err());
        let version = runtime
            .delete_thumbnail(&context, &model.id, version, &second.id)
            .unwrap();
        assert!(version > model.metadata_version);
        assert!(!runtime.root.join(second_path).exists());
    }

    #[test]
    fn source_lease_streams_verified_bytes_and_epoch_invalidation_revokes_it() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        fs::write(&source, &bytes).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| {
                db.import_file(&source, None, None)
                    .map(|result| result.model)
            })
            .unwrap();
        let context = RequestContext {
            principal_id: "download".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "download-request".into(),
        };
        let lease = runtime
            .issue_model_download(&context, &model.id, model.content_hash.as_deref())
            .unwrap();
        assert_eq!(lease.bytes, bytes.len() as u64);
        assert!(!lease.file_name.contains('/'));
        assert_eq!(
            runtime.describe_content(&context, &lease.id).unwrap(),
            lease
        );
        let mut streamed = Vec::new();
        runtime
            .stream_content(&context, &lease.id, &mut streamed)
            .unwrap();
        assert_eq!(streamed, bytes);
        let other = RequestContext {
            principal_id: "other-principal".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "other-download".into(),
        };
        assert_eq!(
            runtime
                .describe_content(&other, &lease.id)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert_eq!(
            runtime
                .stream_content(&other, &lease.id, &mut Vec::new())
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        let maintenance = runtime.begin_maintenance().unwrap();
        maintenance.invalidate();
        drop(maintenance);
        assert_eq!(
            runtime
                .describe_content(&context, &lease.id)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
        assert_eq!(
            runtime
                .stream_content(&context, &lease.id, &mut Vec::new())
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
    }

    #[test]
    fn indexed_download_requires_operator_admitted_catalog_root() {
        use library_core::{ScannedModel, SourceRef};

        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let sources = temp.path().join("sources");
        fs::create_dir(&sources).unwrap();
        let path = sources.join("part.stl");
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        fs::write(&path, &bytes).unwrap();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let scanned = ScannedModel {
            relative_path: "part.stl".into(),
            content_hash: hash.clone(),
            bytes: bytes.len() as u64,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        let added = db
            .record_scan_entries(&root.id, std::slice::from_ref(&scanned))
            .unwrap();
        db.complete_scan_sources_with_new_models(
            &root.id,
            &[SourceRef {
                relative_path: "part.stl".into(),
                archive_member: None,
            }],
            &added.added_model_ids,
            "1",
        )
        .unwrap();
        let model = db.list_models(None, None).unwrap().pop().unwrap();
        drop(db);
        let context = RequestContext {
            principal_id: "remote".into(),
            capabilities: BTreeSet::from([Capability::MediaRead, Capability::HostPaths]),
            request_id: "remote-download".into(),
        };
        let mut headless = config(library.clone());
        headless.local_catalog_root_access = false;
        let runtime = LibraryRuntime::open(headless).unwrap();
        let bundle_id = runtime
            .mutate_db(|db| db.create_bundle(std::slice::from_ref(&model.id)))
            .unwrap();
        assert_eq!(
            runtime
                .issue_model_download(&context, &model.id, Some(&hash))
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert_eq!(
            runtime
                .prepare_bundle_preview(&context, &bundle_id)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert_eq!(
            runtime
                .issue_model_geometry(&context, &model.id, Some(&hash), None)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        drop(runtime);
        let desktop = LibraryRuntime::open(config(library.clone())).unwrap();
        assert!(desktop
            .issue_model_download(&context, &model.id, Some(&hash))
            .is_ok());
        drop(desktop);
        let mut admitted = config(library);
        admitted.local_catalog_root_access = false;
        admitted.roots.push(RuntimeRootPolicy {
            id: "operator-models".into(),
            catalog_root_id: Some(root.id),
            label: "Models".into(),
            path: sources,
            access: RootAccess::ReadOnly,
        });
        let runtime = LibraryRuntime::open(admitted).unwrap();
        assert!(runtime
            .issue_model_download(&context, &model.id, Some(&hash))
            .is_ok());
    }

    #[test]
    fn staged_capture_upload_imports_verified_model_and_consumes_token() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let context = RequestContext {
            principal_id: "capture-upload".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
            request_id: "capture-upload-request".into(),
        };
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let intent = UploadIntent {
            item_id: "upload-item".into(),
            capture_id: "capture-1".into(),
            file_name: "part.stl".into(),
            bytes: bytes.len() as u64,
            sha256: hash.clone(),
        };
        let token = runtime
            .stage_upload(&context, intent, &mut bytes.as_slice())
            .unwrap();
        let envelope = CaptureEnvelope {
            version: CAPTURE_ENVELOPE_VERSION,
            item_id: "upload-item".into(),
            created_at: "2026-10-11T02:00:00Z".into(),
            page: CapturePage {
                url: "https://example.test/upload".into(),
                title: "Uploaded model".into(),
                designer: Some("Designer".into()),
                license: Some("CC0".into()),
                notes: None,
                parent_external_id: None,
            },
            downloads: vec![CaptureDownload {
                capture_id: "capture-1".into(),
                file_name: "part.stl".into(),
                relative_inbox_path: None,
                sha256: Some(hash.clone()),
                bytes: Some(bytes.len() as u64),
            }],
        };
        let result = runtime
            .import_capture(&context, envelope.clone(), vec![token.clone()])
            .unwrap();
        assert_eq!(result.status, CaptureItemStatus::Imported);
        let model_id = result.downloads[0].model_id.as_ref().unwrap();
        let model = runtime
            .read_db(|db| db.get_model(model_id))
            .unwrap()
            .unwrap();
        assert_eq!(model.content_hash.as_deref(), Some(hash.as_str()));
        assert_eq!(model.metadata.parent_model_id, result.page_model_id);
        assert!(!runtime.uploads.lock().unwrap().contains_key(&token.id));
        assert_eq!(
            runtime
                .import_capture(&context, envelope, vec![])
                .unwrap()
                .status,
            CaptureItemStatus::AlreadyImported
        );
    }

    #[test]
    fn upload_requires_runtime_and_context_write_capability_intersection() {
        let temp = tempfile::tempdir().unwrap();
        let readonly_root = temp.path().join("readonly-library");
        let mut readonly_config = config(readonly_root.clone());
        readonly_config.capabilities = BTreeSet::from([Capability::CatalogRead]);
        let readonly = LibraryRuntime::open(readonly_config).unwrap();
        let context = RequestContext {
            principal_id: "writer".into(),
            capabilities: BTreeSet::from([Capability::CatalogWrite, Capability::MediaWrite]),
            request_id: "forged-write".into(),
        };
        let bytes = b"verified upload".to_vec();
        let intent = UploadIntent {
            item_id: "item".into(),
            capture_id: "capture".into(),
            file_name: "preview.png".into(),
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        assert_eq!(
            readonly
                .stage_upload(&context, intent.clone(), &mut bytes.as_slice())
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert!(!readonly_root.join("staging").exists());
        drop(readonly);

        let authorized =
            LibraryRuntime::open(config(temp.path().join("writable-library"))).unwrap();
        authorized.set_temp_limits_for_test(256 * 1024 * 1024, bytes.len() as u64 + 1);
        let token = authorized
            .stage_upload(&context, intent, &mut bytes.as_slice())
            .unwrap();
        let staged_path = authorized
            .uploads
            .lock()
            .unwrap()
            .get(&token.id)
            .unwrap()
            .path
            .clone();
        assert_eq!(fs::read(&staged_path).unwrap(), bytes);
        let second_intent = UploadIntent {
            item_id: "second".into(),
            capture_id: "second".into(),
            file_name: "second.png".into(),
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        assert_eq!(
            authorized
                .stage_upload(&context, second_intent, &mut bytes.as_slice())
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        assert_eq!(
            fs::read_dir(authorized.root.join("staging"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("upload-"))
                .count(),
            1
        );
        authorized.discard_upload(&context, &token.id).unwrap();
        assert!(!staged_path.exists());
        assert_eq!(
            authorized
                .temp_resources
                .lock()
                .unwrap()
                .staged_upload_bytes,
            0
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn desktop_configuration_removes_physical_source_writes_without_handle_relative_support() {
        let config = RuntimeConfig::desktop(
            PathBuf::from("C:/synthetic/library"),
            PathBuf::from("C:/synthetic/settings.json"),
        );
        assert!(!config
            .capabilities
            .contains(&Capability::PhysicalSourceWrite));
        assert_eq!(
            require_durable_source_mutations().unwrap_err().code,
            RuntimeErrorCode::Unsupported
        );
    }
}

fn fleet_printersourcekind(value: library_core::models::PrinterSourceKind) -> PrinterSourceKind {
    match value {
        library_core::models::PrinterSourceKind::Manual => PrinterSourceKind::Manual,
        library_core::models::PrinterSourceKind::Discovered => PrinterSourceKind::Discovered,
        library_core::models::PrinterSourceKind::Imported => PrinterSourceKind::Imported,
    }
}

fn fleet_printerconnectionstatus(
    value: library_core::models::PrinterConnectionStatus,
) -> PrinterConnectionStatus {
    match value {
        library_core::models::PrinterConnectionStatus::Unknown => PrinterConnectionStatus::Unknown,
        library_core::models::PrinterConnectionStatus::Online => PrinterConnectionStatus::Online,
        library_core::models::PrinterConnectionStatus::Offline => PrinterConnectionStatus::Offline,
        library_core::models::PrinterConnectionStatus::AuthRequired => {
            PrinterConnectionStatus::AuthRequired
        }
        library_core::models::PrinterConnectionStatus::Error => PrinterConnectionStatus::Error,
    }
}

fn fleet_printerjobstatus(value: library_core::models::PrinterJobStatus) -> PrinterJobStatus {
    match value {
        library_core::models::PrinterJobStatus::Unknown => PrinterJobStatus::Unknown,
        library_core::models::PrinterJobStatus::Idle => PrinterJobStatus::Idle,
        library_core::models::PrinterJobStatus::Printing => PrinterJobStatus::Printing,
        library_core::models::PrinterJobStatus::Paused => PrinterJobStatus::Paused,
        library_core::models::PrinterJobStatus::Complete => PrinterJobStatus::Complete,
    }
}

fn fleet_printer(value: library_core::models::Printer) -> Printer {
    Printer {
        id: value.id,
        metadata_version: value.metadata_version,
        nickname: value.nickname,
        make: value.make,
        model: value.model,
        printer_type: value.printer_type,
        firmware: value.firmware,
        host: value.host,
        web_url: value.web_url,
        endpoint_approved: value.endpoint_approved,
        bed_mm: value.bed_mm,
        notes: value.notes,
        source_kind: fleet_printersourcekind(value.source_kind),
        connection_status: fleet_printerconnectionstatus(value.connection_status),
        job_status: fleet_printerjobstatus(value.job_status),
        last_checked_at: value.last_checked_at,
        connection_error_code: value.connection_error_code,
        print_count: value.print_count,
        last_printed_at: value.last_printed_at,
        created_at: value.created_at,
        updated_at: value.updated_at,
    }
}

fn fleet_core_printerinput(value: PrinterInput) -> library_core::models::PrinterInput {
    library_core::models::PrinterInput {
        nickname: value.nickname,
        make: value.make,
        model: value.model,
        printer_type: value.printer_type,
        firmware: value.firmware,
        host: value.host,
        web_url: value.web_url,
        bed_mm: value.bed_mm,
        notes: value.notes,
    }
}

fn fleet_core_printermaintenanceinput(
    value: PrinterMaintenanceInput,
) -> library_core::models::PrinterMaintenanceInput {
    library_core::models::PrinterMaintenanceInput {
        title: value.title,
        performed_at: value.performed_at,
        notes: value.notes,
    }
}

fn fleet_printermaintenancelog(
    value: library_core::models::PrinterMaintenanceLog,
) -> PrinterMaintenanceLog {
    PrinterMaintenanceLog {
        id: value.id,
        metadata_version: value.metadata_version,
        printer_id: value.printer_id,
        printer_nickname: value.printer_nickname,
        title: value.title,
        performed_at: value.performed_at,
        notes: value.notes,
        created_at: value.created_at,
    }
}

fn fleet_printerreminderstatus(
    value: library_core::models::PrinterReminderStatus,
) -> PrinterReminderStatus {
    match value {
        library_core::models::PrinterReminderStatus::Active => PrinterReminderStatus::Active,
        library_core::models::PrinterReminderStatus::Paused => PrinterReminderStatus::Paused,
        library_core::models::PrinterReminderStatus::Completed => PrinterReminderStatus::Completed,
    }
}

fn fleet_core_printerreminderinput(
    value: PrinterReminderInput,
) -> library_core::models::PrinterReminderInput {
    library_core::models::PrinterReminderInput {
        title: value.title,
        notes: value.notes,
        due_at: value.due_at,
        interval_days: value.interval_days,
    }
}

fn fleet_printerreminder(value: library_core::models::PrinterReminder) -> PrinterReminder {
    PrinterReminder {
        id: value.id,
        metadata_version: value.metadata_version,
        printer_id: value.printer_id,
        printer_nickname: value.printer_nickname,
        title: value.title,
        notes: value.notes,
        due_at: value.due_at,
        interval_days: value.interval_days,
        status: fleet_printerreminderstatus(value.status),
        last_completed_at: value.last_completed_at,
        created_at: value.created_at,
        updated_at: value.updated_at,
    }
}

fn fleet_core_printerremindercompletioninput(
    value: PrinterReminderCompletionInput,
) -> library_core::models::PrinterReminderCompletionInput {
    library_core::models::PrinterReminderCompletionInput {
        completed_at: value.completed_at,
        notes: value.notes,
    }
}

fn fleet_printerremindercompletion(
    value: library_core::models::PrinterReminderCompletion,
) -> PrinterReminderCompletion {
    PrinterReminderCompletion {
        reminder: fleet_printerreminder(value.reminder),
        maintenance_log: fleet_printermaintenancelog(value.maintenance_log),
    }
}

impl PrinterFleetEngine for LibraryRuntime {
    fn list_printers(&self, context: &RequestContext) -> Result<Vec<Printer>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.list_printers())
            .map(|values| values.into_iter().map(fleet_printer).collect())
    }
    fn get_printer(
        &self,
        context: &RequestContext,
        id: &str,
    ) -> Result<Option<Printer>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.get_printer(id))
            .map(|value| value.map(fleet_printer))
    }
    fn create_printer(
        &self,
        context: &RequestContext,
        input: PrinterInput,
    ) -> Result<Printer, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = fleet_core_printerinput(input);
        self.mutate_db(|db| db.create_printer(&input, false))
            .map(fleet_printer)
    }
    fn update_printer(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        input: PrinterInput,
    ) -> Result<Printer, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = fleet_core_printerinput(input);
        self.mutate_db(|db| db.update_printer(id, expected_version, &input, None))
            .map(fleet_printer)
    }
    fn delete_printer(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.delete_printer(id, expected_version))
    }
    fn list_printer_maintenance_logs(
        &self,
        context: &RequestContext,
        printer_id: Option<&str>,
    ) -> Result<Vec<PrinterMaintenanceLog>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.list_printer_maintenance_logs(printer_id))
            .map(|values| {
                values
                    .into_iter()
                    .map(fleet_printermaintenancelog)
                    .collect()
            })
    }
    fn create_printer_maintenance_log(
        &self,
        context: &RequestContext,
        printer_id: &str,
        input: PrinterMaintenanceInput,
    ) -> Result<PrinterMaintenanceLog, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = fleet_core_printermaintenanceinput(input);
        self.mutate_db(|db| db.create_printer_maintenance_log(printer_id, &input))
            .map(fleet_printermaintenancelog)
    }
    fn update_printer_maintenance_log(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        input: PrinterMaintenanceInput,
    ) -> Result<PrinterMaintenanceLog, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = fleet_core_printermaintenanceinput(input);
        self.mutate_db(|db| db.update_printer_maintenance_log(id, expected_version, &input))
            .map(fleet_printermaintenancelog)
    }
    fn delete_printer_maintenance_log(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.delete_printer_maintenance_log(id, expected_version))
    }
    fn list_printer_reminders(
        &self,
        context: &RequestContext,
        printer_id: Option<&str>,
        include_completed: bool,
    ) -> Result<Vec<PrinterReminder>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.list_printer_reminders(printer_id, include_completed))
            .map(|values| values.into_iter().map(fleet_printerreminder).collect())
    }
    fn create_printer_reminder(
        &self,
        context: &RequestContext,
        printer_id: &str,
        input: PrinterReminderInput,
    ) -> Result<PrinterReminder, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = fleet_core_printerreminderinput(input);
        self.mutate_db(|db| db.create_printer_reminder(printer_id, &input))
            .map(fleet_printerreminder)
    }
    fn update_printer_reminder(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        input: PrinterReminderInput,
    ) -> Result<PrinterReminder, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = fleet_core_printerreminderinput(input);
        self.mutate_db(|db| db.update_printer_reminder(id, expected_version, &input))
            .map(fleet_printerreminder)
    }
    fn delete_printer_reminder(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.delete_printer_reminder(id, expected_version))
    }
    fn complete_printer_reminder(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        input: PrinterReminderCompletionInput,
    ) -> Result<PrinterReminderCompletion, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = fleet_core_printerremindercompletioninput(input);
        self.mutate_db(|db| db.complete_printer_reminder(id, expected_version, &input))
            .map(fleet_printerremindercompletion)
    }
    fn set_printer_reminder_paused(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        paused: bool,
    ) -> Result<PrinterReminder, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.set_printer_reminder_paused(id, expected_version, paused))
            .map(fleet_printerreminder)
    }
    fn reschedule_printer_reminder(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        due_at: &str,
    ) -> Result<PrinterReminder, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.reschedule_printer_reminder(id, expected_version, due_at))
            .map(fleet_printerreminder)
    }
}
