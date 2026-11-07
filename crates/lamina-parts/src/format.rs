//! The EasyEDA PCB document as a [`BoardFormat`].

use std::collections::BTreeMap;
use std::path::Path;

use lamina_model::ProjectData;
use lamina_ports::format::{BoardFormat, FormatInfo, ImportedBoard};

use crate::provider;

pub const ID: &str = "easyeda";

/// Reads the PCB JSON that the standard EasyEDA editor saves with
/// File > Export > EasyEDA.
#[derive(Debug, Default)]
pub struct EasyedaFormat;

impl BoardFormat for EasyedaFormat {
    fn info(&self) -> FormatInfo {
        FormatInfo {
            id: ID.into(),
            name: "EasyEDA board".into(),
            description: "The PCB JSON file of the standard EasyEDA editor (File > \
                          Export > EasyEDA): outline, parts with their LCSC numbers, \
                          nets, placement, tracks, vias, copper areas, holes and texts. \
                          The 3D models come from the `lcsc` provider."
                .into(),
            extensions: vec!["json".into()],
            read: true,
            write: false,
        }
    }

    /// A `.json` file that is not in a Lamina project folder.
    fn detect(&self, path: &Path) -> bool {
        path.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
            && !path.parent().is_some_and(ProjectData::is_project)
    }

    fn read(&self, path: &Path, name: &str) -> Result<ImportedBoard, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let imported = crate::board::import_board(&doc, name)?;
        let models = imported
            .models
            .iter()
            .map(|(fp, info)| {
                (
                    fp.clone(),
                    vec![format!("{}:{}", provider::ID, provider::model_id(info))],
                )
            })
            .collect();
        Ok(ImportedBoard {
            data: imported.data,
            models,
            extras: BTreeMap::new(),
            notes: imported.notes,
        })
    }
}
