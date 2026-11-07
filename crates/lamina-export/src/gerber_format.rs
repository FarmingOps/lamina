//! Gerber and drill files as a [`BoardFormat`] that reads artwork.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lamina_ports::format::{BoardFormat, FormatInfo, ImportedBoard};

use crate::fab_import;

pub const ID: &str = "gerber";

/// The extensions of Gerber and drill files, lower case.
const GERBER_EXT: &[&str] = &[
    "gbr", "ger", "gtl", "gbl", "gto", "gbo", "gts", "gbs", "gtp", "gbp", "gko", "gm1", "drl",
    "xln", "exc", "art", "pho",
];

fn ext(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn is_gerber_file(p: &Path) -> bool {
    GERBER_EXT.contains(&ext(p).as_str())
}

/// Reads a folder, a zip or one file of Gerber and Excellon files as artwork:
/// the outline, the holes, and the copper and silkscreen as drawings.
#[derive(Debug, Default)]
pub struct GerberFormat;

impl BoardFormat for GerberFormat {
    fn info(&self) -> FormatInfo {
        let mut extensions: Vec<String> = GERBER_EXT.iter().map(|e| e.to_string()).collect();
        extensions.push("zip".into());
        FormatInfo {
            id: ID.into(),
            name: "Gerber files".into(),
            description: "Gerber and Excellon files from any CAD program, as a folder, \
                          a zip or one file. The board gets the outline, the holes, and \
                          the copper and silkscreen as drawings. It has no parts and no \
                          nets."
                .into(),
            extensions,
            read: true,
            write: false,
        }
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_dir() {
            return std::fs::read_dir(path)
                .ok()
                .is_some_and(|rd| rd.flatten().any(|e| is_gerber_file(&e.path())));
        }
        path.is_file() && (ext(path) == "zip" || is_gerber_file(path))
    }

    /// One Gerber file stands for its folder.
    fn source_root(&self, path: &Path) -> PathBuf {
        if path.is_file() && is_gerber_file(path) {
            if let Some(d) = path.parent() {
                return d.to_path_buf();
            }
        }
        path.to_path_buf()
    }

    fn read(&self, path: &Path, name: &str) -> Result<ImportedBoard, String> {
        let root = self.source_root(path);
        let files = fab_import::read_files(&root)?;
        let name = name.trim_end_matches("-gerbers");
        let b = fab_import::import(&files, name)?;
        let layers: Vec<String> = b.layers.iter().map(|(f, l)| format!("{f}: {l}")).collect();
        let mut extras = BTreeMap::new();
        extras.insert(
            "layers.txt".to_string(),
            format!("{}\n", layers.join("\n")).into_bytes(),
        );
        let mut notes = layers;
        notes.push(format!(
            "read {} file(s) from {}",
            files.len(),
            root.display()
        ));
        notes.extend(b.notes);
        Ok(ImportedBoard {
            data: b.data,
            models: BTreeMap::new(),
            extras,
            notes,
        })
    }
}
