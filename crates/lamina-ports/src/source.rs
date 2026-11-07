//! A design source: a text file that generates the board files, for example
//! a Jsonnet design.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lamina_model::FileKind;
use lamina_model::error::Result;
use schemars::JsonSchema;
use serde::Serialize;

/// The result of a build.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct Report {
    pub source: String,
    pub owns: Vec<String>,
    /// Files that the build wrote because their content changed.
    pub written: Vec<String>,
    pub parts: usize,
    pub nets: usize,
    pub added_parts: Vec<String>,
    pub removed_parts: Vec<String>,
    /// Placements removed because their part was removed.
    pub unplaced: Vec<String>,
    pub warnings: Vec<String>,
    /// Every file the build read.
    pub inputs: Vec<String>,
}

pub trait DesignSource: Send + Sync {
    /// The file name of the source in a board folder, for example `design.jsonnet`.
    fn file_name(&self) -> &str;
    fn source_path(&self, root: &Path) -> PathBuf {
        root.join(self.file_name())
    }
    fn has_source(&self, root: &Path) -> bool {
        self.source_path(root).is_file()
    }
    /// The file kinds that the source generates, from the record of the last build.
    fn owned(&self, root: &Path) -> BTreeSet<FileKind>;
    /// Build if the source or an input changed. `None` if nothing was built.
    fn ensure_built(&self, root: &Path) -> Result<Option<Report>>;
    fn build(&self, root: &Path) -> Result<Report>;
    /// A starter source for a new board.
    fn template(&self, name: &str) -> String;
}
