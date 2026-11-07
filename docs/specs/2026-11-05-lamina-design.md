# Lamina design

Status: draft, 2026-11-05.

## 1. Intent

Lamina is a PCB layout program in Rust. Its primary user is a coding agent
(Claude Code, Codex or any MCP client). A person watches and writes
comments. Lamina does not use other EDA tools. It must work when no window
is open. When the viewer is open, it must show each change of the agent at
the time that it occurs, in 2D and 3D.

The tools that exist now do one of these:

- They add an assistant to a GUI that is made for a mouse (Flux, EasyEDA).
- They give access to the file format of a GUI tool through a thin MCP
  wrapper (the KiCad MCP servers).
- They make a new language that the agent must learn in each session
  (atopile).

None of them gives the agent a clean board model, tools with results that
the agent can verify, and a fast way to see what it did.

## 2. Goals, non-goals, anti-goals

Goals

- An agent can take a board from a parts list and a netlist to Gerbers
  that are ready for the fab, with only MCP tools. It can verify each step.
- The project is plain files in git. Diffs are easy to read.
- A native viewer shows the live board in 2D and 3D, and draws it again
  after each change. The viewer can run or not run, and this has no effect
  on the agent.
- There is no bundled parts library. Lamina gets parts, footprints and 3D
  models when it needs them, and caches them in the project.
- Fast and light: a dense 4-layer board renders at 60 fps, the memory use
  stays low, and the PC does not freeze.
- The first real output is our own reference board (a small USB-C
  microcontroller board from LCSC parts) and a test coupon. Both are
  exported for JLCPCB with assembly files.
- Each part shows its real 3D model. A grey box is only a temporary
  condition when a model is missing or did not load. It is never the final
  condition.

Non-goals (not in version one)

- A schematic editor or a schematic drawing. The netlist comes from a text
  description or a file.
- Drag-and-drop edits by a person in the viewer. In version one, the person
  watches and writes comments. Edits come in a later phase.
- Simulation (SPICE, signal integrity, thermal).
- Import of designs from Altium, Eagle, KiCad or EasyEDA.
- More than one user or more than one agent on one project at the same
  time.

Anti-goals (if Lamina has one of these, it is a failure, also if it works)

- A dependency on KiCad, its libraries or its CLI.
- A daemon that must run before the agent can work. If a window or a server
  must be open for the agent to work, the design is a failure.
- A new language. The agent uses JSON and tool calls. It has nothing to
  learn.
- A footprint that is wrong because somebody guessed it. Each footprint
  comes from a datasheet drawing or an IPC-7351 formula, and Lamina records
  the inputs.

## 3. Constraints

- Rust for all of the code. One Cargo workspace.
- MCP over stdio. The client starts one agent process for each session.
- Windows 11 first (the development computer), then macOS. Linux will
  possibly work, but version one does not test it.
- No AI attribution in the repository (commits, docs, code).
- Keep the system load low during builds and runs: no parallel heavy jobs.
- The crate name `lamina` on crates.io is not available (a compiler backend
  uses it). If we publish crates, they use the names `lamina-core`,
  `lamina-mcp` and `lamina-viewer`. These names were available on
  2026-11-05. The binaries are still `lamina` and `lamina-viewer`.

## 4. Approaches considered

1. One native app that contains everything, also the MCP server. This is
   the simplest approach. But the agent can work only when the window is
   open, and a viewer crash stops the session. Rejected: this goes against
   an anti-goal.
2. A core library, a headless engine that uses MCP over stdio, and a
   separate viewer process that watches the project files. The agent and
   the viewer share only the files on disk. Chosen.
3. The same as 2, but a localhost HTTP daemon that runs for a long time
   replaces stdio. Then many clients can share one engine in memory.
   Rejected for version one, for these reasons:
   - The state is already in files.
   - During development, a daemon runs old code after each rebuild.
   - A daemon needs lifecycle management and a localhost token.
   - The user runs only one agent at a time.

   We can add an HTTP transport later without a change to the core.

Transport note: stdio and localhost HTTP are both much faster than any DRC
or render. So speed was not the reason for this decision.

## 5. Decisions

### 5.1 Workspace layout

- `lamina-core` (library): data model, units, geometry, footprint
  generator, parts download and cache, DRC, raster renderer, exporters,
  project load and save. You can test all of it without a window.
- `lamina-mcp` (binary `lamina`): a stdio MCP server that uses `rmcp`. It
  also has the same operations as CLI subcommands (`lamina check`,
  `lamina render`, `lamina export`), so that persons and CI can use them.
- `lamina-viewer` (binary): an egui + wgpu window. It watches the project
  folder with `notify`, then it loads the files again and draws 2D and 3D
  again. In version one, it is read-only. It links `lamina-core` to parse
  and tessellate. There is no protocol between the engine and the viewer.
  The files are the protocol.

### 5.2 Project on disk

A project is a folder in git:

```
board.json        outline, stackup, layers, design rules, fab preset
parts.json        refdes -> part (MPN, LCSC id, footprint id, value, 3D ref)
nets.json         net name -> list of (refdes, pad)
placement.json    refdes -> (x, y, rotation, side)
routing.json      traces, vias, zones (pours), keep-outs, text, graphics
cache/footprints/ generated footprints or footprints from datasheets, one JSON
                  file each, with provenance (formula + inputs, or datasheet page)
cache/parts/      distributor data as downloaded
cache/models/     STEP as downloaded, plus a tessellated mesh (binary) for each model
cache/render/     PNG snapshots that the agent asked for
```

All geometry is in millimetres as f64, Y up, rotation counter-clockwise in
degrees. The project origin is fixed and does not depend on the outline. A
change to the outline never moves other items. So mounting holes that
come from the housing CAD stay stable from the first placement. A
convention, not a rule: put the origin at the lower-left corner of the
finished board.

Each file has a `schema` version field. `lamina-core` publishes the JSON
schema for each file, so an agent can read it with one tool call.

Lamina saves files atomically (write a temporary file, then rename it).
Because of this, the viewer never reads a half file. A `.lamina.lock` file in the
project folder prevents a second engine from opening the same project. This
is a safety feature, not a function for the user.

### 5.3 MCP tool surface

The tools are small and strictly typed. Each tool returns a result that the
agent can check. This is a sketch. The builder decides the final names:

- project: `open`, `status`, `schema`
- board: `set_outline`, `set_stackup`, `set_rules` (or `use_fab_preset`)
- parts: `search_parts` (all providers), `import_part`, `add_part`,
  `remove_part`, `footprint_from_package` (IPC-7351),
  `footprint_from_pads` (numbers typed from the datasheet), `fetch_model`
- nets: `set_nets` (full netlist), `connect`, `disconnect`
- placement: `place`, `move`, `rotate`, `flip`, `align`, `distribute`
- routing: `route_trace` (explicit points), `add_via`, `add_zone`,
  `add_keepout`, `autoroute` (see 5.7), `ripup`
- checks: `drc`, `connectivity` (unrouted nets, shorts), `measure`
- views: `render` (PNG of a layer set or a 3D view, returned as an image),
  `describe` (text summary of the board or of a region)
- output: `export_gerbers`, `export_drill`, `export_bom`, `export_pnp`,
  `export_step`

Rules for all tools:

- Validate the input before you change the project.
- Return what changed and each rule that is now violated.
- Never succeed without a report.

`drc` and `connectivity` are fast enough to run after each mutation. Their
result is in the reply of the mutation.

By default, a mutation is applied and reported. You can place a part over
the courtyard of a different part or in a keep-out, and the reply lists the
violations. Rough placement passes need this freedom. Each mutating tool
accepts `strict: true` to refuse instead. Then, if the change adds a
violation, the project does not change. Lamina always refuses malformed
input (unknown refdes, a pad that does not exist, a trace with one point, a
layer that is not in the stackup), also if `strict` is false.

`set_nets` accepts the shape of the Lamina `nets.json` file. Other netlist
formats are imports and are not in the scope of version one (see the
non-goals).

### 5.4 Parts and footprints

Lamina does not include a library. These are the sources, in the sequence
of preference:

1. LCSC / JLCPCB / EasyEDA: part search, stock, price, package name, and
   the EasyEDA footprint and 3D model (OBJ, and STEP if available). Lamina
   imports them, converts them into its own footprint format and caches
   them in the project. There is no official API. The endpoints are
   unofficial, so each endpoint is behind a source trait, and a failure
   does not stop all of the function. This is important because JLCPCB
   assembly uses only LCSC parts.
2. Manufacturer: STEP model and datasheet land pattern. The agent reads
   the datasheet and calls `footprint_from_pads` with the numbers. The
   footprint keeps the datasheet name and page.
3. IPC-7351 generator for standard packages (chip 0402 to 2512, SOT,
   SOIC, QFN, LQFP, BGA, LGA grids). The inputs are the package
   dimensions. The outputs are pads, paste, mask, silkscreen and
   courtyard, at a selected density level. The footprint keeps the formula
   inputs.
4. Distributor APIs (Mouser, Digi-Key, Nexar) for parameters and prices,
   if LCSC does not have the part. The keys are in the configuration of the
   user, not in the project.

Not used: SnapEDA, Ultra Librarian, SamacSys (their licences do not let
us redistribute their files), KiCad libraries.

3D models: the STEP or OBJ file is the stored source. When Lamina uses a
model for the first time, it tessellates the model into a binary mesh. It
caches the mesh next to the source file. The viewer and the 3D renderer
load meshes, never STEP. If a part has no model yet, Lamina draws a grey
box from its courtyard and the package height.

Real models are mandatory. Lamina has its own STEP reader and tessellator
in Rust (a subset of AP203/AP214/AP242 B-rep, assemblies, colours). We test
it with a corpus of real vendor files. All of these files must load. If
Lamina cannot tessellate a face, it reports the face and skips it. The full
model does not fail. Lamina loads mesh formats that need no B-rep kernel
(OBJ from EasyEDA, glTF, 3MF) directly. There is no C++ kernel.

### 5.5 Geometry and DRC

Lamina uses `geo` and `i_overlay` for polygon work (clearance, pours,
union, offset). Copper pours are calculated geometry. Lamina keeps the
outlines and the fill result, and calculates them again when necessary.
DRC checks in version one:

- clearance for each layer and net class
- trace width
- via drill and annular ring
- hole to hole and hole to copper
- courtyard overlap
- copper in keep-outs
- copper to board edge
- mask and paste sanity
- unconnected pads
- shorts

The rules come from a fab preset (JLCPCB 2-layer and 4-layer in version
one). You can override them for each board and each net class.

### 5.6 Rendering

Two renderers use the same scene code in `lamina-core`:

- A raster renderer to PNG for the agent: any layer set, any region, with
  optional overlays (DRC markers, net highlight, ratsnest). With this
  renderer, the agent can see its work from the first day.
- The viewer draws the same scene with wgpu: 2D layers, each with its own
  on/off switch, and 3D with the board stack and the part meshes. Hot
  reload: the file watcher sends an event, Lamina parses the changed file
  again, and builds again only the GPU buffers that the change affects.

### 5.7 Routing

Version one has two methods:

- The agent routes by hand with `route_trace`. It gives points and a
  layer. The engine snaps the points to pads and reports each clearance or
  width violation. With `strict`, it refuses the violation, as for all
  mutations.
- A built-in grid router (A* or Lee type, two to four layers) routes the
  easy nets. It obeys the rules and keep-outs.

Freerouting as an external subprocess can be an option behind a
flag, but it is not necessary. The user does not want Java or other EDA
tools on the computer. A real native topological router is a later project.

### 5.8 Exports

Lamina writes these formats natively in Rust:

- Gerber X2 (RS-274X with attributes)
- Excellon drill, with plated and non-plated files and slots
- BOM and pick-and-place CSV in the JLCPCB format
- a STEP of the bare board outline (a full assembly STEP comes later)

Acceptance tests of the exporters, without other EDA tools on the computer:
`lamina-core` also contains a Gerber and Excellon reader. So the tests
export a board, read the files back, and compare the copper and hole
geometry with the model, within a tolerance. Golden files prevent
regressions. Before the first order, do a manual check in the online Gerber
viewer of JLCPCB. Then order the coupon board and check it.

### 5.9 Order of work

1. Core model, project files, schema, load and save, `lamina` CLI.
2. IPC-7351 footprint generator and `footprint_from_pads`; LCSC download;
   STEP download and tessellation.
3. Raster renderer and `render` tool. From this step, the agent can see.
4. Placement tools, DRC, connectivity.
5. Tools to route by hand, zones, keep-outs, grid router.
6. Exporters. Validate the output with a test coupon board.
7. Viewer: 2D with hot reload, then 3D.
8. Test coupon and reference board from start to end, exported for JLCPCB.

MCP wraps each core operation when it is available. So, from step 3, the
agent can use Lamina to make the test boards of Lamina.

### 5.10 First target

Lamina is its own project. It does not depend on a product board. The
first targets are:

- A test coupon: trace widths, clearances, via sizes, plated and
  non-plated holes, slots, all pad shapes.
- A reference board: a small USB-C microcontroller board (ESP32-C3 or
  RP2040 class), 2 or 4 layers, JLCPCB basic parts where possible. An agent
  makes all of it through MCP.

The model must also show all that a real product board needs: slots, NPTH,
keep-outs on any layer set, widths for each net, mask and paste overrides,
fiducials, and modules with antenna keep-outs.

## 6. Left to the builder

- The exact JSON schemas, in the file split above.
- Tool names and argument shapes, in the sketch above.
- The tessellation tolerance, the mesh format and the default PNG
  resolutions.
- The details and limits of the grid router algorithm.
- How the viewer selects the project to watch (argument, last opened).
- The test strategy. But each geometry and export function needs tests,
  and the Gerber output needs a golden-file comparison.

## 7. Decided after review

- Part data: LCSC/JLCPCB/EasyEDA, with footprint and model import (5.4).
- 3D: real models are mandatory, own STEP reader (5.4).
- Licence: MIT OR Apache-2.0.
- Lamina does not depend on the MeraTag project (5.10).

## 8. Open

- Do we publish on crates.io or not?

## 9. Modular sources and workspaces (2026-11-05)

The user asked for modularity of this type: "A in one file, B in a
different file, C = A + B in a third file". This is for projects with many
boards.

**Decision: Jsonnet sources, JSON layout.** Jsonnet is a superset of JSON
with variables, functions, `import` and object merge. It has a Rust
implementation (jrsonnet), so all of the code stays in Rust. We considered
these alternatives and did not use them:

- YAML: no imports or expressions. It needs a dialect.
- CUE and Pkl: not Rust.
- Nickel: Rust, but agents do not know it well enough to write it reliably.
- Starlark: the one real alternative. It has a Python syntax, but the
  board becomes a program that you must run to read it.

**The split.** You cannot edit formulas backwards. So a built board has
two types of files:

- *Design source* `design.jsonnet`: parts, nets, board (outline, holes,
  stackup, rules). The user or the agent edits it as text. It evaluates
  to an object with one or more of the keys `board`, `parts`, `nets`. For
  each key that is present, the source generates ("owns") that file.
- *Layout* `placement.json`, `routing.json`, `notes.json`: you edit them in
  the viewer and over MCP, as before.

`lamina build` evaluates the source into the usual JSON. The engine, DRC,
router, renderer, exporter and viewer do not change. Lamina refuses edits
to an owned file and shows the source file to edit. The MCP server and the
viewer build again automatically when the source or an imported file
changes (`cache/build.json` records the inputs and their hashes). If a part
is removed from the source, its placement is also removed.

**Workspaces.** `lamina.json` above the board folders lists `boards` and
`lib` folders. Each lib folder is a Jsonnet import path and can contain
shared `footprints/` and `models/`. If a board has a file in its own cache,
Lamina uses it before the libraries. `import 'lamina.libsonnet'` is built
in. It gives `merge` (each part must be unique, and nets with the same name
join their pads), `group`, `net`, `pads`, `rect`, `hole` and
`corner_holes`.

**Blocks** are plain Jsonnet functions that return fragments
`{parts, nets}`. The caller gives a name prefix and the names of the
external nets. `lamina.group(name, fragment)` sets `group` on each part of
an instance.

**Layout reuse** (`lamina-core/src/reuse.rs`). A block is a group or a
refdes list. The layout of a block has these items:

- The placement of its parts, relative to an anchor part.
- All tracks and vias of internal nets (nets with all pads in the block)
  that connect to its pads.
- On nets that go out of the block: the connected pieces that are near the
  parts and contain a via (fan-outs).

Other routes on external nets are different for each instance. Lamina does
not copy them and does not remove them.

A backtracking search matches a target to the block, part by part. A
candidate must have a footprint with numbered pads that align with the pads
of the block part after a quarter turn and a shift (0.1 mm). If two pads
share a net in the block, they must share a net in the target, and the
opposite is also true. The anchor of the target keeps its placement. Lamina
puts the other parts around it, turned and moved as their footprint fit
shows. Lamina removes the old block copper of the target, and adds the
copper of the block with the nets mapped.

You can save a layout as `layouts/<name>.json` (in the workspace library,
or in the board if there is no workspace) and place it on any board. Lamina
cannot mirror a block to the other side.

**Compatibility** with other PCB tools (KiCad, EasyEDA, Gerber, ODB++,
IPC-2581, Specctra, STEP): this decision has no effect on it. Import and
export occur at the JSON level and are a separate work item.
