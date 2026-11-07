//! Open a board file from another program. Lamina imports the file into a
//! Lamina project folder through the board format that detects it.

use std::path::{Path, PathBuf};

use lamina_model::error::Result;
use lamina_model::*;
use lamina_ports::Ports;

use crate::ops::ImportBoard;
use crate::project::{OpResult, Project};

fn ext(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// The id of the format that reads `p`, if one of the standard formats does.
pub fn board_file_kind(p: &Path) -> Option<String> {
    board_file_kind_in(&crate::wiring::standard(None), p)
}

/// Like [`board_file_kind`], with the given ports.
pub fn board_file_kind_in(ports: &Ports, p: &Path) -> Option<String> {
    ports.format_for(p).map(|f| f.info().id)
}

/// The path that holds the whole board for `p`: the file, or the folder of
/// a file that is one of a set.
fn source_root(ports: &Ports, p: &Path) -> PathBuf {
    ports
        .format_for(p)
        .map(|f| f.source_root(p))
        .unwrap_or_else(|| p.to_path_buf())
}

/// 64-bit FNV-1a. It gives the same result in all builds, but the std hasher does not.
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join("lamina")
}

/// The project folder that an opened board file is imported into. There is
/// one folder for each source path, in the local data folder of the user.
pub fn import_dir_for(file: &Path) -> PathBuf {
    import_dir_in(&data_dir().join("imports"), file)
}

/// Like [`import_dir_for`], but in `base`.
pub fn import_dir_in(base: &Path, file: &Path) -> PathBuf {
    let abs = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
    let src = source_root(&crate::wiring::standard(None), &abs);
    let stem: String = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("board")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let key = src.to_string_lossy().to_lowercase();
    base.join(format!("{stem}-{:08x}", fnv(&key) as u32))
}

fn modified(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The time of the last change to a source. For a file, this is the time of
/// the file. For a folder, it is the time of the newest file in it, because
/// the time of a folder does not change when a file in it is edited in place.
fn source_modified(ports: &Ports, p: &Path) -> Option<std::time::SystemTime> {
    let root = source_root(ports, p);
    if root.is_dir() {
        std::fs::read_dir(&root)
            .ok()?
            .flatten()
            .filter_map(|e| modified(&e.path()))
            .max()
    } else {
        modified(&root)
    }
}

/// Import `file` into `dir`. This replaces a board that is already there.
pub fn import_board_file(file: &Path, dir: &Path) -> Result<OpResult> {
    let ports = crate::wiring::standard(Some(dir));
    if board_file_kind_in(&ports, file).is_none() {
        let ids: Vec<String> = ports.formats.iter().map(|f| f.info().id).collect();
        return Err(Error::invalid(format!(
            "{} is not a board file that Lamina can open. The formats are: {}.",
            file.display(),
            ids.join(", ")
        )));
    }
    let name = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("board")
        .to_string();
    let mut p = if ProjectData::is_project(dir) {
        Project::open_with(dir, ports)?
    } else {
        Project::create_with(dir, &name, ports)?
    };
    p.import_board(ImportBoard {
        path: file.to_string_lossy().into_owned(),
        format: None,
        models: true,
        replace: true,
        strict: false,
    })
}

/// The Lamina project for `path`. If `path` is a project folder, this is the
/// folder. If not, it is the import folder of the board file. Lamina imports
/// the file again if the source is newer than the import. Returns the folder,
/// and the import result if an import ran.
pub fn open_board_path(path: &Path) -> Result<(PathBuf, Option<OpResult>)> {
    open_board_path_in(&data_dir().join("imports"), path)
}

/// Like [`open_board_path`], but with the imports in `base`.
pub fn open_board_path_in(base: &Path, path: &Path) -> Result<(PathBuf, Option<OpResult>)> {
    if ProjectData::is_project(path) {
        return Ok((path.to_path_buf(), None));
    }
    if let Some(dir) = path.parent().filter(|d| ProjectData::is_project(d)) {
        if ext(path) == "json" {
            return Ok((dir.to_path_buf(), None));
        }
    }
    let dir = import_dir_in(base, path);
    let stamp = dir.join(".imported");
    let ports = crate::wiring::standard(None);
    let fresh = match (source_modified(&ports, path), modified(&stamp)) {
        (Some(src), Some(done)) => done >= src && ProjectData::is_project(&dir),
        _ => false,
    };
    if fresh {
        return Ok((dir, None));
    }
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    let r = import_board_file(path, &dir)?;
    let _ = std::fs::write(&stamp, path.display().to_string());
    Ok((dir, Some(r)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_import_dirs() {
        let t = tempfile::tempdir().unwrap();
        let k = t.path().join("b.kicad_pcb");
        std::fs::write(&k, "").unwrap();
        assert_eq!(board_file_kind(&k).as_deref(), Some("kicad"));
        let g = t.path().join("gerbers");
        std::fs::create_dir_all(&g).unwrap();
        assert_eq!(board_file_kind(&g), None);
        std::fs::write(g.join("b.GTL"), "").unwrap();
        assert_eq!(board_file_kind(&g).as_deref(), Some("gerber"));
        assert_eq!(board_file_kind(&g.join("b.GTL")).as_deref(), Some("gerber"));
        // A single Gerber file uses the import of its folder.
        assert_eq!(import_dir_for(&g.join("b.GTL")), import_dir_for(&g));
        assert_ne!(import_dir_for(&k), import_dir_for(&g));
        assert_eq!(board_file_kind(&t.path().join("x.txt")), None);
        assert_eq!(fnv("a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn opens_a_board_file_once_and_again_when_it_changes() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("b.kicad_pcb");
        std::fs::write(
            &file,
            r#"(kicad_pcb (version 20240108) (generator "t")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
  (gr_rect (start 0 0) (end 20 10) (layer "Edge.Cuts") (width 0.1)))"#,
        )
        .unwrap();
        let base = t.path().join("imports");
        let (dir, first) = open_board_path_in(&base, &file).unwrap();
        assert!(first.is_some());
        assert!(dir.starts_with(&base));
        assert!(ProjectData::is_project(&dir));
        // If the source did not change, the import is used again.
        let (again, second) = open_board_path_in(&base, &file).unwrap();
        assert_eq!(again, dir);
        assert!(second.is_none());
        // If the source is newer, Lamina imports it again.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(open_board_path_in(&base, &file).unwrap().1.is_some());
        // A project folder opens directly.
        let (same, ran) = open_board_path_in(&base, &dir).unwrap();
        assert_eq!(same, dir);
        assert!(ran.is_none());
        // Other files are refused with a clear message.
        let txt = t.path().join("notes.txt");
        std::fs::write(&txt, "x").unwrap();
        let e = open_board_path_in(&base, &txt).unwrap_err().to_string();
        assert!(e.contains("not a board file"), "{e}");
    }
}
