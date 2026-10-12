use std::io::{Cursor, Read};

use library_protocol::{EmbeddedFilament, EmbeddedMetadataFields, RuntimeError, RuntimeErrorCode};
use quick_xml::events::Event;
use quick_xml::Reader;

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
    let mut model_xml = None;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| invalid("3MF entry is invalid"))?;
        if entry.is_file() && entry.name().to_ascii_lowercase().ends_with(".model") {
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
            model_xml = Some(xml);
            break;
        }
    }
    let xml = model_xml.ok_or_else(|| invalid("3MF package has no model metadata document"))?;
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

fn invalid(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::InvalidInput,
        message: message.into(),
        retryable: false,
        current_version: None,
    }
}
