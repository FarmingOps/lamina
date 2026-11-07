//! KiCad compatibility: read `.kicad_mod` footprints and `.kicad_pcb`
//! boards, write boards KiCad can open, and find footprints in local or
//! online libraries. Nothing here runs KiCad.

pub mod footprint;
pub mod format;
pub mod layers;
pub mod library;
pub mod pcb;
pub mod project;
pub mod provider;

pub use lamina_model::sexpr;

pub use footprint::{Instance, KicadModel, convert, footprint_id, parse_footprint};
pub use format::KicadFormat;
pub use library::{DirSource, Hit, HttpSource, KicadSource};
pub use pcb::{ImportedBoard, export_board, export_board_with, import_board};
pub use project::apply_project;
