//! The part library: folders that keep downloaded parts, models and KiCad
//! files, so that a board can import them without a network.
//!
//! A library root has this layout:
//!
//! - `parts/<LCSC>.json`: the EasyEDA component record.
//! - `models/lcsc/<uuid>.obj` and `.step`: EasyEDA models.
//! - `models/<other>/...`: any model file, in the same relative form as
//!   `cache/models/` of a board.
//! - `kicad/footprints/<Library>.pretty/<Name>.kicad_mod`: KiCad footprints.
//! - `kicad/models/<Library>.3dshapes/<Name>.step`: KiCad models.
//!
//! The roots are, in the search order: the `lib` folders of the workspace and
//! then the user library. Lamina writes new downloads to the user library only.
//! The project cache stays complete on its own, so a clone of the board needs
//! no library.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::workspace::Workspace;

/// The environment variable that moves the user library.
pub const USER_LIB_ENV: &str = "LAMINA_LIB";

/// The user library folder: `$LAMINA_LIB`, or `%LOCALAPPDATA%\lamina\lib` on
/// Windows, or `~/.lamina/lib`.
pub fn user_lib() -> PathBuf {
    if let Some(p) = std::env::var_os(USER_LIB_ENV).filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    if cfg!(windows) {
        if let Some(p) = std::env::var_os("LOCALAPPDATA").filter(|p| !p.is_empty()) {
            return PathBuf::from(p).join("lamina").join("lib");
        }
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".lamina").join("lib")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Library {
    /// The roots, in the search order.
    pub roots: Vec<PathBuf>,
    /// The root that gets new downloads.
    pub store: PathBuf,
}

impl Library {
    /// The user library alone.
    pub fn user() -> Library {
        let store = user_lib();
        Library {
            roots: vec![store.clone()],
            store,
        }
    }

    /// The library of a board: the workspace `lib` folders, then the user library.
    pub fn for_board(board: &Path) -> Library {
        let mut lib = Library::user();
        if let Some(ws) = Workspace::find(board) {
            let mut roots = ws.lib_dirs();
            roots.push(lib.store.clone());
            lib.roots = roots;
        }
        lib
    }

    /// A library with one folder, for tests and offline mirrors.
    pub fn at(root: &Path) -> Library {
        Library {
            roots: vec![root.to_path_buf()],
            store: root.to_path_buf(),
        }
    }

    /// The first root that has the file `rel`.
    pub fn find(&self, rel: &str) -> Option<PathBuf> {
        self.roots.iter().map(|r| r.join(rel)).find(|p| p.is_file())
    }

    /// Writes `rel` in the store. A failure is not an error for the caller:
    /// the board still has its own copy.
    pub fn put(&self, rel: &str, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.store.join(rel);
        crate::project::write_atomic(&path, bytes)?;
        Ok(path)
    }

    /// Copies a file into the store, if it is not there already.
    pub fn put_file(&self, rel: &str, src: &Path) -> Result<PathBuf> {
        let path = self.store.join(rel);
        if path.is_file() {
            return Ok(path);
        }
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        std::fs::copy(src, &path).map_err(|e| Error::io(src, e))?;
        Ok(path)
    }

    /// The `kicad/footprints` folders that exist.
    pub fn kicad_footprint_dirs(&self) -> Vec<PathBuf> {
        self.sub_dirs("kicad/footprints")
    }

    /// The `kicad/models` folders that exist.
    pub fn kicad_model_dirs(&self) -> Vec<PathBuf> {
        self.sub_dirs("kicad/models")
    }

    /// The `models` folders that exist.
    pub fn model_dirs(&self) -> Vec<PathBuf> {
        self.sub_dirs("models")
    }

    /// The `parts/*.json` files in all roots. A part that is in two roots is
    /// listed once, from the first root.
    pub fn part_files(&self) -> Vec<(String, PathBuf)> {
        let mut out: Vec<(String, PathBuf)> = Vec::new();
        for root in &self.roots {
            let Ok(rd) = std::fs::read_dir(root.join("parts")) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if p.extension().is_some_and(|x| x == "json") && !out.iter().any(|(s, _)| s == stem)
                {
                    out.push((stem.to_string(), p));
                }
            }
        }
        out.sort();
        out
    }

    fn sub_dirs(&self, rel: &str) -> Vec<PathBuf> {
        self.roots
            .iter()
            .map(|r| r.join(rel))
            .filter(|p| p.is_dir())
            .collect()
    }
}

/// The relative file name of an EasyEDA component record.
pub fn part_rel(lcsc: &str) -> String {
    format!("parts/{lcsc}.json")
}

/// The relative file name of a KiCad footprint.
pub fn kicad_footprint_rel(library: &str, name: &str) -> String {
    format!("kicad/footprints/{library}.pretty/{name}.kicad_mod")
}

/// The relative file name of a KiCad model.
pub fn kicad_model_rel(library: &str, name: &str) -> String {
    format!("kicad/models/{library}.3dshapes/{name}.step")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_find_and_store() {
        let t = tempfile::tempdir().unwrap();
        let ws = t.path().join("ws");
        std::fs::create_dir_all(ws.join("boards/a")).unwrap();
        let mut w = Workspace::create(&ws, "demo").unwrap();
        w.add_board(&ws.join("boards/a")).unwrap();
        let lib = Library::for_board(&ws.join("boards/a"));
        assert_eq!(lib.roots.len(), 2);
        assert_eq!(lib.roots[0], ws.join("lib"));
        assert_eq!(lib.roots[1], lib.store);

        let lib = Library::at(&t.path().join("lib"));
        assert!(lib.find("parts/C1.json").is_none());
        lib.put("parts/C1.json", b"{}").unwrap();
        assert!(lib.find("parts/C1.json").is_some());
        assert_eq!(lib.part_files().len(), 1);
        assert_eq!(lib.part_files()[0].0, "C1");
        assert!(lib.kicad_footprint_dirs().is_empty());
        lib.put(
            &kicad_footprint_rel("LED_SMD", "LED_0603_1608Metric"),
            b"(footprint)",
        )
        .unwrap();
        assert_eq!(lib.kicad_footprint_dirs().len(), 1);
    }
}
