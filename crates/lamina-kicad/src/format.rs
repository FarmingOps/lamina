//! The `.kicad_pcb` board file as a [`BoardFormat`].

use std::collections::BTreeMap;
use std::path::Path;

use lamina_model::ProjectData;
use lamina_ports::format::{BoardFormat, FormatInfo, ImportedBoard};

use crate::library;
use crate::provider;

pub const ID: &str = "kicad";

/// Reads `.kicad_pcb` boards with the `.kicad_pro` next to them, and writes
/// boards that KiCad 8 or a newer version opens.
#[derive(Debug, Default)]
pub struct KicadFormat;

/// True if a model path points into the KiCad project folder: `${KIPRJMOD}`,
/// or a relative path with no variable.
fn in_project(path: &str) -> bool {
    path.starts_with("${KIPRJMOD}") || (!path.starts_with("${") && !Path::new(path).is_absolute())
}

impl BoardFormat for KicadFormat {
    fn info(&self) -> FormatInfo {
        FormatInfo {
            id: ID.into(),
            name: "KiCad board".into(),
            description: "A `.kicad_pcb` file from KiCad 6 to 10: outline, footprints, \
                          parts, nets, placement, tracks, vias, zones, keep-outs and \
                          texts. The rules and the net classes come from the \
                          `.kicad_pro` next to it. Lamina writes files for KiCad 8 or \
                          a newer version."
                .into(),
            extensions: vec!["kicad_pcb".into()],
            read: true,
            write: true,
        }
    }

    fn detect(&self, path: &Path) -> bool {
        path.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("kicad_pcb"))
    }

    fn read(&self, path: &Path, name: &str) -> Result<ImportedBoard, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let imported = crate::import_board(&text, name)?;
        let mut notes = imported.notes;
        let mut data = imported.data;
        // KiCad 6+ keeps the rules and the net classes in the project file.
        let pro = path.with_extension("kicad_pro");
        if let Ok(text) = std::fs::read_to_string(&pro) {
            match crate::apply_project(&mut data, &text) {
                Ok(n) => notes.extend(n),
                Err(e) => notes.push(format!("{}: {e}", pro.display())),
            }
        }
        let project_dir = path.parent();
        let mut models = BTreeMap::new();
        for (fp_id, list) in &imported.models {
            let mut wanted = Vec::new();
            for m in list {
                let spec = if in_project(&m.path) {
                    // The file is next to the board. Give its full path.
                    let roots: Vec<_> = project_dir.map(Path::to_path_buf).into_iter().collect();
                    let file = library::resolve_model(&roots, &m.path).unwrap_or_else(|| {
                        let rel = library::model_relative(&m.path);
                        match project_dir {
                            Some(d) => d.join(rel),
                            None => rel.into(),
                        }
                    });
                    file.display().to_string()
                } else {
                    match library::model_lib_name(&m.path) {
                        Some((lib, name)) => format!("{lib}:{name}"),
                        None => {
                            notes.push(format!(
                                "{fp_id}: `{}` is not a model path. It is skipped.",
                                m.path
                            ));
                            continue;
                        }
                    }
                };
                wanted.push(format!(
                    "{}:{}",
                    provider::ID,
                    provider::model_id(&spec, m.offset, m.rotation, m.scale)
                ));
            }
            if !wanted.is_empty() {
                models.insert(fp_id.clone(), wanted);
            }
        }
        Ok(ImportedBoard {
            data,
            models,
            extras: BTreeMap::new(),
            notes,
        })
    }

    fn write(&self, data: &ProjectData, model_dir: &Path) -> Result<String, String> {
        Ok(crate::export_board_with(data, Some(model_dir)))
    }
}
