## Changes in 0.2.0

- Providers: parts, footprints and 3D models come from providers. A part is `provider:id`, for example `lcsc:C6186` or `kicad:LED_SMD:LED_0603_1608Metric`. The tools are `providers`, `search_parts`, `import_part` and `import_parts`. The vendor-named tools are gone.
- Part library: each download also goes into a library on your computer (`LAMINA_LIB`, `%LOCALAPPDATA%\lamina\lib` or `~/.lamina/lib`). Imports look in the project cache, then in the library, then online. `lamina fetch` fills the library before you work without a network.
- Board formats and routers: `formats`, `import_board`, `export_board`, `routers`, `export_for_router` and `import_router_session` replace the format-named tools.
- Fabrication: `fabricators` lists the outputs; `export` takes one by id.
- 3D view: `render` can focus a part, a pad or a region, zoom, hide parts and show only some parts.
- Architecture: the core talks to the outside world through ports only. A new source, format or output is one type plus one line in `lamina-wiring`.

Each archive has two programs, the README and the licence files:

- `lamina`: the MCP server and the command line tool.
- `lamina-viewer`: the 2D and 3D viewer and editor.

| Platform | Archive |
|---|---|
| Windows x64 | `x86_64-pc-windows-msvc.zip` |
| Windows Arm64 | `aarch64-pc-windows-msvc.zip` |
| Linux x64 | `x86_64-unknown-linux-gnu.tar.gz` |
| Linux Arm64 | `aarch64-unknown-linux-gnu.tar.gz` |
| macOS Apple silicon | `aarch64-apple-darwin.tar.gz` |
| macOS Intel | `x86_64-apple-darwin.tar.gz` |

Each archive has a `.sha256` file with its checksum.

## Install

1. Download the archive for your platform and extract it.
2. Put the folder in a permanent location.
3. Connect your MCP client to `lamina mcp`. The README shows the commands for Claude Code and Codex.

## Known limits

- The programs are not signed.
  - On Windows, SmartScreen can show a warning. Click "More info", then "Run anyway".
  - On macOS, remove the quarantine flag before the first start: `xattr -d com.apple.quarantine lamina lamina-viewer`.
- On Linux, the viewer needs a Vulkan or OpenGL driver and an X11 or Wayland session. The Linux builds need glibc 2.35 or later.
