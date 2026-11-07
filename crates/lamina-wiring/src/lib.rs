//! The composition root of Lamina. This crate is the one place that names
//! the adapters. It plugs them into a [`Ports`] bundle and opens projects
//! with it. The binaries (`lamina-mcp`, `lamina-viewer`) depend on this
//! crate. `lamina-core` does not: it sees the ports only.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lamina_core::{OpResult, Project, Result};
use lamina_model::Workspace;
use lamina_ports::Ports;

/// The standard ports for a board in `board` (or none). The local footprint
/// and model folders come from `lamina.json` of the workspace that holds it.
pub fn standard(board: Option<&Path>) -> Ports {
    let ws = board.and_then(Workspace::find);
    let fp_dirs = ws
        .as_ref()
        .map(|w| w.kicad_footprint_dirs())
        .unwrap_or_default();
    let m_dirs = ws
        .as_ref()
        .map(|w| w.kicad_model_dirs())
        .unwrap_or_default();
    Ports {
        providers: vec![
            Arc::new(lamina_parts::provider::LcscProvider::online()),
            Arc::new(lamina_kicad::provider::KicadProvider::online(
                fp_dirs, m_dirs,
            )),
        ],
        formats: vec![
            Arc::new(lamina_kicad::KicadFormat),
            Arc::new(lamina_parts::EasyedaFormat),
            Arc::new(lamina_export::GerberFormat),
        ],
        fabricators: vec![Arc::new(lamina_export::GerberFabricator)],
        meshes: Some(Arc::new(lamina_step::FileMeshLoader)),
        step: Some(Arc::new(lamina_export::StepExporter)),
        renderer: Some(Arc::new(lamina_render::PngRenderer)),
        source: Some(Arc::new(lamina_build::JsonnetSource)),
        routers: vec![Arc::new(lamina_specctra::SpecctraExchange)],
    }
}

/// Create a project with the standard ports.
pub fn create(dir: &Path, name: &str) -> Result<Project> {
    Project::create_with(dir, name, standard(Some(dir)))
}

/// Open a project with the standard ports and its lock.
pub fn open(dir: &Path) -> Result<Project> {
    Project::open_with(dir, standard(Some(dir)))
}

/// Open a project with the standard ports, without the lock.
pub fn open_shared(dir: &Path) -> Result<Project> {
    Project::open_shared_with(dir, standard(Some(dir)))
}

/// The id of the standard format that reads `p`, if one does.
pub fn board_file_kind(p: &Path) -> Option<String> {
    lamina_core::board_file_kind_in(&standard(None), p)
}

/// The import folder of a board file, in the data folder of the user.
pub fn import_dir_for(file: &Path) -> PathBuf {
    lamina_core::import_dir_in(&standard(None), &lamina_core::imports_dir(), file)
}

/// Import a board file into `dir` with the standard ports.
pub fn import_board_file(file: &Path, dir: &Path) -> Result<OpResult> {
    lamina_core::import_board_file(standard(Some(dir)), file, dir)
}

/// The Lamina project for a path: the folder itself, or the import of the
/// board file at that path.
pub fn open_board_path(path: &Path) -> Result<(PathBuf, Option<OpResult>)> {
    lamina_core::open_board_path_in(&standard(None), &lamina_core::imports_dir(), path)
}
