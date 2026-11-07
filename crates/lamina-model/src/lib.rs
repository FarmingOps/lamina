//! Lamina's board data model.
//!
//! Units are millimetres (f64). Y points up. Rotations are in degrees
//! counter-clockwise. The project origin is fixed: if you change the board
//! outline, nothing else moves.

pub mod error;
pub mod files;
pub mod footprint;
pub mod geom;
pub mod json;
pub mod layer;
pub mod library;
pub mod project;
pub mod provider;
pub mod sexpr;
pub mod workspace;

pub use error::Error;
pub use files::*;
pub use footprint::*;
pub use geom::*;
pub use layer::*;
pub use library::*;
pub use project::*;
pub use workspace::*;

/// JSON schema of a project file kind, for agents to read.
pub fn schema_for(kind: &str) -> Option<serde_json::Value> {
    let s = match kind {
        "board" => schemars::schema_for!(BoardFile),
        "parts" => schemars::schema_for!(PartsFile),
        "nets" => schemars::schema_for!(NetsFile),
        "placement" => schemars::schema_for!(PlacementFile),
        "routing" => schemars::schema_for!(RoutingFile),
        "notes" => schemars::schema_for!(NotesFile),
        "footprint" => schemars::schema_for!(Footprint),
        _ => return None,
    };
    serde_json::to_value(s).ok()
}

pub const SCHEMA_KINDS: [&str; 7] = [
    "board",
    "parts",
    "nets",
    "placement",
    "routing",
    "notes",
    "footprint",
];
