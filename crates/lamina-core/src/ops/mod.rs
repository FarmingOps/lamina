/// The serde default for a `bool` that starts as true.
pub(crate) fn yes() -> bool {
    true
}

pub mod board;
pub mod format;
pub mod import;
pub mod layout;
pub mod nets;
pub mod output;
pub mod parts;
pub mod place;
pub mod provider;
pub mod render;
pub mod route;
pub mod status;
pub mod text;

pub use board::{RectSpec, SetOutline, SetRules, SetStackup, UseFabPreset};
pub use format::{
    BoardExported, ExportBoard, ExportForRouter, IMPORT_DIR, ImportBoard, ImportRouterSession,
    RouterExported,
};
pub use import::{
    board_file_kind_in, import_board_file, import_dir_in, imports_dir, open_board_path_in,
};
pub use layout::{LayoutSaved, PlaceLayoutBlock, RepeatLayout, SaveLayoutBlock};
pub use nets::{Connect, Disconnect, SetNets};
pub use output::{AddNote, Export, ExportStep, Exported, ListNotes, ResolveNotes, StepExported};
pub use parts::{
    AddPart, AddParts, BodySize, FootprintFromPackage, FootprintFromPads, FootprintId,
    FootprintSummary, RemovePart, SetModel,
};
pub use place::{
    Align, Axis, Distribute, FlipPart, Measure, Measurement, MoveParts, Place, PlaceMany,
    RotatePart, Target, Unplace,
};
pub use provider::{
    FetchLibrary, Fetched, ImportPart, ImportParts, LibraryInfo, PartSearch, Providers, SearchParts,
};
pub use render::{Render, RenderView, Rendered, parse_scene_layer};
pub use route::{
    AddHole, AddKeepout, AddText, AddVia, AddZone, Autoroute, DeleteItems, Ripup, RouteTrace,
};
pub use status::Status;
pub use text::{natural, today};
