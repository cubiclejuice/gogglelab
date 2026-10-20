use std::io::{Cursor, Read, Seek};
use stl_core::zip_bounds::{BoundedZipArchive, ZipResourceObserver};

use library_protocol::{EmbeddedFilament, EmbeddedMetadataFields, RuntimeError, RuntimeErrorCode};
use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::{LibraryRuntime, ProcessingError};

pub(crate) struct ParsedEmbeddedMetadata {
    pub(crate) fields: EmbeddedMetadataFields,
    pub(crate) filaments: Vec<EmbeddedFilament>,
}

pub(crate) fn read_metadata(
    runtime: &LibraryRuntime,
    model: &library_core::Model,
) -> Result<ParsedEmbeddedMetadata, RuntimeError> {
    let bytes = runtime.verified_model_source_bytes(model)?;
    let parsed = stl_core::parse_model_with_plates(&bytes).map_err(|error| {
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
    let fields = parse_standard_fields(&bytes)?;
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
}

fn parse_standard_fields(bytes: &[u8]) -> Result<EmbeddedMetadataFields, RuntimeError> {
    parse_standard_fields_with_reader(Cursor::new(bytes)).map_err(metadata_processing_error)
}

fn metadata_processing_error(error: ProcessingError) -> RuntimeError {
    let fallback = invalid(&error.to_string());
    error.into_runtime(fallback)
}

fn parse_standard_fields_with_reader<R: Read + Seek>(
    input: R,
) -> Result<EmbeddedMetadataFields, ProcessingError> {
    let observer = ZipResourceObserver::new();
    let result = (|| {
        let mut archive = observer.open_bounded_zip(input).map_err(|error| {
            ProcessingError::from(error).with_ordinary_message("3MF metadata package is invalid")
        })?;
        if archive.len() > 10_000 {
            return Err(invalid_processing(
                "3MF metadata package has too many entries",
            ));
        }
        let root_model_path = root_model_path(&mut archive)?;
        let mut entry = archive.by_name(&root_model_path).map_err(|error| {
            ProcessingError::from(error)
                .with_ordinary_message("3MF root model metadata document is missing")
        })?;
        let entry_size = entry.size();
        if entry_size > library_core::MAX_INDEXED_FILE_BYTES {
            return Err(invalid_processing("3MF model metadata entry is too large"));
        }
        let mut xml = Vec::with_capacity(entry_size as usize);
        entry
            .by_ref()
            .take(entry_size.saturating_add(1))
            .read_to_end(&mut xml)
            .map_err(|error| {
                ProcessingError::from_io(error)
                    .with_ordinary_message("3MF model metadata entry could not be read")
            })?;
        if xml.len() as u64 != entry_size {
            return Err(invalid_processing(
                "3MF model metadata entry changed while reading",
            ));
        }
        let mut reader = Reader::from_reader(xml.as_slice());
        reader.config_mut().trim_text(true);
        let mut fields = EmbeddedMetadataFields::default();
        let mut current_name = None::<String>;
        loop {
            match reader
                .read_event()
                .map_err(|_| invalid_processing("3MF model metadata XML is invalid"))?
            {
                Event::Start(node) if node.local_name().as_ref() == b"metadata" => {
                    current_name = None;
                    for attribute in node.attributes().with_checks(true) {
                        let attribute = attribute
                            .map_err(|_| invalid_processing("3MF metadata attribute is invalid"))?;
                        if attribute.key.local_name().as_ref() == b"name" {
                            current_name = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        quick_xml::XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )
                                    .map_err(|_| {
                                        invalid_processing("3MF metadata name is invalid")
                                    })?
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
    })();
    observer.finish(result)
}

/// Selects the root model document in the same way as `stl_core`'s 3MF
/// geometry parser: follow the package-level model relationship, then fall
/// back to the conventional root part when no relationship target is present.
fn root_model_path<R: Read + Seek>(
    archive: &mut BoundedZipArchive<R>,
) -> Result<String, ProcessingError> {
    let has_relationships = archive.file_names().any(|name| name == "_rels/.rels");
    if has_relationships {
        let mut relationships = String::new();
        archive
            .by_name("_rels/.rels")
            .map_err(|error| {
                ProcessingError::from(error)
                    .with_ordinary_message("3MF package relationships are invalid")
            })?
            .read_to_string(&mut relationships)
            .map_err(|error| {
                ProcessingError::from_io(error)
                    .with_ordinary_message("3MF package relationships are invalid")
            })?;
        if let Some(target) = model_relationship_target(&relationships) {
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

fn model_relationship_target(relationships: &str) -> Option<String> {
    let mut reader = Reader::from_str(relationships);
    loop {
        match reader.read_event() {
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
    }
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
}
