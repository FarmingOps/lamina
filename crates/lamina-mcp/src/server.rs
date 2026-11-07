//! MCP tool surface. Each tool locks the open project, runs one core
//! operation and returns its result as JSON text.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lamina_core::Project;
use lamina_core::model::{self, Severity};
use lamina_core::ops::*;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const INSTRUCTIONS: &str = "\
Lamina is a PCB layout engine. The board is in a project folder of JSON files \
(board, parts, nets, placement, routing, notes, cache/footprints). A viewer can watch \
these files and draws the board again after each change.

Conventions: millimetres, Y up, rotation in degrees counter-clockwise, fixed origin \
(a change to the outline does not move other items). Copper layers: `top`, `in1`.., \
`bottom`. Other layers: `top.silk`, `top.mask`, `top.paste`, `top.courtyard`, `top.fab`, \
`bottom.*`, `outline`. Refer to a pad as `REFDES.PAD`, for example `U1.3`.

A board can be modular: `design.jsonnet` (Jsonnet = JSON plus variables, functions and \
`import`) generates board.json, parts.json and nets.json. The tools then cannot change \
these files. Edit the source as text. Lamina builds it again before the next tool call. \
Placement and routing stay plain JSON. Edit them with the tools as usual.

Shared blocks are in the workspace `lib` folders (see `lamina.json`). \
`import 'lamina.libsonnet'` gives `merge`, `group`, `net`, `pads`, `rect`, `hole` and \
`corner_holes`. Put each block instance in `lamina.group('name', ...)`. Lay out one \
instance, then use `repeat_layout` to copy its layout to the other instances.

Lamina applies each change and reports the new rule violations. To refuse a change \
that adds an error, set `strict: true`. Lamina always refuses malformed input. \
Start with `open` (create: true for a new project), then `status`.";

pub type Shared = Arc<Mutex<Option<Project>>>;

#[derive(Clone)]
pub struct Lamina {
    project: Shared,
    tool_router: ToolRouter<Lamina>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct OpenParams {
    /// Project folder.
    pub path: String,
    /// If the folder has no project, create one.
    #[serde(default)]
    pub create: bool,
    /// The board name for a new project (the default is the folder name).
    #[serde(default)]
    pub name: Option<String>,
    /// With `create`: start the board from a modular `design.jsonnet`
    /// source instead of plain JSON files.
    #[serde(default)]
    pub modular: bool,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SchemaParams {
    /// One of: board, parts, nets, placement, routing, notes, footprint.
    pub kind: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct CheckParams {
    /// Report only these rule ids.
    #[serde(default)]
    pub rules: Option<Vec<String>>,
    /// Include warnings (default true).
    #[serde(default = "yes")]
    pub warnings: bool,
    /// The maximum number of violations to return (default 200).
    #[serde(default)]
    pub limit: Option<usize>,
}

fn yes() -> bool {
    true
}

pub fn json_result<T: Serialize>(v: &T) -> CallToolResult {
    match serde_json::to_string_pretty(v) {
        Ok(s) => CallToolResult::success(vec![ContentBlock::text(s)]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
    }
}

pub fn error_result(e: impl std::fmt::Display) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(e.to_string())])
}

impl Lamina {
    pub fn new(project: Option<Project>) -> Self {
        Lamina {
            project: Arc::new(Mutex::new(project)),
            tool_router: Self::tool_router(),
        }
    }

    /// Run `f` on the open project.
    pub fn with<T: Serialize>(
        &self,
        f: impl FnOnce(&mut Project) -> lamina_core::Result<T>,
    ) -> Result<CallToolResult, McpError> {
        let mut guard = self.project.lock().unwrap_or_else(|p| p.into_inner());
        let Some(p) = guard.as_mut() else {
            return Ok(error_result(
                "No project is open. Call `open` with a folder path first.",
            ));
        };
        // Read the edits that were made in the viewer or by hand after the last call.
        if let Err(e) = p.sync() {
            return Ok(error_result(format!("Cannot read the project again: {e}")));
        }
        Ok(match f(p) {
            Ok(v) => json_result(&v),
            Err(e) => error_result(e),
        })
    }
}

/// Open a project to read it, without the lock. This also works when an
/// agent or another tool has the project open.
pub fn read_project(path: &str) -> lamina_core::Result<Project> {
    let dir = PathBuf::from(path);
    if !model::ProjectData::is_project(&dir) {
        return Err(model::Error::NotAProject(dir));
    }
    let mut p = Project::open_shared(&dir)?;
    crate::register_checkers(&mut p);
    Ok(p)
}

pub fn open_project(
    path: &str,
    create: bool,
    name: Option<String>,
    modular: bool,
) -> lamina_core::Result<Project> {
    let dir = PathBuf::from(path);
    if model::ProjectData::is_project(&dir) {
        let mut p = Project::open(&dir)?;
        crate::register_checkers(&mut p);
        return Ok(p);
    }
    if !create {
        return Err(model::Error::NotAProject(dir));
    }
    let name = name.unwrap_or_else(|| {
        dir.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("board")
            .to_string()
    });
    let mut p = Project::create(&dir, &name)?;
    crate::register_checkers(&mut p);
    if modular {
        p.init_source()?;
    }
    Ok(p)
}

#[tool_router]
impl Lamina {
    /// Open a project folder, or create one with `create: true`. Only one
    /// project is open at a time.
    #[tool(annotations(title = "Open project"))]
    async fn open(
        &self,
        Parameters(p): Parameters<OpenParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut guard = self.project.lock().unwrap_or_else(|p| p.into_inner());
        // First, release the lock of the previous project.
        *guard = None;
        match open_project(&p.path, p.create, p.name, p.modular) {
            Ok(proj) => {
                let s = proj.status();
                *guard = Some(proj);
                Ok(json_result(&s))
            }
            Err(e) => Ok(error_result(e)),
        }
    }

    /// A summary of the open project: size, layers, counts, unplaced parts,
    /// and the number of violations for each rule.
    #[tool(annotations(title = "Project status", read_only_hint = true))]
    async fn status(&self) -> Result<CallToolResult, McpError> {
        self.with(|p| Ok(p.status()))
    }

    /// Search the KiCad footprint libraries on this computer: the
    /// `kicad_footprints` folders in lamina.json, KICAD*_FOOTPRINT_DIR and
    /// standard installs. `import_kicad_footprint` with `Library:Name` also
    /// works without a local library. It then downloads from the official
    /// KiCad libraries.
    #[tool(annotations(title = "Search KiCad footprints", read_only_hint = true))]
    async fn search_kicad_footprints(
        &self,
        Parameters(p): Parameters<SearchKicadFootprints>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.search_kicad_footprints(p))
    }

    /// Import a KiCad footprint and its STEP model from a local library, a
    /// `.kicad_mod` file, or the official KiCad libraries online
    /// (`Library:Name`, for example `Package_TO_SOT_SMD:SOT-23`). With
    /// `refdes`, also add a part.
    #[tool(annotations(title = "Import KiCad footprint"))]
    async fn import_kicad_footprint(
        &self,
        Parameters(p): Parameters<ImportKicadFootprint>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.import_kicad_footprint(p))
    }

    /// Import a `.kicad_pcb` board into the open project: outline, parts
    /// with footprints, nets, placement, tracks, vias, zones, keep-outs and
    /// texts. The project must be empty, or `replace` must be true.
    #[tool(annotations(title = "Import KiCad board"))]
    async fn import_kicad_board(
        &self,
        Parameters(p): Parameters<ImportKicadBoard>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.import_kicad_board(p))
    }

    /// Import an EasyEDA (standard editor) PCB JSON file that was saved with
    /// File > Export > EasyEDA: outline, parts with footprints and LCSC
    /// numbers, nets, placement, tracks, vias, copper areas, holes and
    /// texts. The tool downloads the 3D models from EasyEDA. The project must
    /// be empty, or `replace` must be true.
    #[tool(annotations(title = "Import EasyEDA board"))]
    async fn import_easyeda_board(
        &self,
        Parameters(p): Parameters<ImportEasyedaBoard>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.import_easyeda_board(p))
    }

    /// Write a Specctra DSN of the board for an external autorouter, for
    /// example Freerouting (default `output/<name>.dsn`). The existing routes
    /// are exported as protected wiring. If `reroute_all` is true, they are
    /// not exported.
    #[tool(annotations(title = "Export Specctra DSN"))]
    async fn export_specctra(
        &self,
        Parameters(p): Parameters<ExportSpecctra>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.export_specctra(p))
    }

    /// Read the `.ses` session that a Specctra router wrote, and put its
    /// tracks and vias on the board. They replace the routes of those nets.
    #[tool(annotations(title = "Import Specctra session"))]
    async fn import_specctra_session(
        &self,
        Parameters(p): Parameters<ImportSpecctraSession>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.import_specctra_session(p))
    }

    /// Import Gerber and drill files (a folder or a zip) from any CAD
    /// program as artwork: board outline, holes, copper and silkscreen as
    /// drawings. Fab files have no parts or nets, so use the result only to
    /// look at, render and check the board. The project must be empty, or
    /// `replace` must be true.
    #[tool(annotations(title = "Import Gerbers"))]
    async fn import_gerbers(
        &self,
        Parameters(p): Parameters<ImportGerbers>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.import_gerbers(p))
    }

    /// Write the assembled board as a STEP file (AP214) for mechanical CAD:
    /// the board solid with its outline, cut-outs and drilled holes, and the
    /// 3D model of each part at its placement. The file includes the full part
    /// STEP files, so the geometry and the colours are exact. The default is
    /// `output/<name>.step`.
    #[tool(annotations(title = "Export STEP"))]
    async fn export_step(
        &self,
        Parameters(p): Parameters<ExportStep>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.export_step(p))
    }

    /// Write the board as a `.kicad_pcb` file that KiCad 8 or a newer version
    /// can open (default `output/<name>.kicad_pcb`). KiCad fills the zones
    /// again.
    #[tool(annotations(title = "Export KiCad board"))]
    async fn export_kicad(
        &self,
        Parameters(p): Parameters<ExportKicad>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.export_kicad(p))
    }

    /// Build `design.jsonnet` into board.json, parts.json and nets.json, then
    /// reload. Each tool also builds again automatically if the source or an
    /// imported file changed. Use this tool to force a build or to see the
    /// report (added and removed parts, warnings).
    #[tool(annotations(title = "Build design source"))]
    async fn build(&self) -> Result<CallToolResult, McpError> {
        self.with(|p| p.build())
    }

    /// Read all files from disk again, for example after you edit the JSON by hand.
    #[tool(annotations(title = "Reload from disk"))]
    async fn reload(&self) -> Result<CallToolResult, McpError> {
        self.with(|p| {
            p.reload()?;
            Ok(p.status())
        })
    }

    /// The JSON schema of a project file kind.
    #[tool(annotations(title = "File schema", read_only_hint = true))]
    async fn schema(
        &self,
        Parameters(p): Parameters<SchemaParams>,
    ) -> Result<CallToolResult, McpError> {
        Ok(match model::schema_for(&p.kind) {
            Some(s) => json_result(&s),
            None => error_result(format!(
                "Unknown kind `{}`. Use one of these: {}",
                p.kind,
                model::SCHEMA_KINDS.join(", ")
            )),
        })
    }

    /// List the rule violations that the project has now (validation and DRC).
    #[tool(annotations(title = "Check", read_only_hint = true))]
    async fn check(
        &self,
        Parameters(p): Parameters<CheckParams>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| {
            let mut v = proj.check();
            if let Some(rules) = &p.rules {
                v.retain(|x| rules.contains(&x.rule));
            }
            if !p.warnings {
                v.retain(|x| x.severity == Severity::Error);
            }
            v.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.rule.cmp(&b.rule)));
            let total = v.len();
            v.truncate(p.limit.unwrap_or(200));
            Ok(serde_json::json!({ "total": total, "violations": v }))
        })
    }

    /// Set the board outline: a rectangle (with optional round corners) or a
    /// contour of lines and arcs. If you give cut-outs, they replace the
    /// existing cut-outs.
    #[tool(annotations(title = "Set outline"))]
    async fn set_outline(
        &self,
        Parameters(p): Parameters<SetOutline>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.set_outline(p))
    }

    /// Change the number of copper layers, the thickness, the copper weight,
    /// the finish or the colours. Only the fields that you give change.
    #[tool(annotations(title = "Set stackup"))]
    async fn set_stackup(
        &self,
        Parameters(p): Parameters<SetStackup>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.set_stackup(p))
    }

    /// Change the design rules. Only the fields that you give change, for example
    /// `{"rules": {"clearance": 0.2, "net_classes": {"power": {"track_width": 0.5}}}}`.
    #[tool(annotations(title = "Set rules"))]
    async fn set_rules(
        &self,
        Parameters(p): Parameters<SetRules>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.set_rules(p))
    }

    /// Set the netlist: `{"nets": {"GND": {"pads": ["U1.1", "C1.2"]}}}`.
    /// This replaces the whole netlist. If `replace` is false, the nets merge
    /// into the netlist.
    #[tool(annotations(title = "Set nets"))]
    async fn set_nets(
        &self,
        Parameters(p): Parameters<SetNets>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.set_nets(p))
    }

    /// Add pads to a net. If the net does not exist, create it. You can also
    /// set its class.
    #[tool(annotations(title = "Connect"))]
    async fn connect(
        &self,
        Parameters(p): Parameters<Connect>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.connect(p))
    }

    /// Remove pads from a net. If you do not give `pads`, remove the whole net.
    #[tool(annotations(title = "Disconnect"))]
    async fn disconnect(
        &self,
        Parameters(p): Parameters<Disconnect>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.disconnect(p))
    }
    /// Generate an IPC-7351 footprint from a package name. Names: `R_0402`,
    /// `R_0603`, `C_0805`, `LED_0603`, `D_SOD-123`, `SOT-23`, `SOT-23-5`,
    /// `SOT-23-6`, `SOT-223`, `SOIC-8`, `SOIC-16W`, `TSSOP-28`, `SSOP-20`,
    /// `MSOP-10`, `QFN-32_5x5_P0.5_EP3.3x3.3`, `DFN-8_3x3_P0.65`,
    /// `LQFP-48_7x7_P0.5`, `PinHeader_1x03_P2.54`, `BGA-64_8x8_P0.8_5x5`.
    #[tool(annotations(title = "Footprint from package"))]
    async fn footprint_from_package(
        &self,
        Parameters(p): Parameters<FootprintFromPackage>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.footprint_from_package(p))
    }

    /// Create a footprint from pads that you type from the land pattern in a
    /// datasheet. If you give no courtyard, the tool adds a courtyard, a fab
    /// outline and silkscreen.
    #[tool(annotations(title = "Footprint from pads"))]
    async fn footprint_from_pads(
        &self,
        Parameters(p): Parameters<FootprintFromPads>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.footprint_from_pads(p))
    }

    /// The full JSON of a footprint: pads, graphics, model, provenance.
    #[tool(annotations(title = "Get footprint", read_only_hint = true))]
    async fn get_footprint(
        &self,
        Parameters(p): Parameters<FootprintId>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.get_footprint(&p.id))
    }

    /// The footprints in the project, with the number of pads, the source and
    /// the parts that use each footprint.
    #[tool(annotations(title = "List footprints", read_only_hint = true))]
    async fn list_footprints(&self) -> Result<CallToolResult, McpError> {
        self.with(|proj| Ok(proj.list_footprints()))
    }

    /// Add a part (refdes, footprint id, value, MPN, LCSC number).
    #[tool(annotations(title = "Add part"))]
    async fn add_part(
        &self,
        Parameters(p): Parameters<AddPart>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.add_part(p))
    }

    /// Add many parts in one call.
    #[tool(annotations(title = "Add parts"))]
    async fn add_parts(
        &self,
        Parameters(p): Parameters<AddParts>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.add_parts(p))
    }

    /// Remove a part and its placement. By default, also remove its pads from
    /// the nets.
    #[tool(annotations(title = "Remove part"))]
    async fn remove_part(
        &self,
        Parameters(p): Parameters<RemovePart>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.remove_part(p))
    }
    /// Search LCSC / JLCPCB parts by keyword, MPN or LCSC number. The results
    /// show the package, the stock, the price and if the part is a JLCPCB
    /// basic part.
    #[tool(annotations(title = "Search parts", read_only_hint = true, open_world_hint = true))]
    async fn search_part(
        &self,
        Parameters(p): Parameters<SearchPart>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.search_part(p))
    }

    /// Import an LCSC part. The tool converts its EasyEDA footprint, downloads
    /// the 3D model and caches all of it in the project. It can also add a
    /// part.
    #[tool(annotations(title = "Import LCSC part", open_world_hint = true))]
    async fn import_lcsc(
        &self,
        Parameters(p): Parameters<ImportLcsc>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.import_lcsc(p))
    }

    /// Import several LCSC parts in one call.
    #[tool(annotations(title = "Import LCSC parts", open_world_hint = true))]
    async fn import_lcsc_many(
        &self,
        Parameters(p): Parameters<ImportMany>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.import_many(p))
    }
    /// Load design rules from a fab preset: `jlcpcb-2layer`, `jlcpcb-4layer`, `relaxed`.
    #[tool(annotations(title = "Use fab preset"))]
    async fn use_fab_preset(
        &self,
        Parameters(p): Parameters<UseFabPreset>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.use_fab_preset(p))
    }

    /// Place a part at x, y (mm) with an optional rotation (degrees CCW) and
    /// side.
    #[tool(annotations(title = "Place part"))]
    async fn place(&self, Parameters(p): Parameters<Place>) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.place(p))
    }

    /// Place many parts: `{"placements": {"R1": {"x": 10, "y": 5, "rotation": 90}}}`.
    #[tool(annotations(title = "Place parts"))]
    async fn place_many(
        &self,
        Parameters(p): Parameters<PlaceMany>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.place_many(p))
    }

    /// Move placed parts by dx, dy.
    #[tool(annotations(title = "Move parts"))]
    async fn move_parts(
        &self,
        Parameters(p): Parameters<MoveParts>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.move_parts(p))
    }

    /// Rotate a part to an absolute angle (`rotation`) or by a relative angle
    /// (`by`).
    #[tool(annotations(title = "Rotate part"))]
    async fn rotate_part(
        &self,
        Parameters(p): Parameters<RotatePart>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.rotate_part(p))
    }

    /// Move a part to the other side of the board.
    #[tool(annotations(title = "Flip part"))]
    async fn flip_part(
        &self,
        Parameters(p): Parameters<FlipPart>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.flip_part(p))
    }

    /// Align parts on a common X or Y.
    #[tool(annotations(title = "Align parts"))]
    async fn align(&self, Parameters(p): Parameters<Align>) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.align(p))
    }

    /// Put parts at equal distances along X or Y.
    #[tool(annotations(title = "Distribute parts"))]
    async fn distribute(
        &self,
        Parameters(p): Parameters<Distribute>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.distribute(p))
    }

    /// Copy the layout of one block to other blocks: the placement of each
    /// part, and the tracks and vias that connect to the pads of the block in
    /// its area. These replace the old tracks and vias of the target in that
    /// area. A block is a group name (`lamina.group` in the source, or `group`
    /// on parts) or a refdes list. The tool pairs parts by footprint and
    /// wiring, not by name. The anchor part does not move, and the tool puts
    /// the other parts around it.
    #[tool(annotations(title = "Repeat layout"))]
    async fn repeat_layout(
        &self,
        Parameters(p): Parameters<RepeatLayout>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.repeat_layout(p))
    }

    /// Save the layout of a block (placement and routing) as
    /// `layouts/<name>.json` in the first workspace library. If the board has
    /// no workspace, save it in the board. Other boards can then place it.
    #[tool(annotations(title = "Save layout block"))]
    async fn save_layout_block(
        &self,
        Parameters(p): Parameters<SaveLayoutBlock>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.save_layout_block(p))
    }

    /// Put a saved layout on blocks of this board. This is like
    /// `repeat_layout`, but the layout comes from `layouts/<name>.json` (first
    /// in the board, then in the libraries).
    #[tool(annotations(title = "Place layout block"))]
    async fn place_layout_block(
        &self,
        Parameters(p): Parameters<PlaceLayoutBlock>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.place_layout_block(p))
    }

    /// Remove placements.
    #[tool(annotations(title = "Unplace parts"))]
    async fn unplace(
        &self,
        Parameters(p): Parameters<Unplace>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.unplace(p))
    }

    /// Measure the distance between two pads (`U1.3`), parts (`U1`) or points.
    #[tool(annotations(title = "Measure", read_only_hint = true))]
    async fn measure(
        &self,
        Parameters(p): Parameters<Measure>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.measure(p))
    }

    /// Add a trace on one copper layer. Each point is `{x, y}` or a pad
    /// reference, for example `U1.3`. A pad reference snaps to the pad centre.
    #[tool(annotations(title = "Route trace"))]
    async fn route_trace(
        &self,
        Parameters(p): Parameters<RouteTrace>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.route_trace(p))
    }

    /// Add a through via at a point or on a pad.
    #[tool(annotations(title = "Add via"))]
    async fn add_via(&self, Parameters(p): Parameters<AddVia>) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.add_via(p))
    }

    /// Add a copper pour (zone). By default, its outline is the board outline.
    #[tool(annotations(title = "Add zone"))]
    async fn add_zone(
        &self,
        Parameters(p): Parameters<AddZone>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.add_zone(p))
    }

    /// Add a keep-out area on copper layers.
    #[tool(annotations(title = "Add keep-out"))]
    async fn add_keepout(
        &self,
        Parameters(p): Parameters<AddKeepout>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.add_keepout(p))
    }

    /// Add a mounting hole to the board. If you give pad_diameter, the hole is
    /// plated.
    #[tool(annotations(title = "Add hole"))]
    async fn add_hole(
        &self,
        Parameters(p): Parameters<AddHole>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.add_hole(p))
    }

    /// Add text on silkscreen or copper.
    #[tool(annotations(title = "Add text"))]
    async fn add_text(
        &self,
        Parameters(p): Parameters<AddText>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.add_text(p))
    }

    /// Remove tracks and vias by net, ids, layer or region.
    #[tool(annotations(title = "Rip up"))]
    async fn ripup(&self, Parameters(p): Parameters<Ripup>) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.ripup(p))
    }

    /// Delete routing items, zones, keep-outs, texts or holes by id.
    #[tool(annotations(title = "Delete items"))]
    async fn delete_items(
        &self,
        Parameters(p): Parameters<DeleteItems>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.delete_items(p))
    }

    /// Route all open connections (45-degree tracks, vias between layers)
    /// with the built-in router. The nets negotiate for space. The router
    /// repairs failed connections on a finer grid. Then it fills the pours
    /// again and routes the connections that the pours leave open. Add pours
    /// (`add_zone`) first: a net with a pour then gets wires only where its
    /// pour cannot reach. Use `skip` for nets that you route by hand.
    #[tool(annotations(title = "Autoroute"))]
    async fn autoroute(
        &self,
        Parameters(p): Parameters<Autoroute>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.autoroute(p))
    }

    /// The connections that are not routed, as lines from pad to pad.
    #[tool(annotations(title = "Ratsnest", read_only_hint = true))]
    async fn ratsnest(&self) -> Result<CallToolResult, McpError> {
        self.with(|proj| Ok(proj.ratsnest()))
    }
    /// Render the board to PNG and return it as an image. Views: `layers`
    /// (technical view, select the layers), `top` / `bottom` (the finished
    /// board), `3d` (perspective with part models). In 3D, `yaw` and `pitch`
    /// turn the camera, `focus` points it at a part, a pad or a point,
    /// `region` fits a box, `zoom` moves it closer, and `hide` / `only`
    /// select the parts to draw. The tool saves the PNG in cache/render.
    #[tool(annotations(title = "Render", read_only_hint = true))]
    async fn render(&self, Parameters(p): Parameters<Render>) -> Result<CallToolResult, McpError> {
        let mut guard = self.project.lock().unwrap_or_else(|p| p.into_inner());
        let Some(proj) = guard.as_mut() else {
            return Ok(error_result(
                "No project is open. Call `open` with a folder path first.",
            ));
        };
        if let Err(e) = proj.sync() {
            return Ok(error_result(format!("Cannot read the project again: {e}")));
        }
        match proj.render(p) {
            Ok(r) => {
                use base64::Engine;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&r.png);
                let meta = serde_json::to_string(&r).unwrap_or_default();
                Ok(CallToolResult::success(vec![
                    ContentBlock::image(b64, "image/png"),
                    ContentBlock::text(meta),
                ]))
            }
            Err(e) => Ok(error_result(e)),
        }
    }
    /// Write Gerber X2, Excellon (PTH and NPTH), JLCPCB BOM and CPL files and
    /// a Gerber zip into the output folder of the project. If DRC has errors
    /// and `force` is not true, the tool refuses.
    #[tool(annotations(title = "Export for fabrication"))]
    async fn export(&self, Parameters(p): Parameters<Export>) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.export(p))
    }

    /// The notes that the user wrote in the viewer (by default, only the open
    /// notes). Read them before and after each set of changes.
    #[tool(annotations(title = "Notes", read_only_hint = true))]
    async fn notes(
        &self,
        Parameters(p): Parameters<ListNotes>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| Ok(proj.list_notes(p)))
    }

    /// Write a note for the user. The viewer shows it.
    #[tool(annotations(title = "Add note"))]
    async fn add_note(
        &self,
        Parameters(p): Parameters<AddNote>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.add_note(p))
    }

    /// Mark notes as resolved.
    #[tool(annotations(title = "Resolve notes"))]
    async fn resolve_notes(
        &self,
        Parameters(p): Parameters<ResolveNotes>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| {
            proj.resolve_notes(p)
                .map(|n| serde_json::json!({"resolved": n}))
        })
    }
    /// Use a different 3D model (STEP or OBJ) for a footprint, for example the
    /// STEP file of the manufacturer. The reply includes the raw bounds of the
    /// model. Use them to calculate the offset that puts the model on the
    /// centre of the pads.
    #[tool(annotations(title = "Set 3D model"))]
    async fn set_model(
        &self,
        Parameters(p): Parameters<SetModel>,
    ) -> Result<CallToolResult, McpError> {
        self.with(|proj| proj.set_model(p))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Lamina {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("lamina", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}
