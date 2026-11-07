//! Board file formats: read a board from another program, write one for it.

use std::collections::BTreeMap;
use std::path::Path;

use lamina_model::ProjectData;
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FormatInfo {
    /// The short id, for example `kicad`.
    pub id: String,
    pub name: String,
    /// What the format holds and where it comes from.
    pub description: String,
    /// File extensions without the dot, lower case.
    pub extensions: Vec<String>,
    pub read: bool,
    pub write: bool,
}

/// A board as a format reads it. The data is a complete Lamina board. The
/// models are what the footprints need from a provider, as `provider:id`,
/// so that the core can get them through the provider port.
#[derive(Debug)]
pub struct ImportedBoard {
    pub data: ProjectData,
    /// Footprint id to the model to get, as `provider:id`. The format can
    /// put more than one candidate; the core takes the first that works.
    pub models: BTreeMap<String, Vec<String>>,
    /// Files to keep next to the board, by name: artwork that the format
    /// read but did not convert (for example the Gerber layers).
    pub extras: BTreeMap<String, Vec<u8>>,
    pub notes: Vec<String>,
}

pub trait BoardFormat: Send + Sync {
    fn info(&self) -> FormatInfo;
    /// True if this format can read `path` (a file or a folder).
    fn detect(&self, path: &Path) -> bool;
    /// Read the board at `path`. `name` is the board name to use.
    /// `project_dir` is the folder that holds `path`, for files next to it.
    fn read(&self, path: &Path, name: &str) -> Result<ImportedBoard, String>;
    /// Write the board as text. `model_dir` holds the model files of the
    /// board, for formats that reference them.
    fn write(&self, data: &ProjectData, model_dir: &Path) -> Result<String, String> {
        let _ = (data, model_dir);
        Err(format!("The format `{}` cannot write.", self.info().id))
    }
}
