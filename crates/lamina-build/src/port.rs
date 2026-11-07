//! The [`DesignSource`] port, implemented with the Jsonnet build functions
//! of this crate.

use std::collections::BTreeSet;
use std::path::Path;

use lamina_model::FileKind;
use lamina_model::error::Result;
use lamina_ports::source::{DesignSource, Report};

/// A `design.jsonnet` source in the board folder.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonnetSource;

impl DesignSource for JsonnetSource {
    fn file_name(&self) -> &str {
        crate::SOURCE_FILE
    }

    fn owned(&self, root: &Path) -> BTreeSet<FileKind> {
        crate::owned(root)
    }

    fn ensure_built(&self, root: &Path) -> Result<Option<Report>> {
        crate::ensure_built(root)
    }

    fn build(&self, root: &Path) -> Result<Report> {
        crate::build(root)
    }

    fn template(&self, name: &str) -> String {
        crate::template(name)
    }
}
