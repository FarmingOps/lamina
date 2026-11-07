/// The serde default for a `bool` that starts as true.
pub(crate) fn yes() -> bool {
    true
}

pub mod board;
pub mod import;
pub mod kicad;
pub mod layout;
pub mod lcsc;
pub mod nets;
pub mod output;
pub mod parts;
pub mod place;
pub mod provider;
pub mod render;
pub mod route;
pub mod specctra;
pub mod status;

pub use board::{RectSpec, SetOutline, SetRules, SetStackup, UseFabPreset};
pub use import::{BoardFile, board_file_kind, import_board_file, import_dir_for, open_board_path};
pub use kicad::{ExportKicad, ImportKicadBoard, KicadExported};
pub use layout::{LayoutSaved, PlaceLayoutBlock, RepeatLayout, SaveLayoutBlock};
pub use lcsc::{ImportEasyedaBoard, natural, today};
pub use nets::{Connect, Disconnect, SetNets};
pub use output::{
    AddNote, Export, ExportStep, Exported, ImportGerbers, ListNotes, ResolveNotes, StepExported,
};
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
pub use specctra::{ExportSpecctra, ImportSpecctraSession, SpecctraExported};
pub use status::Status;
