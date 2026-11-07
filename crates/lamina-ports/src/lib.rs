//! The ports of Lamina.
//!
//! Lamina is built as a hexagon. The domain crates (`lamina-model`,
//! `lamina-geom`, `lamina-footprint`, `lamina-drc`, `lamina-route`,
//! `lamina-scene`) know nothing outside the board. `lamina-core` runs the
//! operations on a board and talks to the outside world only through the
//! traits in this crate, the ports. An adapter crate implements a port:
//! a parts vendor, a footprint library, a board file format, a fabrication
//! output, a 3D file reader, a renderer, a design source. The binaries
//! (`lamina-mcp`, `lamina-viewer`) plug the adapters into a [`Ports`] bundle
//! and give it to the core.
//!
//! A rule the compiler enforces: `lamina-core` does not depend on an adapter
//! crate. If an operation needs something from outside, it goes through a
//! port.

pub mod fab;
pub mod format;
pub mod mesh;
pub mod render;
pub mod router;
pub mod source;

pub use lamina_model::provider::{self, Provider, ProviderInfo};

use std::sync::Arc;

/// Everything the core gets from outside, bundled. The binaries build one
/// with the standard adapters. Tests build one with fakes.
#[derive(Clone, Default)]
pub struct Ports {
    /// Sources of parts, footprints and models, in the search order.
    pub providers: Vec<Arc<dyn Provider>>,
    /// Board file formats that Lamina reads or writes.
    pub formats: Vec<Arc<dyn format::BoardFormat>>,
    /// Fabrication outputs.
    pub fabricators: Vec<Arc<dyn fab::Fabricator>>,
    /// Reads 3D model files into meshes.
    pub meshes: Option<Arc<dyn mesh::MeshLoader>>,
    /// Writes the assembled board as a 3D file.
    pub step: Option<Arc<dyn mesh::StepWriter>>,
    /// Draws views of the board.
    pub renderer: Option<Arc<dyn render::Renderer>>,
    /// Builds board files from a design source.
    pub source: Option<Arc<dyn source::DesignSource>>,
    /// Exchanges with external autorouters.
    pub routers: Vec<Arc<dyn router::RouterExchange>>,
}

impl Ports {
    /// A port that is not plugged in gives this error.
    pub fn missing(what: &str) -> String {
        format!("No {what} is plugged in. The program that started Lamina did not give one.")
    }

    pub fn provider(&self, id: &str) -> Option<&dyn Provider> {
        self.providers
            .iter()
            .find(|p| p.info().id == id)
            .map(|p| p.as_ref())
    }

    pub fn format(&self, id: &str) -> Option<&dyn format::BoardFormat> {
        self.formats
            .iter()
            .find(|f| f.info().id == id)
            .map(|f| f.as_ref())
    }

    /// The format that can read `path`, by its own detection.
    pub fn format_for(&self, path: &std::path::Path) -> Option<&dyn format::BoardFormat> {
        self.formats
            .iter()
            .find(|f| f.detect(path))
            .map(|f| f.as_ref())
    }

    pub fn fabricator(&self, id: &str) -> Option<&dyn fab::Fabricator> {
        self.fabricators
            .iter()
            .find(|f| f.info().id == id)
            .map(|f| f.as_ref())
    }

    pub fn router(&self, id: &str) -> Option<&dyn router::RouterExchange> {
        self.routers
            .iter()
            .find(|r| r.info().id == id)
            .map(|r| r.as_ref())
    }
}
