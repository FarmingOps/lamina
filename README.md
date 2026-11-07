<p align="center"><img src="assets/logo.svg" width="72" alt="Lamina logo"></p>

# Lamina

Lamina is a PCB layout program that an AI coding agent operates through MCP tools, while you watch the board in a live viewer and edit it yourself when you want.

- `lamina` is an MCP server. An agent uses its tools to add parts and nets, place the
  parts, route the tracks, do a DRC and export the fab files.
- A board is a folder of JSON files. You can read and change these files in a text editor.
- `lamina-viewer` shows the board in 2D and 3D. When a file changes, the viewer shows
  the change. You can also edit the board in the viewer.

Lamina is in early development. The design document is in `docs/specs/`.

<p align="center"><img src="docs/images/board-3d.png" width="760" alt="3D view of an ESP32-S3 servo board"></p>

<table>
<tr>
<td><img src="docs/images/board-top.png" alt="Top side of the board"></td>
<td><img src="docs/images/board-layers.png" alt="Copper layers, silkscreen and drills"></td>
</tr>
<tr>
<td align="center">Top side</td>
<td align="center">Copper, silkscreen and drills</td>
</tr>
</table>

An agent made this board with the Lamina tools. It is an ESP32-S3 servo controller with
26 parts on 2 copper layers. The `render` tool made the images.

## Crates

| Crate | Contents |
|---|---|
| `lamina-model` | Data types, JSON schema, load and save of a project |
| `lamina-geom` | Polygon operations, spatial index |
| `lamina-footprint` | IPC-7351 footprints, footprints from a datasheet |
| `lamina-parts` | Part search and import from LCSC, JLCPCB and EasyEDA |
| `lamina-step` | STEP reader and tessellator, OBJ reader, mesh cache |
| `lamina-drc` | Design rules, fab presets, DRC, connectivity |
| `lamina-route` | Track routing, pour fill, autorouter |
| `lamina-export` | Gerber X2, Excellon, BOM, CPL, board STEP |
| `lamina-scene` | Shapes to draw, for the 2D and 3D renderers |
| `lamina-render` | PNG images in 2D and 3D |
| `lamina-build` | Jsonnet board sources, workspaces |
| `lamina-kicad` | KiCad footprint and board import and export |
| `lamina-core` | Project operations, one function for each tool |
| `lamina-mcp` | The `lamina` program: MCP server and CLI |
| `lamina-viewer` | The `lamina-viewer` program: 2D and 3D viewer and editor |

`vendor/jrsonnet-stdlib` is a copy of the Jsonnet standard library without YAML and
wasm-bindgen. The pinned versions of these two do not agree with egui and wgpu.

## Build

```
cargo build --release
```

## Connect an MCP client

Claude Code:

```
claude mcp add --scope user lamina -- <path>/target/release/lamina mcp
```

Codex, in `~/.codex/config.toml`:

```toml
[mcp_servers.lamina]
command = "<path>/target/release/lamina"
args = ["mcp"]
```

An agent usually calls the tools in this order:

1. `open` with `create: true`
2. `use_fab_preset`
3. `set_outline`
4. `import_lcsc` for each part
5. `set_nets`
6. `place_many`
7. `autoroute`
8. `add_zone` for the ground pour
9. `check`
10. `render`
11. `export`

The `notes` tool gives the agent the notes that you type in the viewer.

## Viewer

```
lamina-viewer [--3d] [project folder or board file]...
```

- You can open more than one board. The boards show side by side in one window.
- `--3d` starts in the 3D view.
- The layer panel, the inspector, the status bar and the shortcuts apply to the board
  that you clicked last.
- If you give no arguments, the viewer opens the boards from the last time.

To open a board, click `+` in the top bar or push `Ctrl+O`. You can also drop a folder or a
file on the window. The viewer opens Lamina project folders and KiCad, EasyEDA and Gerber
files. It imports a file from a different program into a project in the local data folder.
If that file changes, the viewer imports it again.

Keys:

- Click to select a part. Drag to move it. Drag a box to select more than one item.
- `R` rotate, `F` flip, `L` lock, `M` move, `Del` delete. The arrow keys move the
  selection a small distance.
- `X` route a track. `V` adds a via and changes the layer. `/` changes the bend.
- `V` via, `Z` pour, `K` keep-out, `O` outline.
- `D` measure. The measurement snaps to the centres of pads and vias.
- `N` grid. `Ctrl+Z` undo, `Ctrl+Y` redo.
- `Ctrl+F` find a part, a value or a net.
- Right-drag or `Space`+drag to pan. Scroll to zoom. `1` 2D view, `2` 3D view.
- `F1` shows all the shortcuts.

The Board menu has these commands:

- Autoroute the full board, or only the nets of the selection. You can undo it in one step.
- Rip up the tracks.
- Export the fab files (Gerber, drill, BOM, CPL), a STEP assembly or a KiCad board.

The inspector shows the values of the selected item. You can change them there.

The viewer and an agent can change the same board at the same time. Before each change,
each of them reads the files again, so neither writes over a change of the other.

## Autorouter

The `autoroute` tool and the Board menu route all open connections, or only the nets that
you name. You do not need a different program.

- The router routes each net as a tree. A tree can start or stop on any copper of the net,
  also on a pour.
- At first, tracks of different nets can overlap. After each pass, the cost of a shared
  cell increases. The router then routes the overlapped nets again, until no nets overlap.
  This method is PathFinder.
- If a connection fails at the width of its net class, the router tries the minimum width.
  If it fails again, the router removes the tracks near it. Then it routes that area again
  on a grid that is two times finer.
- The router routes the nets that have a pour last. Then it fills the pours again and
  routes the connections that are still open.
- The router obeys keep-outs, hole-to-hole clearance, the width and clearance of each net
  class, and the layers that it must not use.

Test: two real KiCad boards, with all tracks removed and the pours kept. The router
connected 58 of 58 and 94 of 94 connections, in about 12 seconds for each board. The DRC
found 0 errors. One of the boards has a USB-C D+/D- crossover between 0.5 mm pads.

To do the same test on a different board:

```
cargo run --release -p lamina-route --example bench -- <project>
```

## Modular boards

You can write a board in Jsonnet (`design.jsonnet`) instead of JSON. Jsonnet is JSON with
variables, functions and `import`. You can write a circuit block one time and use it
many times:

```jsonnet
local lamina = import 'lamina.libsonnet';
local servo = import 'servo.libsonnet';
lamina.merge([
  { board: { name: 'demo', outline: lamina.rect(64, 52, 3), rules: { preset: 'jlcpcb-2layer' } } },
  servo.channel('J2', 'R8', { pwm: 'PWM0', out: 'SERVO1', vservo: 'VSERVO', gnd: 'GND' }),
  servo.channel('J3', 'R9', { pwm: 'PWM1', out: 'SERVO2', vservo: 'VSERVO', gnd: 'GND' }),
])
```

`servo.channel` is not part of Lamina. It is a function in the `lib` folder of the board.
You write functions like this for your own circuits.

`lamina build <board>` makes `board.json`, `parts.json` and `nets.json` from the source.
When the source changes, the MCP server and the viewer build the board again. Do not edit
these three files. Lamina refuses edits to them. You edit the placement and the routing
with the tools or in the viewer, as usual.

`lamina.libsonnet` is part of Lamina. It has these functions:

- `merge` joins blocks into one board. A refdes must be unique. Nets with the same name
  join their pads.
- `group` gives the parts of a block a group name.
- `net`, `pads`, `rect`, `hole` and `corner_holes`.

More than one board can use the same blocks in a workspace. `lamina workspace <dir>` makes
`lamina.json`, with the `boards` and `lib` folders. Each `lib` folder can also have shared
`footprints/` and `models/`. `lamina new <dir> --modular` makes a board from a template.

## Copy the layout of a block

Put each instance of a block in a group. Make the layout of one instance. Then copy that
layout to the other instances:

```jsonnet
lamina.group('servo1', servo.channel('J2', 'R8', { pwm: 'PWM0', out: 'SERVO1', vservo: 'VSERVO', gnd: 'GND' })),
lamina.group('servo2', servo.channel('J3', 'R9', { pwm: 'PWM1', out: 'SERVO2', vservo: 'VSERVO', gnd: 'GND' })),
```

```
lamina repeat-layout boards/servo --from servo1 --to servo2 --anchor J3
```

Lamina pairs the parts by footprint and nets, not by name. A footprint from a different
library also pairs if its pads are at the same positions. Different pad numbers or a
different origin are not a problem. The anchor part does not move. Lamina puts the other
parts of the block around it.

Lamina also copies these tracks and vias:

- All tracks and vias of a net that is fully in the block.
- Short tracks to a via near the parts, on nets that go out of the block.

Lamina does not change the tracks to other parts of the board. After the copy, look at
the ratsnest. You can also give a block as a list of refdes (`--from D1,R1`).

To keep a layout for other boards:

- `lamina save-layout <board> --from led1 --name led_indicator` writes the layout to
  `layouts/led_indicator.json` in the workspace library.
- `lamina place-layout <board> --name led_indicator --to led2` puts it on a block of
  any board in the workspace.

The MCP tools are `repeat_layout`, `save_layout_block` and `place_layout_block`. In the
viewer, select a part of a group. Then use *Select group*, *layout like* or *Copy layout
to all like it*.

## KiCad files

Lamina reads and writes KiCad files. You do not need KiCad.

- `import_kicad_footprint` imports a footprint. Give `Library:Name` (for example
  `Package_TO_SOT_SMD:SOT-23`), a name from a local library, or a `.kicad_mod` file.
  Lamina finds local libraries through `kicad_footprints` in `lamina.json`,
  `KICAD*_FOOTPRINT_DIR` or a KiCad install. If there is no local library, Lamina downloads
  the footprint and its STEP model from the official KiCad libraries.
  `search_kicad_footprints` searches the local libraries.
- `import_kicad_board` and `lamina import-kicad <file> <dir>` import a `.kicad_pcb` from
  KiCad 6 to 10. Lamina imports the outline, footprints, parts, nets, placement, tracks,
  vias, zones, keep-outs and texts. The design rules and net classes come from the
  `.kicad_pro` file next to the board. Lamina finds `${KIPRJMOD}` models next to the board.
  `--online` downloads the other models from the official libraries.
- `export_kicad` and `lamina export-kicad <dir>` write a `.kicad_pcb` for KiCad 8 or
  later. KiCad fills the zones again when it opens the file.

## EasyEDA boards

`import_easyeda_board` and `lamina import-easyeda <file.json> <dir>` import a board from the
standard EasyEDA editor. In EasyEDA, use File → Export → EasyEDA to save the file.

Lamina imports the outline, the parts with their LCSC numbers, nets, placement, tracks,
vias, copper areas, holes and texts. Lamina downloads the 3D models from EasyEDA. Solid
copper shapes, for example teardrops, stay as drawings. Lamina makes the design rules
less strict to agree with the board. After the import, apply a fab preset.

## Gerber files

`import_gerbers` and `lamina import-gerbers <folder or zip> <dir>` import Gerber and
Excellon files from any CAD program. Lamina finds the layer of each file from its X2
attributes or from its name (KiCad, Altium, EasyEDA and Eagle names).

Gerber files have no parts and no nets, so the board has only the outline, the holes,
and the copper and silkscreen as drawings. Use it to look at a board, to render it or to
compare it with a different version.

## STEP export

`export_step` and `lamina export-step <dir>` write the board and its parts as one STEP file
(AP214). You can open it in Fusion, FreeCAD, SolidWorks and other CAD programs, for example
to design an enclosure.

- The board is a solid with its outline, cut-outs and holes. `vias` also adds the via holes.
- Each part is a component at its position. Lamina copies the STEP file of each part without
  changes, so the shapes, colours and names stay the same.
- A part that has only an OBJ model goes in as flat faces.

## Other autorouters

Lamina does not need a different router. To compare with a different router, export the
board as a Specctra DSN file and import the result:

- `export_specctra` and `lamina export-dsn <dir>` write the DSN file.
- `import_specctra_session` and `lamina import-ses <file.ses> <dir>` read the session file.

With [Freerouting](https://github.com/freerouting/freerouting):

```
lamina export-dsn boards/servo
java -jar freerouting.jar -de boards/servo/output/esp32-servo.dsn -do boards/servo/output/esp32-servo.ses
lamina import-ses boards/servo/output/esp32-servo.ses boards/servo
```

The DSN file has the existing tracks as fixed tracks. If you set `reroute_all`, Lamina does
not put them in the file.

## Developer tools

- `cargo run -p lamina-export --example outline -- <file.gbr> [tolerance]` shows the closed
  loops in a Gerber outline file.
- `cargo run --release -p lamina-step --example corpus -- <dir>` loads all STEP files in a
  folder. It compares the size of each one with the OBJ file that has the same name.
- `cargo run -p lamina-render --example meshview -- <model> <out.png>` renders one model.

## Licence

MIT OR Apache-2.0.
