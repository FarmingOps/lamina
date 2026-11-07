//! The part library: folders that keep fetched parts and models, so that a
//! board can import them without a network.
//!
//! A library root has this layout:
//!
//! - `parts/<provider>/<id>.json`: a [`crate::provider::Record`].
//! - `models/<provider>/<file>`: the model files, in the same relative form
//!   as `cache/models/` of a board.
//!
//! The roots are, in the search order: the `lib` folders of the workspace and
//! then the user library. Lamina writes new downloads to the user library only.
//! The project cache stays complete on its own, so a clone of the board needs
//! no library.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::provider::Record;
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

    /// Writes `rel` in the store.
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

    /// The `models` folders that exist.
    pub fn model_dirs(&self) -> Vec<PathBuf> {
        self.roots
            .iter()
            .map(|r| r.join("models"))
            .filter(|p| p.is_dir())
            .collect()
    }

    /// All records in all roots. A part that is in two roots is listed once,
    /// from the first root. With a provider id, only that provider.
    pub fn records(&self, provider: Option<&str>) -> Vec<Record> {
        let mut out: Vec<Record> = Vec::new();
        for root in &self.roots {
            let Ok(providers) = std::fs::read_dir(root.join("parts")) else {
                continue;
            };
            for pd in providers.flatten() {
                let pname = pd.file_name().to_string_lossy().to_string();
                if provider.is_some_and(|p| p != pname) {
                    continue;
                }
                let Ok(files) = std::fs::read_dir(pd.path()) else {
                    continue;
                };
                for f in files.flatten() {
                    let p = f.path();
                    if !p.extension().is_some_and(|x| x == "json") {
                        continue;
                    }
                    let Ok(text) = std::fs::read_to_string(&p) else {
                        continue;
                    };
                    let Ok(r) = serde_json::from_str::<Record>(&text) else {
                        continue;
                    };
                    if !out.iter().any(|o| o.provider == r.provider && o.id == r.id) {
                        out.push(r);
                    }
                }
            }
        }
        out.sort_by(|a, b| (&a.provider, &a.id).cmp(&(&b.provider, &b.id)));
        out
    }
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
        assert!(lib.find("parts/x/C1.json").is_none());
        let r = Record {
            provider: "x".into(),
            id: "C1".into(),
            footprint: crate::footprint::Footprint::new(
                "F",
                crate::footprint::Provenance::Manual { note: "t".into() },
            ),
            info: Default::default(),
            notes: vec![],
        };
        lib.put(
            "parts/x/C1.json",
            serde_json::to_string(&r).unwrap().as_bytes(),
        )
        .unwrap();
        assert!(lib.find("parts/x/C1.json").is_some());
        assert_eq!(lib.records(None).len(), 1);
        assert_eq!(lib.records(Some("y")).len(), 0);
        assert_eq!(lib.records(Some("x"))[0].id, "C1");
    }
}
