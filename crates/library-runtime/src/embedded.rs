use std::io::{Cursor, Read};

use library_protocol::{EmbeddedFilament, EmbeddedMetadataFields, RuntimeError, RuntimeErrorCode};
use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::LibraryRuntime;

pub(crate) struct ParsedEmbeddedMetadata {
    pub(crate) fields: EmbeddedMetadataFields,
    pub(crate) filaments: Vec<EmbeddedFilament>,
}

pub(crate) fn read_metadata(
    runtime: &LibraryRuntime,
    model: &library_core::Model,
) -> Result<ParsedEmbeddedMetadata, RuntimeError> {
    let bytes = runtime.verified_model_source_bytes(model)?;
    let parsed = stl_core::parse_model_with_plates(&bytes)
        .map_err(|_| invalid("3MF metadata source could not be parsed"))?;
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
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| invalid("3MF metadata package is invalid"))?;
    if archive.len() > 10_000 {
        return Err(invalid("3MF metadata package has too many entries"));
    }
    let root_model_path = root_model_path(&mut archive)?;
    let mut entry = archive
        .by_name(&root_model_path)
        .map_err(|_| invalid("3MF root model metadata document is missing"))?;
    let entry_size = entry.size();
    if entry_size > library_core::MAX_INDEXED_FILE_BYTES {
        return Err(invalid("3MF model metadata entry is too large"));
    }
    let mut xml = Vec::with_capacity(entry_size as usize);
    entry
        .by_ref()
        .take(entry_size.saturating_add(1))
        .read_to_end(&mut xml)
        .map_err(|_| invalid("3MF model metadata entry could not be read"))?;
    if xml.len() as u64 != entry_size {
        return Err(invalid("3MF model metadata entry changed while reading"));
    }
    let mut reader = Reader::from_reader(xml.as_slice());
    reader.config_mut().trim_text(true);
    let mut fields = EmbeddedMetadataFields::default();
    let mut current_name = None::<String>;
    loop {
        match reader
            .read_event()
            .map_err(|_| invalid("3MF model metadata XML is invalid"))?
        {
            Event::Start(node) if node.local_name().as_ref() == b"metadata" => {
                current_name = None;
                for attribute in node.attributes().with_checks(true) {
                    let attribute =
                        attribute.map_err(|_| invalid("3MF metadata attribute is invalid"))?;
                    if attribute.key.local_name().as_ref() == b"name" {
                        current_name = Some(
                            attribute
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|_| invalid("3MF metadata name is invalid"))?
                                .into_owned(),
                        );
                    }
                }
            }
            Event::Text(text) if current_name.is_some() => {
                let value = text
                    .decode()
                    .map_err(|_| invalid("3MF metadata text is invalid"))?
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
            Event::DocType(_) => return Err(invalid("3MF metadata XML cannot contain a DTD")),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(fields)
}

/// Selects the root model document in the same way as `stl_core`'s 3MF
/// geometry parser: follow the package-level model relationship, then fall
/// back to the conventional root part when no relationship target is present.
fn root_model_path(archive: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Result<String, RuntimeError> {
    let has_relationships = archive.file_names().any(|name| name == "_rels/.rels");
    if has_relationships {
        let mut relationships = String::new();
        archive
            .by_name("_rels/.rels")
            .map_err(|_| invalid("3MF package relationships are invalid"))?
            .read_to_string(&mut relationships)
            .map_err(|_| invalid("3MF package relationships are invalid"))?;
        if let Some(target) = model_relationship_target(&relationships) {
            // OPC part names may begin with `/`, while ZIP entry names do not.
            return Ok(target.trim_start_matches('/').to_owned());
        }
    }

    const ROOT_MODEL_FALLBACK: &str = "3D/3dmodel.model";
    if archive.file_names().any(|name| name == ROOT_MODEL_FALLBACK) {
        Ok(ROOT_MODEL_FALLBACK.into())
    } else {
        Err(invalid("3MF package has no model metadata document"))
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
    use std::io::{Cursor, Write};

    use zip::write::SimpleFileOptions;

    use super::parse_standard_fields;

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
}
