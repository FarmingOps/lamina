//! Board files of other programs, and the exchange with external routers.
//! The formats and the routers are ports. The core only picks one, reads or
//! writes through it, and commits the result.

use std::path::{Path, PathBuf};

use lamina_model::error::Result;
use lamina_model::*;
use lamina_ports::format::{BoardFormat, FormatInfo};
use lamina_ports::router::{RouterExchange, RouterInfo};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::project::{OpResult, Project, Touched, next_id};

/// The folder in the project that keeps the files a format read but did
/// not convert.
pub const IMPORT_DIR: &str = "import";

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImportBoard {
    /// The board file, or the folder or zip of files.
    pub path: String,
    /// The format id. The default is the format that detects the file.
    /// The `formats` tool lists them.
    #[serde(default)]
    pub format: Option<String>,
    /// Get the 3D models of the footprints from the providers (default
    /// true). This can use the network.
    #[serde(default = "super::yes")]
    pub models: bool,
    /// Replace the board if the project already has one.
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExportBoard {
    /// The format id, for example `kicad`. The `formats` tool lists them.
    pub format: String,
    /// The output file. The default is `output/<board name>.<extension>`
    /// in the project.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BoardExported {
    pub path: String,
    pub format: String,
    pub parts: usize,
    pub nets: usize,
    pub tracks: usize,
    pub zones: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExportForRouter {
    /// The router id, for example `specctra`. The `routers` tool lists them.
    pub router: String,
    /// The output file. The default is `output/<board name>.<extension>`
    /// in the project.
    #[serde(default)]
    pub path: Option<String>,
    /// If true, do not export the tracks and vias that are on the board, so
    /// the router starts with no wiring. The default (false) exports them as
    /// protected wiring.
    #[serde(default)]
    pub reroute_all: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RouterExported {
    pub path: String,
    pub router: String,
    pub nets: usize,
    pub parts: usize,
    /// What to do with the file.
    pub hint: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImportRouterSession {
    /// The router id, for example `specctra`.
    pub router: String,
    /// The path of the session file that the router wrote.
    pub path: String,
    /// If true (default), replace the tracks and vias of the routed nets.
    /// If false, add the routes of the session to them.
    #[serde(default = "super::yes")]
    pub replace: bool,
    #[serde(default)]
    pub strict: bool,
}

fn file_stem(p: &Path) -> String {
    p.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("board")
        .to_string()
}

impl Project {
    /// The board formats that are plugged in.
    pub fn formats(&self) -> Vec<FormatInfo> {
        self.ports().formats.iter().map(|f| f.info()).collect()
    }

    /// The router exchanges that are plugged in.
    pub fn routers(&self) -> Vec<RouterInfo> {
        self.ports().routers.iter().map(|r| r.info()).collect()
    }

    fn format_ids(&self) -> String {
        self.formats()
            .iter()
            .map(|f| f.id.clone())
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn need_format(&self, id: &str) -> Result<&dyn BoardFormat> {
        self.ports().format(id).ok_or_else(|| {
            Error::invalid(format!(
                "There is no format `{id}`. The formats are: {}.",
                self.format_ids()
            ))
        })
    }

    fn need_router(&self, id: &str) -> Result<&dyn RouterExchange> {
        self.ports().router(id).ok_or_else(|| {
            let ids: Vec<String> = self.routers().iter().map(|r| r.id.clone()).collect();
            Error::invalid(format!(
                "There is no router `{id}`. The routers are: {}.",
                ids.join(", ")
            ))
        })
    }

    /// Read a board file of another program into the project. The project
    /// must be empty, or `replace` must be true.
    pub fn import_board(&mut self, p: ImportBoard) -> Result<OpResult> {
        let path = Path::new(&p.path);
        let format = match &p.format {
            Some(id) => self.need_format(id)?,
            None => self.ports().format_for(path).ok_or_else(|| {
                Error::invalid(format!(
                    "{} is not a board file that Lamina can read. The formats are: {}.",
                    path.display(),
                    self.format_ids()
                ))
            })?,
        };
        let info = format.info();
        if !info.read {
            return Err(Error::invalid(format!(
                "The format `{}` cannot read.",
                info.id
            )));
        }
        let cur = &self.data;
        let has_board = !cur.parts.parts.is_empty()
            || !cur.routing.tracks.is_empty()
            || !cur.routing.graphics.is_empty()
            || cur.board.outline.is_some();
        if has_board && !p.replace {
            return Err(Error::Refused(
                "The project already has a board. Set replace: true to replace it.".into(),
            ));
        }
        let name = file_stem(&format.source_root(path));
        let imported = format.read(path, &name).map_err(Error::Invalid)?;
        let mut notes = imported.notes;
        let mut d = imported.data;
        d.notes = self.data.notes.clone();
        if p.models && !imported.models.is_empty() {
            let mut found = 0;
            for (fp_id, wanted) in &imported.models {
                let Some(fp) = d.footprints.get_mut(fp_id) else {
                    continue;
                };
                // The first request that works wins. The notes of the failed
                // requests stay only if none works.
                let mut tries = Vec::new();
                let mut done = false;
                for name in wanted {
                    match self.provider_model(name) {
                        Ok((m, h)) => {
                            notes.push(format!("3D model {}", m.file));
                            fp.model = Some(m);
                            fp.height = h;
                            found += 1;
                            done = true;
                            break;
                        }
                        Err(e) => tries.push(format!("{fp_id}: {e}")),
                    }
                }
                if !done {
                    tries.dedup();
                    notes.extend(tries);
                }
            }
            notes.push(format!(
                "3D models: {found} of {} resolved",
                imported.models.len()
            ));
        }
        let mut extra_files = Vec::new();
        for (file, bytes) in &imported.extras {
            if file.contains("..") || file.contains('/') || file.contains('\\') {
                notes.push(format!("The extra file `{file}` was not written."));
                continue;
            }
            let rel = format!("{IMPORT_DIR}/{file}");
            write_atomic(&self.root().join(&rel), bytes)?;
            extra_files.push(rel);
        }
        let mut touched = Touched::default();
        for k in [
            FileKind::Board,
            FileKind::Parts,
            FileKind::Nets,
            FileKind::Placement,
            FileKind::Routing,
        ] {
            touched.files.insert(k);
        }
        for id in d.footprints.keys() {
            touched.footprints.insert(id.clone());
        }
        for id in self.data.footprints.keys() {
            if !d.footprints.contains_key(id) {
                touched.removed_footprints.insert(id.clone());
            }
        }
        let summary = if d.parts.parts.is_empty() && !d.routing.graphics.is_empty() {
            format!(
                "imported {} ({}): {} copper layer(s), {} drawings, {} holes",
                path.display(),
                info.id,
                d.board.stackup.copper_layers,
                d.routing.graphics.len(),
                d.board.holes.len()
            )
        } else {
            format!(
                "imported {} ({}): {} parts, {} nets, {} tracks, {} vias, {} zones",
                path.display(),
                info.id,
                d.parts.parts.len(),
                d.nets.nets.len(),
                d.routing.tracks.len(),
                d.routing.vias.len(),
                d.routing.zones.len()
            )
        };
        let mut res = self.commit(d, touched, p.strict, summary)?;
        res.changed.extend(extra_files);
        res.notes = notes;
        Ok(res)
    }

    /// Write the board as a file of another program.
    pub fn export_board(&self, p: ExportBoard) -> Result<BoardExported> {
        let format = self.need_format(&p.format)?;
        let info = format.info();
        if !info.write {
            return Err(Error::invalid(format!(
                "The format `{}` cannot write.",
                info.id
            )));
        }
        let ext = info.extensions.first().cloned().unwrap_or_default();
        let path = match p.path {
            Some(s) => PathBuf::from(s),
            None => self
                .root()
                .join("output")
                .join(format!("{}.{ext}", self.data.board.name)),
        };
        let text = format
            .write(&self.data, &self.root().join(MODEL_DIR))
            .map_err(Error::Invalid)?;
        write_atomic(&path, text.as_bytes())?;
        Ok(BoardExported {
            path: path.display().to_string(),
            format: info.id,
            parts: self.data.parts.parts.len(),
            nets: self.data.nets.nets.len(),
            tracks: self.data.routing.tracks.len(),
            zones: self.data.routing.zones.len(),
        })
    }

    /// Write the design file for an external router.
    pub fn export_for_router(&self, p: ExportForRouter) -> Result<RouterExported> {
        let router = self.need_router(&p.router)?;
        let info = router.info();
        let path = match p.path {
            Some(s) => PathBuf::from(s),
            None => self.root().join("output").join(format!(
                "{}.{}",
                self.data.board.name, info.design_extension
            )),
        };
        if self.data.board.outline.is_none() {
            return Err(Error::invalid(
                "The board outline is not set. Set the outline before you export for a router.",
            ));
        }
        let text = router
            .export(&self.data, &self.data.board.name, p.reroute_all)
            .map_err(Error::Invalid)?;
        write_atomic(&path, text.as_bytes())?;
        Ok(RouterExported {
            path: path.display().to_string(),
            router: info.id.clone(),
            nets: self.data.nets.nets.len(),
            parts: self.data.placement.placements.len(),
            hint: format!(
                "Route the file with the router ({}). Then call import_router_session with \
                 router `{}` and the `.{}` file it wrote.",
                info.description, info.id, info.session_extension
            ),
        })
    }

    /// Read the session that an external router wrote, and put its tracks
    /// and vias on the board.
    pub fn import_router_session(&mut self, p: ImportRouterSession) -> Result<OpResult> {
        let router = self.need_router(&p.router)?;
        let path = Path::new(&p.path);
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        let session = router
            .import_session(&text, &self.data)
            .map_err(Error::Invalid)?;
        let mut d = self.data.clone();
        let mut removed = 0;
        if p.replace {
            let before = d.routing.tracks.len() + d.routing.vias.len();
            d.routing.tracks.retain(|t| !session.nets.contains(&t.net));
            d.routing.vias.retain(|v| !session.nets.contains(&v.net));
            removed = before - d.routing.tracks.len() - d.routing.vias.len();
        }
        let mut ids: Vec<String> = d
            .routing
            .tracks
            .iter()
            .map(|t| t.id.clone())
            .chain(d.routing.vias.iter().map(|v| v.id.clone()))
            .collect();
        let (nt, nv) = (session.tracks.len(), session.vias.len());
        let min_w = d.board.rules.min_track_width;
        let mut widened = 0;
        for mut t in session.tracks {
            // Routers can return widths that are one rounding step less than the minimum.
            if t.width < min_w && t.width >= min_w * 0.9 {
                t.width = min_w;
                widened += 1;
            }
            t.id = next_id("t", ids.iter().map(String::as_str));
            ids.push(t.id.clone());
            d.routing.tracks.push(t);
        }
        for mut v in session.vias {
            v.id = next_id("v", ids.iter().map(String::as_str));
            ids.push(v.id.clone());
            d.routing.vias.push(v);
        }
        let mut res = self.commit(
            d,
            Touched::file(FileKind::Routing),
            p.strict,
            format!(
                "session routed {} net(s): {nt} track(s) and {nv} via(s) added, {removed} replaced",
                session.nets.len()
            ),
        )?;
        res.notes = session.notes;
        if widened > 0 {
            res.notes.push(format!(
                "{widened} track(s) came back a little narrower than the minimum width. \
                 They are now {min_w} mm wide."
            ));
        }
        Ok(res)
    }
}
