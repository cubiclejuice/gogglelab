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

fn scan_standard_fields<R: Read>(
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
                let value = text
                    .decode()
                    .map_err(|_| invalid_processing("3MF metadata text is invalid"))?
                    .trim()
                    .to_owned();
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
}
