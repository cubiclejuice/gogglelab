use std::{
    env, fs,
    io::{Cursor, Write},
    process::ExitCode,
};

use stl_core::parse_model_with_plates;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel" Target="/3D/3dmodel.model"/>
</Relationships>"#;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
 <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
 <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
 <Default Extension="config" ContentType="application/xml"/>
 <Default Extension="json" ContentType="application/json"/>
</Types>"#;

fn triangle_object(id: u32, name: &str, color_index: u32) -> String {
    format!(
        r#"<object id="{id}" type="model" name="{name}" pid="1" pindex="{color_index}"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="20" y="0" z="0"/><vertex x="0" y="20" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object>"#
    )
}

fn model() -> String {
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06">
 <metadata name="Title">Colorway UI Fixture</metadata>
 <resources><basematerials id="1"><base name="White" displaycolor="#F8F8F8"/><base name="Charcoal" displaycolor="#1F2937"/><base name="Orange" displaycolor="#FF6B35"/></basematerials>{}</resources>
 <build><item objectid="1"/><item objectid="2"/><item objectid="3"/><item objectid="4"/></build>
</model>"##,
        [
            triangle_object(1, "Shell", 0),
            triangle_object(2, "Badge", 2),
            triangle_object(3, "Top", 1),
            triangle_object(4, "Button", 2),
        ]
        .join("")
    )
}

fn metadata() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
 <plate><metadata key="plater_id" value="1"/><metadata key="plater_name" value="Housing"/>
  <model_instance><metadata key="object_id" value="1"/><metadata key="instance_id" value="0"/></model_instance>
  <model_instance><metadata key="object_id" value="2"/><metadata key="instance_id" value="0"/></model_instance>
 </plate>
 <plate><metadata key="plater_id" value="2"/><metadata key="plater_name" value="Lid"/>
  <model_instance><metadata key="object_id" value="3"/><metadata key="instance_id" value="0"/></model_instance>
  <model_instance><metadata key="object_id" value="4"/><metadata key="instance_id" value="0"/></model_instance>
 </plate>
 <object id="1"><metadata key="name" value="Shell"/><metadata key="extruder" value="1"/></object>
 <object id="2"><metadata key="name" value="Badge"/><metadata key="extruder" value="3"/></object>
 <object id="3"><metadata key="name" value="Top"/><metadata key="extruder" value="2"/></object>
 <object id="4"><metadata key="name" value="Button"/><metadata key="extruder" value="3"/></object>
</config>"#
}

fn project() -> &'static str {
    r##"{"filament_settings_id":["PLA White","PLA Charcoal","PLA Orange"],"filament_type":["PLA","PLA","PLA"],"filament_vendor":["Fixture","Fixture","Fixture"],"filament_colour":["#F8F8F8","#1F2937","#FF6B35"]}"##
}

fn package() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (path, contents) in [
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", ROOT_RELS),
        ("3D/3dmodel.model", &model()),
        ("Metadata/model_settings.config", metadata()),
        ("Metadata/project_settings.config", project()),
    ] {
        writer.start_file(path, options)?;
        writer.write_all(contents.as_bytes())?;
    }
    Ok(writer.finish()?.into_inner())
}

fn run(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = package()?;
    let parsed = parse_model_with_plates(&bytes)?;
    let triangles = parsed.model.triangle_count;
    let covered: u32 = parsed.parts.iter().map(|part| part.triangle_count).sum();
    assert_eq!(parsed.plates.len(), 2, "fixture must contain two plates");
    assert_eq!(
        parsed
            .plates
            .iter()
            .map(|plate| plate.name.as_str())
            .collect::<Vec<_>>(),
        ["Housing", "Lid"]
    );
    assert_eq!(
        parsed
            .plates
            .iter()
            .map(|plate| plate.triangle_count)
            .collect::<Vec<_>>(),
        [2, 2]
    );
    assert_eq!(
        parsed.parts.len(),
        4,
        "fixture must contain four authored parts"
    );
    assert_eq!(
        parsed
            .parts
            .iter()
            .map(|part| part.name.as_str())
            .collect::<Vec<_>>(),
        ["Shell", "Badge", "Top", "Button"]
    );
    assert_eq!(
        parsed
            .parts
            .iter()
            .map(|part| part.triangle_start)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    assert_eq!(
        parsed.embedded_filaments.len(),
        3,
        "fixture must contain three filament slots"
    );
    assert_eq!(
        parsed
            .embedded_filaments
            .iter()
            .map(|filament| filament.color.as_str())
            .collect::<Vec<_>>(),
        ["#F8F8F8", "#1F2937", "#FF6B35"]
    );
    assert_eq!(
        covered, triangles,
        "authored parts must cover every triangle"
    );
    assert_eq!(triangles, 4, "fixture geometry changed unexpectedly");
    fs::write(path, bytes)?;
    Ok(())
}

fn main() -> ExitCode {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| "colorway_fixture".into());
    let Some(path) = args.next() else {
        eprintln!("usage: {program} OUTPUT.3mf");
        return ExitCode::from(2);
    };
    if args.next().is_some() {
        eprintln!("usage: {program} OUTPUT.3mf");
        return ExitCode::from(2);
    }
    match run(&path) {
        Ok(()) => {
            println!("wrote {path}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to create {path}: {error}");
            ExitCode::from(1)
        }
    }
}
