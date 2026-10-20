use std::io::{Cursor, Read, Seek};
use stl_core::zip_bounds::{BoundedZipArchive, ZipResourceObserver};

use library_protocol::{EmbeddedFilament, EmbeddedMetadataFields, RuntimeError, RuntimeErrorCode};
use quick_xml::events::Event;
use quick_xml::XmlVersion;
use stl_core::package_bounds::{PackageProcessingContext, TextRole};
use stl_core::xml_bounds::XmlEventReader;

use crate::{LibraryRuntime, ProcessingError};

pub(crate) struct ParsedEmbeddedMetadata {
    pub(crate) fields: EmbeddedMetadataFields,
    pub(crate) filaments: Vec<EmbeddedFilament>,
}

pub(crate) fn read_metadata(
    runtime: &LibraryRuntime,
    model: &library_core::Model,
) -> Result<ParsedEmbeddedMetadata, RuntimeError> {
    let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
    read_metadata_in_context(runtime, model, &mut context)
}

fn read_metadata_in_context(
    runtime: &LibraryRuntime,
    model: &library_core::Model,
    context: &mut PackageProcessingContext,
) -> Result<ParsedEmbeddedMetadata, RuntimeError> {
    let observer = context.observer();
    observer
        .check()
        .map_err(|error| metadata_processing_error(error.into()))?;
    let result = (|| {
        let bytes = runtime.verified_model_source_bytes_in_context(model, context)?;
        let parsed =
            stl_core::parse_model_with_plates_in_context(&bytes, context).map_err(|error| {
                ProcessingError::from(error)
                    .into_runtime(invalid("3MF metadata source could not be parsed"))
            })?;
        if parsed.model.format != stl_core::StlFormat::ThreeMf {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Unsupported,
                message: "Embedded metadata is available only for 3MF models".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        let fields =
            parse_standard_fields_in_context(&bytes, context).map_err(metadata_processing_error)?;
        Ok(ParsedEmbeddedMetadata {
            fields,
            filaments: parsed
                .embedded_filaments
                .into_iter()
                .map(|filament| EmbeddedFilament {
                    slot: filament.slot,
                    name: filament.name,
                    material: filament.material,
                    color: filament.color,
                })
                .collect(),
        })
    })();
    observer
        .check()
        .map_err(|error| metadata_processing_error(error.into()))?;
    result
}

#[cfg(test)]
fn parse_standard_fields(bytes: &[u8]) -> Result<EmbeddedMetadataFields, RuntimeError> {
    let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
    parse_standard_fields_in_context(bytes, &mut context).map_err(metadata_processing_error)
}

fn metadata_processing_error(error: ProcessingError) -> RuntimeError {
    let fallback = invalid(&error.to_string());
    error.into_runtime(fallback)
}

fn parse_standard_fields_in_context(
    bytes: &[u8],
    context: &mut PackageProcessingContext,
) -> Result<EmbeddedMetadataFields, ProcessingError> {
    parse_standard_fields_reader_in_context(Cursor::new(bytes), context)
}

#[cfg(test)]
fn parse_standard_fields_with_reader<R: Read + Seek>(
    input: R,
) -> Result<EmbeddedMetadataFields, ProcessingError> {
    let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
    parse_standard_fields_reader_in_context(input, &mut context)
}

fn parse_standard_fields_reader_in_context<R: Read + Seek>(
    input: R,
    context: &mut PackageProcessingContext,
) -> Result<EmbeddedMetadataFields, ProcessingError> {
    let observer = context.observer();
    let result = (|| {
        observer.check()?;
        let mut archive = observer.open_bounded_zip(input).map_err(|error| {
            ProcessingError::from(error).with_ordinary_message("3MF metadata package is invalid")
        })?;
        if archive.len() > 10_000 {
            return Err(invalid_processing(
                "3MF metadata package has too many entries",
            ));
        }
        let root_model_path = root_model_path(&mut archive, context)?;
        let entry = archive.by_name(&root_model_path).map_err(|error| {
            ProcessingError::from(error)
                .with_ordinary_message("3MF root model metadata document is missing")
        })?;
        let entry_size = entry.size();
        if entry_size > library_core::MAX_INDEXED_FILE_BYTES {
            return Err(invalid_processing("3MF model metadata entry is too large"));
        }
        let before = context.usage().inflation_bytes;
        let fields = scan_standard_fields(context.model_reader(entry), &observer)?;
        if context.usage().inflation_bytes - before != entry_size {
            return Err(invalid_processing(
                "3MF model metadata entry changed while reading",
            ));
        }
        Ok(fields)
    })();
    observer.finish(result)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(super) enum StandardMetadataTestStop {
    Resource(stl_core::ResourceStop),
    Allocation,
}

#[cfg(test)]
#[derive(Default)]
struct StandardValueCopyProbe {
    copies: std::cell::Cell<usize>,
    copied_bytes: std::cell::Cell<usize>,
    reserves: std::cell::Cell<usize>,
    requested: std::cell::RefCell<Vec<usize>>,
    fail_at: std::cell::Cell<Option<usize>>,
    after_reserve: std::cell::Cell<Option<StandardMetadataTestStop>>,
    at_finish: std::cell::Cell<Option<StandardMetadataTestStop>>,
    decoded: std::cell::Cell<usize>,
    borrowed: std::cell::Cell<usize>,
}
#[cfg(test)]
thread_local! {
    static STANDARD_VALUE_COPY_PROBE: std::cell::RefCell<Option<std::rc::Rc<StandardValueCopyProbe>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
fn standard_probe() -> Option<std::rc::Rc<StandardValueCopyProbe>> {
    STANDARD_VALUE_COPY_PROBE.with(|slot| slot.borrow().clone())
}
#[cfg(test)]
pub(super) struct StandardMetadataProbeGuard {
    previous: Option<std::rc::Rc<StandardValueCopyProbe>>,
    probe: std::rc::Rc<StandardValueCopyProbe>,
}
#[cfg(test)]
impl StandardMetadataProbeGuard {
    pub(super) fn install() -> Self {
        let probe = std::rc::Rc::new(StandardValueCopyProbe::default());
        let previous = STANDARD_VALUE_COPY_PROBE.with(|slot| slot.replace(Some(probe.clone())));
        Self { previous, probe }
    }
    pub(super) fn fail_reserve_at(&self, attempt: usize) {
        self.probe.fail_at.set(Some(attempt));
    }
    pub(super) fn stop_after_reserve(&self, stop: StandardMetadataTestStop) {
        self.probe.after_reserve.set(Some(stop));
    }
    pub(super) fn stop_at_finish(&self, stop: StandardMetadataTestStop) {
        self.probe.at_finish.set(Some(stop));
    }
    pub(super) fn copies(&self) -> usize {
        self.probe.copies.get()
    }
    pub(super) fn reserve_attempts(&self) -> usize {
        self.probe.reserves.get()
    }
    pub(super) fn requested_lengths(&self) -> Vec<usize> {
        self.probe.requested.borrow().clone()
    }
    fn copied_bytes(&self) -> usize {
        self.probe.copied_bytes.get()
    }
}
#[cfg(test)]
impl Drop for StandardMetadataProbeGuard {
    fn drop(&mut self) {
        STANDARD_VALUE_COPY_PROBE.with(|slot| slot.replace(self.previous.take()));
    }
}
#[cfg(test)]
fn latch_test_stop(observer: &ZipResourceObserver, stop: StandardMetadataTestStop) {
    match stop {
        StandardMetadataTestStop::Resource(stop) => {
            let _ = observer.stop_io(stop);
        }
        StandardMetadataTestStop::Allocation => {
            let _ = observer.allocation_failed_io();
        }
    }
}

#[derive(Clone, Copy)]
enum StandardField {
    Designer,
    SourceUrl,
    Notes,
    ModelLicense,
    ParentModel,
    Unknown,
}
impl StandardField {
    fn classify(name: &str) -> Self {
        if name.eq_ignore_ascii_case("designer") || name.eq_ignore_ascii_case("author") {
            Self::Designer
        } else if name.eq_ignore_ascii_case("source") || name.eq_ignore_ascii_case("url") {
            Self::SourceUrl
        } else if name.eq_ignore_ascii_case("description") || name.eq_ignore_ascii_case("notes") {
            Self::Notes
        } else if name.eq_ignore_ascii_case("license") || name.eq_ignore_ascii_case("terms") {
            Self::ModelLicense
        } else if name.eq_ignore_ascii_case("parentmodel")
            || name.eq_ignore_ascii_case("parent_model")
        {
            Self::ParentModel
        } else {
            Self::Unknown
        }
    }
    fn destination(self, fields: &mut EmbeddedMetadataFields) -> Option<&mut Option<String>> {
        match self {
            Self::Designer => Some(&mut fields.designer),
            Self::SourceUrl => Some(&mut fields.source_url),
            Self::Notes => Some(&mut fields.notes),
            Self::ModelLicense => Some(&mut fields.model_license),
            Self::ParentModel => Some(&mut fields.parent_model),
            Self::Unknown => None,
        }
    }
}

// The input is borrowed trim of one admitted XML Text event. This owns only
// the selected field backing; SDK normalization/decoding are separate workspaces.
fn retain_standard_value(
    value: &str,
    observer: &ZipResourceObserver,
) -> Result<String, ProcessingError> {
    observer.check()?;
    #[cfg(test)]
    if let Some(probe) = standard_probe() {
        let attempt = probe.reserves.get() + 1;
        probe.reserves.set(attempt);
        probe.requested.borrow_mut().push(value.len());
        if probe.fail_at.get() == Some(attempt) {
            return Err(ProcessingError::from_io(observer.allocation_failed_io()));
        }
    }
    let mut retained = String::new();
    retained
        .try_reserve_exact(value.len())
        .map_err(|_| ProcessingError::from_io(observer.allocation_failed_io()))?;
    #[cfg(test)]
    if let Some(probe) = standard_probe() {
        if let Some(stop) = probe.after_reserve.take() {
            latch_test_stop(observer, stop);
        }
    }
    observer.check()?;
    retained.push_str(value);
    #[cfg(test)]
    if let Some(probe) = standard_probe() {
        probe.copies.set(probe.copies.get() + 1);
        probe
            .copied_bytes
            .set(probe.copied_bytes.get() + value.len());
    }
    Ok(retained)
}

fn scan_standard_fields<R: Read>(
    input: R,
    observer: &ZipResourceObserver,
) -> Result<EmbeddedMetadataFields, ProcessingError> {
    observer.check()?;
    let result = (|| {
        let mut reader =
            XmlEventReader::new(input, observer.clone()).map_err(ProcessingError::from)?;
        reader.set_trim_text(true);
        let mut fields = EmbeddedMetadataFields::default();
        // Some(Unknown) still decodes Text; absence of a name does not.
        let mut current = None;
        loop {
            let event = reader.next_event();
            observer.check()?;
            match event.map_err(|_| invalid_processing("3MF model metadata XML is invalid"))? {
                Event::Start(node) if node.local_name().as_ref() == b"metadata" => {
                    current = None;
                    for attribute in node.attributes().with_checks(true) {
                        let attribute = attribute
                            .map_err(|_| invalid_processing("3MF metadata attribute is invalid"))?;
                        if attribute.key.local_name().as_ref() == b"name" {
                            let name = attribute
                                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .map_err(|_| invalid_processing("3MF metadata name is invalid"))?;
                            current = Some(StandardField::classify(&name));
                        }
                    }
                }
                Event::Text(text) if current.is_some() => {
                    // Validate even Unknown/occupied selections exactly as before.
                    let decoded = text
                        .decode()
                        .map_err(|_| invalid_processing("3MF metadata text is invalid"))?;
                    #[cfg(test)]
                    if let Some(probe) = standard_probe() {
                        probe.decoded.set(probe.decoded.get() + 1);
                        if matches!(&decoded, std::borrow::Cow::Borrowed(_)) {
                            probe.borrowed.set(probe.borrowed.get() + 1);
                        }
                    }
                    let value = decoded.trim();
                    if value.is_empty() {
                        continue;
                    }
                    let Some(destination) = current.unwrap().destination(&mut fields) else {
                        continue;
                    };
                    if destination.is_none() {
                        *destination = Some(retain_standard_value(value, observer)?);
                    }
                }
                Event::End(node) if node.local_name().as_ref() == b"metadata" => {
                    current = None;
                }
                Event::DocType(_) => {
                    return Err(invalid_processing("3MF metadata XML cannot contain a DTD"))
                }
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(fields)
    })();
    #[cfg(test)]
    if let Some(probe) = standard_probe() {
        if let Some(stop) = probe.at_finish.take() {
            latch_test_stop(observer, stop);
        }
    }
    observer.finish(result)
}

#[cfg(test)]
fn scan_standard_fields_reference<R: Read>(
    input: R,
    observer: &ZipResourceObserver,
) -> Result<EmbeddedMetadataFields, ProcessingError> {
    let mut reader = XmlEventReader::new(input, observer.clone()).map_err(ProcessingError::from)?;
    reader.set_trim_text(true);
    let mut fields = EmbeddedMetadataFields::default();
    let mut current_name = None::<String>;
    loop {
        let event = reader.next_event();
        observer.check()?;
        match event.map_err(|_| invalid_processing("3MF model metadata XML is invalid"))? {
            Event::Start(node) if node.local_name().as_ref() == b"metadata" => {
                current_name = None;
                for attribute in node.attributes().with_checks(true) {
                    let attribute = attribute
                        .map_err(|_| invalid_processing("3MF metadata attribute is invalid"))?;
                    if attribute.key.local_name().as_ref() == b"name" {
                        current_name = Some(
                            attribute
                                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .map_err(|_| invalid_processing("3MF metadata name is invalid"))?
                                .into_owned(),
                        );
                    }
                }
            }
            Event::Text(text) if current_name.is_some() => {
                let value = {
                    let decoded = text
                        .decode()
                        .map_err(|_| invalid_processing("3MF metadata text is invalid"))?;
                    let value = decoded.trim();
                    value.to_owned()
                };
                if !value.is_empty() {
                    match current_name
                        .as_deref()
                        .unwrap()
                        .to_ascii_lowercase()
                        .as_str()
                    {
                        "designer" | "author" => {
                            fields.designer.get_or_insert(value);
                        }
                        "source" | "url" => {
                            fields.source_url.get_or_insert(value);
                        }
                        "description" | "notes" => {
                            fields.notes.get_or_insert(value);
                        }
                        "license" | "terms" => {
                            fields.model_license.get_or_insert(value);
                        }
                        "parentmodel" | "parent_model" => {
                            fields.parent_model.get_or_insert(value);
                        }
                        _ => {}
                    }
                }
            }
            Event::End(node) if node.local_name().as_ref() == b"metadata" => {
                current_name = None;
            }
            Event::DocType(_) => {
                return Err(invalid_processing("3MF metadata XML cannot contain a DTD"))
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(fields)
}

/// Selects the root model document in the same way as `stl_core`'s 3MF
/// geometry parser: follow the package-level model relationship, then fall
/// back to the conventional root part when no relationship target is present.
fn root_model_path<R: Read + Seek>(
    archive: &mut BoundedZipArchive<R>,
    context: &mut PackageProcessingContext,
) -> Result<String, ProcessingError> {
    let has_relationships = archive.file_names().any(|name| name == "_rels/.rels");
    if has_relationships {
        let observer = context.observer();
        let entry = archive.by_name("_rels/.rels").map_err(|error| {
            ProcessingError::from(error)
                .with_ordinary_message("3MF package relationships are invalid")
        })?;
        if entry.size() > TextRole::Relationships.byte_limit() as u64 {
            let _ = observer.stop_io(stl_core::ResourceStop::InputLimitExceeded);
            observer.check()?;
        }
        let relationships = context
            .collect_text(entry, TextRole::Relationships)
            .map_err(|error| {
                ProcessingError::from(error)
                    .with_ordinary_message("3MF package relationships are invalid")
            })?;
        if let Some(target) = model_relationship_target(&relationships, &observer)? {
            // OPC part names may begin with `/`, while ZIP entry names do not.
            return Ok(target.trim_start_matches('/').to_owned());
        }
    }

    const ROOT_MODEL_FALLBACK: &str = "3D/3dmodel.model";
    if archive.file_names().any(|name| name == ROOT_MODEL_FALLBACK) {
        Ok(ROOT_MODEL_FALLBACK.into())
    } else {
        Err(invalid_processing(
            "3MF package has no model metadata document",
        ))
    }
}

fn model_relationship_target(
    relationships: &str,
    observer: &ZipResourceObserver,
) -> Result<Option<String>, ProcessingError> {
    let mut reader = XmlEventReader::new(relationships.as_bytes(), observer.clone())
        .map_err(ProcessingError::from)?;
    let target = (|| loop {
        match reader.next_event() {
            Ok(Event::Start(element)) | Ok(Event::Empty(element))
                if element.local_name().as_ref() == b"Relationship" =>
            {
                let mut target = None;
                let mut is_model = false;
                for attribute in element.attributes().flatten() {
                    let value = attribute.normalized_value(XmlVersion::Implicit1_0).ok()?;
                    match attribute.key.local_name().as_ref() {
                        b"Target" => target = Some(value.into_owned()),
                        b"Type" => is_model = value.ends_with("/3dmodel"),
                        _ => {}
                    }
                }
                if is_model {
                    return target;
                }
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    })();
    observer.finish(Ok(target))
}

fn invalid_processing(message: &str) -> ProcessingError {
    message.into()
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

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Seek, Write};

    use zip::write::SimpleFileOptions;

    use super::{
        metadata_processing_error, parse_standard_fields, parse_standard_fields_with_reader,
    };
    use crate::ProcessingError;
    use library_protocol::RuntimeErrorCode;
    use stl_core::ResourceStop;

    struct StoppedMetadataReader {
        cursor: Cursor<Vec<u8>>,
        stop: ResourceStop,
        allocation: bool,
        fail_at: u64,
    }

    impl Read for StoppedMetadataReader {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            if !bytes.is_empty() && self.cursor.position() == self.fail_at {
                return Err(if self.allocation {
                    stl_core::zip_bounds::ZipAllocationFailed.into_io_error()
                } else {
                    self.stop.into_io_error()
                });
            }
            self.cursor.read(bytes)
        }
    }

    impl Seek for StoppedMetadataReader {
        fn seek(&mut self, from: std::io::SeekFrom) -> std::io::Result<u64> {
            self.cursor.seek(from)
        }
    }

    fn package(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            archive
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn embedded_metadata_uses_relationship_root_model_before_component_parts() {
        let relationships = br#"<Relationships>
            <Relationship Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"
                Target="/3D/3dmodel.model" />
        </Relationships>"#;
        let component =
            br#"<model><metadata name="ParentModel">Component Parent</metadata></model>"#;
        let root = br#"<model><metadata name="ParentModel">Root Parent</metadata></model>"#;
        let bytes = package(&[
            ("_rels/.rels", relationships),
            ("3D/Objects/component.model", component),
            ("3D/3dmodel.model", root),
        ]);

        let fields = parse_standard_fields(&bytes).unwrap();

        assert_eq!(fields.parent_model.as_deref(), Some("Root Parent"));
    }

    #[test]
    fn embedded_metadata_falls_back_to_the_conventional_root_model_part() {
        let component =
            br#"<model><metadata name="ParentModel">Component Parent</metadata></model>"#;
        let root = br#"<model><metadata name="ParentModel">Root Parent</metadata></model>"#;
        let bytes = package(&[
            ("3D/Objects/component.model", component),
            ("3D/3dmodel.model", root),
        ]);

        let fields = parse_standard_fields(&bytes).unwrap();

        assert_eq!(fields.parent_model.as_deref(), Some("Root Parent"));
    }

    #[test]
    fn typed_embedded_metadata_preserves_terminal_relationship_reads_without_fallback() {
        let relationships = br#"<Relationships/>"#;
        let root = br#"<model><metadata name="ParentModel">Must not return this fallback</metadata></model>"#;
        let bytes = package(&[("_rels/.rels", relationships), ("3D/3dmodel.model", root)]);
        let name_bytes = u16::from_le_bytes(bytes[26..28].try_into().unwrap()) as u64;
        let extra_bytes = u16::from_le_bytes(bytes[28..30].try_into().unwrap()) as u64;
        let first_payload = 30 + name_bytes + extra_bytes;
        for (allocation, stop, code, retryable) in [
            (
                false,
                ResourceStop::InputLimitExceeded,
                RuntimeErrorCode::Unsupported,
                false,
            ),
            (
                false,
                ResourceStop::ReadWorkExceeded,
                RuntimeErrorCode::Unsupported,
                false,
            ),
            (
                false,
                ResourceStop::EnvelopeChanged,
                RuntimeErrorCode::Stale,
                true,
            ),
            (
                true,
                ResourceStop::InputLimitExceeded,
                RuntimeErrorCode::Internal,
                false,
            ),
        ] {
            let result = parse_standard_fields_with_reader(StoppedMetadataReader {
                cursor: Cursor::new(bytes.clone()),
                stop,
                allocation,
                fail_at: first_payload,
            });
            let error = match result {
                Ok(_) => panic!("terminal relationship read must not select cached fallback"),
                Err(error) => error,
            };
            if allocation {
                assert!(matches!(error, ProcessingError::AllocationFailed));
            } else {
                assert!(
                    matches!(error, ProcessingError::ResourceStopped(actual) if actual == stop)
                );
            }
            let mapped = metadata_processing_error(error);
            assert_eq!(mapped.code, code);
            assert_eq!(mapped.retryable, retryable);
        }
        // An ordinary relationship without a target keeps conventional fallback.
        assert_eq!(
            parse_standard_fields(&bytes)
                .unwrap()
                .parent_model
                .as_deref(),
            Some("Must not return this fallback")
        );
        let ordinary = parse_standard_fields(b"not a ZIP").err().unwrap();
        assert_eq!(ordinary.code, RuntimeErrorCode::InvalidInput);
        assert_eq!(ordinary.message, "3MF metadata package is invalid");
    }
    #[test]
    fn runtime_metadata_streamed_xml_guard_preserves_terminal_and_ordinary_relationship_policy() {
        let root = b"<model><metadata name='Designer'>Ada</metadata></model>";
        let malformed = b"<Relationships><";
        let bytes = package(&[("_rels/.rels", malformed), ("3D/3dmodel.model", root)]);
        let mut context = stl_core::package_bounds::PackageProcessingContext::new(
            stl_core::zip_bounds::ZipResourceObserver::new(),
        );
        let fields = super::parse_standard_fields_in_context(&bytes, &mut context).unwrap();
        assert_eq!(fields.designer.as_deref(), Some("Ada"));
        assert_eq!(
            context.usage().inflation_bytes,
            (malformed.len() + root.len()) as u64
        );
        assert_eq!(context.usage().retained_input_bytes, malformed.len());
        let oversized = format!(
            "<model><metadata name='{}'>never</metadata></model>",
            "x".repeat(4097)
        );
        let bytes = package(&[("3D/3dmodel.model", oversized.as_bytes())]);
        let mut context = stl_core::package_bounds::PackageProcessingContext::new(
            stl_core::zip_bounds::ZipResourceObserver::new(),
        );
        let error = super::parse_standard_fields_in_context(&bytes, &mut context).unwrap_err();
        assert!(matches!(
            error,
            ProcessingError::ResourceStopped(ResourceStop::InputLimitExceeded)
        ));
        assert!(context.observer().check().is_err());
        let before = context.usage();
        assert!(super::parse_standard_fields_in_context(&bytes, &mut context).is_err());
        assert_eq!(context.usage(), before);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn runtime_archived_source_validation_geometry_metadata_and_preview_share_quantitative_context()
    {
        use crate::{LibraryRuntime, RuntimeConfig, RuntimeRootPolicy};
        use library_protocol::{Capability, RootAccess};
        use std::collections::BTreeSet;
        use stl_core::package_bounds::PackageProcessingContext;
        use stl_core::zip_bounds::ZipResourceObserver;
        let root=br#"<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><metadata name="Designer">Ada</metadata><resources><object id="1" type="model"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#;
        let rels=br#"<Relationships><Relationship Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel" Target="/3D/3dmodel.model"/><Relationship Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail" Target="/Metadata/custom.png"/></Relationships>"#;
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[1, 2, 3, 255]).unwrap();
            writer.finish().unwrap();
        }
        let inner = package(&[
            ("_rels/.rels", rels),
            ("3D/3dmodel.model", root),
            ("Metadata/custom.png", &png),
        ]);
        let outer = package(&[("nested.3mf", &inner)]);
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let sources = base.join("sources");
        std::fs::create_dir(&sources).unwrap();
        let source = sources.join("models.zip");
        std::fs::write(&source, &outer).unwrap();
        let runtime = LibraryRuntime::open(RuntimeConfig {
            library_root: base.join("library"),
            integration_settings_path: base.join("integrations.json"),
            roots: vec![RuntimeRootPolicy {
                id: "fixture".into(),
                label: "Models".into(),
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
        index.scan_zip = true;
        let index = runtime
            .mutate_db(|db| db.update_index_root(&index))
            .unwrap();
        crate::scanner::scan_root(&runtime, &index).unwrap();
        let model = runtime
            .read_db(|db| db.list_models(None, None))
            .unwrap()
            .into_iter()
            .find(|model| model.source_archive_member.as_deref() == Some("nested.3mf"))
            .unwrap();
        let one_pass = (rels.len() + root.len()) as u64;
        let mut metadata_context = PackageProcessingContext::new(ZipResourceObserver::new());
        let metadata =
            super::read_metadata_in_context(&runtime, &model, &mut metadata_context).unwrap();
        assert_eq!(metadata.fields.designer.as_deref(), Some("Ada"));
        assert_eq!(
            metadata_context.usage().inflation_bytes,
            one_pass * 3,
            "archive validation, geometry and streamed metadata must all charge"
        );
        assert_eq!(
            metadata_context.usage().retained_input_bytes,
            rels.len() * 3
        );
        let mut preview_context = PackageProcessingContext::new(ZipResourceObserver::new());
        let bytes = runtime
            .verified_model_source_bytes_in_context(&model, &mut preview_context)
            .unwrap();
        assert_eq!(bytes, inner);
        assert_eq!(
            preview_context.usage().inflation_bytes,
            one_pass,
            "hidden archive validation must use caller context"
        );
        let parsed = stl_core::parse_model_in_context(&bytes, &mut preview_context).unwrap();
        assert_eq!(parsed.triangle_count, 1);
        assert_eq!(preview_context.usage().inflation_bytes, one_pass * 2);
        let preview =
            crate::images::embedded_3mf_thumbnail_in_context_typed(&bytes, &mut preview_context)
                .unwrap()
                .unwrap();
        assert!(preview.starts_with(b"\x89PNG"));
        assert_eq!(
            preview_context.usage().inflation_bytes,
            one_pass * 2 + rels.len() as u64 + png.len() as u64
        );
        assert_eq!(
            preview_context.usage().retained_input_bytes,
            rels.len() * 3,
            "binary image must not spend text counter"
        );
        assert_eq!(std::fs::read(&source).unwrap(), outer);
    }
    #[test]
    fn standard_metadata_value_copies_do_not_exceed_five_retained_destinations() {
        let mut xml = String::from("<model>");
        for (name, value) in [
            ("Designer", " First Designer "),
            ("Source", "https://example.test/source"),
            ("Description", " First Notes "),
            ("License", " CC0 "),
            ("ParentModel", " First Parent "),
        ] {
            xml.push_str(&format!("<metadata name=\"{name}\">{value}</metadata>"));
        }
        for index in 0..200 {
            xml.push_str(&format!(
                "<metadata name=\"Unknown{index}\">ignored value</metadata>"
            ));
            for alias in ["AUTHOR", "URL", "notes", "TERMS", "parent_model"] {
                xml.push_str(&format!(
                    "<metadata name=\"{alias}\">discarded duplicate</metadata>"
                ));
            }
        }
        xml.push_str("</model>");
        let probe = super::StandardMetadataProbeGuard::install();
        let observer = stl_core::zip_bounds::ZipResourceObserver::new();
        let fields = super::scan_standard_fields(xml.as_bytes(), &observer).unwrap();
        assert_eq!(fields.designer.as_deref(), Some("First Designer"));
        assert_eq!(
            fields.source_url.as_deref(),
            Some("https://example.test/source")
        );
        assert_eq!(fields.notes.as_deref(), Some("First Notes"));
        assert_eq!(fields.model_license.as_deref(), Some("CC0"));
        assert_eq!(fields.parent_model.as_deref(), Some("First Parent"));
        assert!(observer.check().is_ok());
        let copies = probe.copies();
        assert!(
            copies >= 5,
            "actual nonempty value copy path must be observed"
        );
        assert!(probe.copied_bytes() > 0);
        assert!(copies <= 5,
            "only five retained field values need backing copies; actual explicit nonempty copies: {copies}");
    }
    fn compare_standard_scan(xml: &[u8]) {
        let expected = super::scan_standard_fields_reference(
            xml,
            &stl_core::zip_bounds::ZipResourceObserver::new(),
        );
        let actual =
            super::scan_standard_fields(xml, &stl_core::zip_bounds::ZipResourceObserver::new());
        match (actual, expected) {
            (Ok(actual), Ok(expected)) => assert_eq!(actual, expected),
            (Err(actual), Err(expected)) => assert_eq!(actual.to_string(), expected.to_string()),
            _ => panic!("new retention path changed scanner success/error"),
        }
    }

    #[test]
    fn standard_metadata_selectors_and_event_order_match_reference() {
        for xml in [
            "<model><metadata name='dEsIgNeR'>A</metadata><metadata name='AUTHOR'>B</metadata></model>",
            "<model><metadata name='URL'>U</metadata><metadata name='NOTES'>N</metadata><metadata name='TERMS'>T</metadata><metadata name='PARENT_MODEL'>P</metadata></model>",
            "<model><metadata>none</metadata><metadata name=''>empty name</metadata><metadata name=' designer '>padded</metadata><metadata name='dèsigner'>nonascii</metadata></model>",
            "<model><metadata name='Designer' q:name='Unknown'>discard</metadata><metadata name='Unknown' q:name='Notes'>notes</metadata></model>",
            "<model><metadata name='De&#115;igner'>é🙂</metadata></model>",
            "<model><metadata name='Designer'> <![CDATA[not text]]> A&amp;B </metadata></model>",
            "<model><metadata name='Designer'><metadata name='Notes'/>A</metadata></model>",
            "<model><metadata name='Designer'>A<metadata name='Notes'>N</metadata>after</metadata></model>",
            "<model><metadata name='Designer'> </metadata><metadata name='author'>later</metadata></model>",
            "<model><metadata name='Designer' bad>value</metadata></model>",
            "<model><metadata name='Designer' q:name='&invalid;'>value</metadata></model>",
            "<model><metadata name='Designer' bad='x' bad='y'>value</metadata></model>",
        ] { compare_standard_scan(xml.as_bytes()); }
    }

    #[test]
    fn standard_metadata_unknown_and_occupied_still_decode_but_missing_name_does_not() {
        for prefix in [
            "<model><metadata name='Unknown'>",
            "<model><metadata name=''>",
            "<model><metadata name='Designer'>first</metadata><metadata name='Author'>",
        ] {
            let mut xml = prefix.as_bytes().to_vec();
            xml.push(0xff);
            xml.extend_from_slice(b"</metadata></model>");
            compare_standard_scan(&xml);
            let error = super::scan_standard_fields(
                &xml[..],
                &stl_core::zip_bounds::ZipResourceObserver::new(),
            )
            .unwrap_err();
            assert!(!error.is_terminal());
            assert_eq!(
                metadata_processing_error(error).code,
                RuntimeErrorCode::InvalidInput
            );
        }
        let mut xml = b"<model><metadata>".to_vec();
        xml.push(0xff);
        xml.extend_from_slice(b"</metadata></model>");
        compare_standard_scan(&xml);
        assert!(super::scan_standard_fields(
            &xml[..],
            &stl_core::zip_bounds::ZipResourceObserver::new()
        )
        .is_ok());
    }

    #[test]
    fn standard_metadata_value_requests_are_exact_and_text_decode_is_borrowed() {
        let probe = super::StandardMetadataProbeGuard::install();
        let xml = "<model><metadata name='Designer'> é🙂 </metadata><metadata name='Unknown'>unknown</metadata><metadata name='Author'>occupied</metadata></model>";
        let fields = super::scan_standard_fields(
            xml.as_bytes(),
            &stl_core::zip_bounds::ZipResourceObserver::new(),
        )
        .unwrap();
        assert_eq!(fields.designer.as_deref(), Some("é🙂"));
        assert_eq!(probe.copies(), 1);
        assert_eq!(probe.reserve_attempts(), 1);
        assert_eq!(probe.requested_lengths(), ["é🙂".len()]);
        assert_eq!(probe.probe.decoded.get(), 3);
        assert_eq!(
            probe.probe.borrowed.get(),
            3,
            "actual borrowed Text/normal UTF-8 path; attribute Cow remains separate"
        );
    }

    #[test]
    fn standard_metadata_five_admitted_text_events_bound_selected_payload() {
        let mut xml = String::from("<model>");
        for name in ["Designer", "Source", "Notes", "License", "ParentModel"] {
            xml.push_str(&format!(
                "<metadata name='{name}'>{}</metadata>",
                "x".repeat(65536)
            ));
        }
        xml.push_str("</model>");
        let probe = super::StandardMetadataProbeGuard::install();
        let fields = super::scan_standard_fields(
            xml.as_bytes(),
            &stl_core::zip_bounds::ZipResourceObserver::new(),
        )
        .unwrap();
        assert_eq!(probe.copies(), 5);
        assert_eq!(probe.requested_lengths(), [65536; 5]);
        assert_eq!(probe.copied_bytes(), 5 * 65536);
        assert_eq!(fields.designer.as_ref().unwrap().len(), 65536);
        drop(probe);
        let probe = super::StandardMetadataProbeGuard::install();
        let too_large = format!(
            "<model><metadata name='Unknown'>{}</metadata></model>",
            "x".repeat(65537)
        );
        let error = super::scan_standard_fields(
            too_large.as_bytes(),
            &stl_core::zip_bounds::ZipResourceObserver::new(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ProcessingError::ResourceStopped(ResourceStop::InputLimitExceeded)
        ));
        assert_eq!(probe.copies(), 0);
        assert_eq!(probe.reserve_attempts(), 0);
    }

    #[test]
    fn standard_metadata_selected_reserve_failure_and_stops_keep_first_cause() {
        use super::StandardMetadataTestStop;
        let bytes = package(&[(
            "3D/3dmodel.model",
            b"<model><metadata name='Designer'>Value</metadata></model>",
        )]);
        let probe = super::StandardMetadataProbeGuard::install();
        probe.fail_reserve_at(1);
        probe.stop_at_finish(StandardMetadataTestStop::Resource(
            ResourceStop::EnvelopeChanged,
        ));
        let observer = stl_core::zip_bounds::ZipResourceObserver::new();
        let mut context = stl_core::package_bounds::PackageProcessingContext::new(observer.clone());
        assert!(matches!(
            super::parse_standard_fields_in_context(&bytes, &mut context),
            Err(ProcessingError::AllocationFailed)
        ));
        assert!(matches!(
            observer.check(),
            Err(stl_core::zip_bounds::ZipOpenError::AllocationFailed)
        ));
        assert_eq!(probe.reserve_attempts(), 1);
        assert_eq!(probe.copies(), 0);
        drop(probe);
        for stop in [
            StandardMetadataTestStop::Resource(ResourceStop::InputLimitExceeded),
            StandardMetadataTestStop::Resource(ResourceStop::ReadWorkExceeded),
            StandardMetadataTestStop::Resource(ResourceStop::EnvelopeChanged),
            StandardMetadataTestStop::Allocation,
        ] {
            let probe = super::StandardMetadataProbeGuard::install();
            probe.stop_after_reserve(stop);
            let observer = stl_core::zip_bounds::ZipResourceObserver::new();
            let mut context =
                stl_core::package_bounds::PackageProcessingContext::new(observer.clone());
            let error = super::parse_standard_fields_in_context(&bytes, &mut context).unwrap_err();
            match stop {
                StandardMetadataTestStop::Resource(expected) => assert!(
                    matches!(error, ProcessingError::ResourceStopped(actual) if actual == expected)
                ),
                StandardMetadataTestStop::Allocation => {
                    assert!(matches!(error, ProcessingError::AllocationFailed))
                }
            }
            assert_eq!(probe.reserve_attempts(), 1);
            assert_eq!(probe.copies(), 0);
        }
    }

    #[test]
    fn standard_metadata_prior_and_final_stops_never_return_fields() {
        use super::StandardMetadataTestStop;
        let xml = b"<model><metadata name='Designer'>Value</metadata></model>";
        for stop in [
            StandardMetadataTestStop::Resource(ResourceStop::InputLimitExceeded),
            StandardMetadataTestStop::Resource(ResourceStop::ReadWorkExceeded),
            StandardMetadataTestStop::Resource(ResourceStop::EnvelopeChanged),
            StandardMetadataTestStop::Allocation,
        ] {
            let probe = super::StandardMetadataProbeGuard::install();
            let observer = stl_core::zip_bounds::ZipResourceObserver::new();
            super::latch_test_stop(&observer, stop);
            assert!(super::scan_standard_fields(&xml[..], &observer).is_err());
            assert_eq!(probe.reserve_attempts(), 0);
            assert_eq!(probe.copies(), 0);
            drop(probe);
            let probe = super::StandardMetadataProbeGuard::install();
            probe.stop_at_finish(stop);
            let observer = stl_core::zip_bounds::ZipResourceObserver::new();
            assert!(super::scan_standard_fields(&xml[..], &observer).is_err());
            assert_eq!(probe.copies(), 1);
        }
    }

    #[test]
    fn standard_metadata_probe_scopes_restore_after_nesting_and_panic() {
        assert!(super::standard_probe().is_none());
        let outer = super::StandardMetadataProbeGuard::install();
        let original = super::standard_probe().unwrap();
        {
            let _inner = super::StandardMetadataProbeGuard::install();
        }
        assert!(std::rc::Rc::ptr_eq(
            &super::standard_probe().unwrap(),
            &original
        ));
        let panic = std::panic::catch_unwind(|| {
            let _inner = super::StandardMetadataProbeGuard::install();
            panic!("synthetic metadata probe unwind");
        });
        assert!(panic.is_err());
        assert!(std::rc::Rc::ptr_eq(
            &super::standard_probe().unwrap(),
            &original
        ));
        drop(outer);
        assert!(super::standard_probe().is_none());
    }
}
