//! A workspace groups several boards with shared libraries.
//!
//! `lamina.json` is above the board folders. Each `lib` folder is a
//! Jsonnet import path and can hold shared `footprints/` and `models/`.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::files::SCHEMA;

pub const WORKSPACE_FILE: &str = "lamina.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceFile {
    #[serde(default = "schema_v")]
    pub schema: u32,
    pub name: String,
    /// Board folders, relative to the workspace root.
    #[serde(default)]
    pub boards: Vec<String>,
    /// Library folders, relative to the workspace root.
    #[serde(default = "default_lib")]
    pub lib: Vec<String>,
    /// Folders that hold KiCad `*.pretty` footprint libraries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kicad_footprints: Vec<String>,
    /// Folders that hold KiCad `*.3dshapes` model libraries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kicad_models: Vec<String>,
}

fn schema_v() -> u32 {
    SCHEMA
}

fn default_lib() -> Vec<String> {
    vec!["lib".into()]
}

#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    pub root: PathBuf,
    pub file: WorkspaceFile,
}

impl Workspace {
    /// The workspace that contains `dir`. The search goes up through the parent folders.
    pub fn find(dir: &Path) -> Option<Workspace> {
        // Make the path absolute first: a relative `.` has no parents to go up to.
        let mut cur = Some(std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()));
        for _ in 0..12 {
            let d = cur?;
            let f = d.join(WORKSPACE_FILE);
            if f.is_file() {
                return Workspace::open(&d).ok();
            }
            cur = d.parent().map(Path::to_path_buf);
        }
        None
    }

    pub fn open(root: &Path) -> Result<Workspace> {
        let path = root.join(WORKSPACE_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let file: WorkspaceFile = serde_json::from_str(&text).map_err(|e| Error::Json {
            path: path.clone(),
            source: e,
        })?;
        if file.schema > SCHEMA {
            return Err(Error::Invalid(format!(
                "{} uses schema {}. This build supports schema {SCHEMA} or lower. Update Lamina.",
                path.display(),
                file.schema
            )));
        }
        Ok(Workspace {
            root: root.to_path_buf(),
            file,
        })
    }

    pub fn create(root: &Path, name: &str) -> Result<Workspace> {
        let ws = Workspace {
            root: root.to_path_buf(),
            file: WorkspaceFile {
                schema: SCHEMA,
                name: name.into(),
                boards: Vec::new(),
                lib: default_lib(),
                kicad_footprints: Vec::new(),
                kicad_models: Vec::new(),
            },
        };
        ws.save()?;
        for l in ws.lib_dirs() {
            std::fs::create_dir_all(&l).map_err(|e| Error::io(&l, e))?;
        }
        Ok(ws)
    }

    pub fn save(&self) -> Result<()> {
        let path = self.root.join(WORKSPACE_FILE);
        let text = crate::json::to_string(&self.file).map_err(|e| Error::Json {
            path: path.clone(),
            source: e,
        })?;
        crate::project::write_atomic(&path, text.as_bytes())
    }

    /// Adds a board folder. The relative path keeps forward slashes.
    pub fn add_board(&mut self, dir: &Path) -> Result<()> {
        let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let root = abs(&self.root);
        let rel = abs(dir)
            .strip_prefix(&root)
            .map_err(|_| {
                Error::Invalid(format!(
                    "{} is outside the workspace {}. Use a board folder in the workspace.",
                    dir.display(),
                    self.root.display()
                ))
            })?
            .to_string_lossy()
            .replace('\\', "/");
        if !self.file.boards.contains(&rel) {
            self.file.boards.push(rel);
            self.save()?;
        }
        Ok(())
    }

    pub fn board_dirs(&self) -> Vec<PathBuf> {
        self.file.boards.iter().map(|b| self.root.join(b)).collect()
    }

    pub fn lib_dirs(&self) -> Vec<PathBuf> {
        self.file.lib.iter().map(|l| self.root.join(l)).collect()
    }

    fn resolve(&self, rel: &str) -> PathBuf {
        let p = Path::new(rel);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        }
    }

    /// The roots of the KiCad footprint libraries in `lamina.json`.
    pub fn kicad_footprint_dirs(&self) -> Vec<PathBuf> {
        self.file
            .kicad_footprints
            .iter()
            .map(|d| self.resolve(d))
            .collect()
    }

    /// The roots of the KiCad 3D model libraries in `lamina.json`.
    pub fn kicad_model_dirs(&self) -> Vec<PathBuf> {
        self.file
            .kicad_models
            .iter()
            .map(|d| self.resolve(d))
            .collect()
    }

    /// Shared footprint folders, in the search order.
    pub fn footprint_dirs(&self) -> Vec<PathBuf> {
        self.lib_dirs()
            .into_iter()
            .map(|l| l.join("footprints"))
            .filter(|p| p.is_dir())
            .collect()
    }

    /// Shared model folders, in the search order.
    pub fn model_dirs(&self) -> Vec<PathBuf> {
        self.lib_dirs()
            .into_iter()
            .map(|l| l.join("models"))
            .filter(|p| p.is_dir())
            .collect()
    }
}

/// The folders that can hold the footprints of a board: the cache of the board
/// first, then the workspace libraries.
pub fn footprint_dirs(board: &Path) -> Vec<PathBuf> {
    let mut out = vec![board.join(crate::project::FOOTPRINT_DIR)];
    if let Some(ws) = Workspace::find(board) {
        out.extend(ws.footprint_dirs());
    }
    out
}

/// The folders that hold saved block layouts: the `layouts/` of the board first,
/// then `layouts/` in each workspace library. The list includes folders that
/// do not exist, so the first entry from a workspace library is where new layouts go.
pub fn layout_dirs(board: &Path) -> Vec<PathBuf> {
    let mut out = vec![board.join(LAYOUT_DIR)];
    if let Some(ws) = Workspace::find(board) {
        out.extend(ws.lib_dirs().into_iter().map(|l| l.join(LAYOUT_DIR)));
    }
    out
}

/// The folder name for saved block layouts.
pub const LAYOUT_DIR: &str = "layouts";

/// Finds a model file for a board: the cache of the board first, then the
/// workspace libraries.
pub fn find_model(board: &Path, rel: &str) -> Option<PathBuf> {
    let own = board.join(crate::project::MODEL_DIR).join(rel);
    if own.is_file() {
        return Some(own);
    }
    Workspace::find(board)?
        .model_dirs()
        .into_iter()
        .map(|d| d.join(rel))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_walks_up_and_lists_dirs() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("ws");
        std::fs::create_dir_all(root.join("boards/a")).unwrap();
        let mut ws = Workspace::create(&root, "demo").unwrap();
        ws.add_board(&root.join("boards/a")).unwrap();
        ws.add_board(&root.join("boards/a")).unwrap();
        let found = Workspace::find(&root.join("boards/a")).unwrap();
        assert_eq!(found.file.boards, vec!["boards/a".to_string()]);
        assert_eq!(found.lib_dirs(), vec![root.join("lib")]);
        assert!(found.footprint_dirs().is_empty());
        std::fs::create_dir_all(root.join("lib/footprints")).unwrap();
        assert_eq!(
            footprint_dirs(&root.join("boards/a")),
            vec![
                root.join("boards/a").join(crate::project::FOOTPRINT_DIR),
                root.join("lib/footprints")
            ]
        );
        assert!(Workspace::find(t.path()).is_none());
        assert!(ws.add_board(t.path()).is_err());
    }

    #[test]
    fn find_from_a_relative_path() {
        // A relative path cannot cross Windows drives. Keep the fixture on the
        // drive of the current directory, not in the system temp folder.
        let cwd = std::env::current_dir().unwrap();
        let t = tempfile::tempdir_in(&cwd).unwrap();
        let root = t.path().join("ws");
        let board = root.join("boards/a");
        std::fs::create_dir_all(&board).unwrap();
        Workspace::create(&root, "demo").unwrap();
        // `.` in the board, written relative to the current directory.
        let rel: PathBuf = cwd
            .components()
            .skip(1)
            .map(|_| Path::new(".."))
            .collect::<PathBuf>()
            .join(
                board
                    .strip_prefix(board.ancestors().last().unwrap())
                    .unwrap(),
            )
            .join(".");
        let mut ws = Workspace::find(&rel).expect("found through a relative path");
        assert_eq!(ws.lib_dirs().len(), 1);
        ws.add_board(&rel).unwrap();
        assert_eq!(ws.file.boards, vec!["boards/a".to_string()]);
    }
}
