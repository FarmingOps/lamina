//! Operations to use a layout again. They copy the layout of one block to
//! other blocks, and save or place block layouts as files (see `crate::reuse`).

use std::path::PathBuf;

use lamina_model::error::Result;
use lamina_model::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::project::{OpResult, Project, Touched};
use crate::reuse::{self, Applied, ApplyOpts, Block, LayoutBlock};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RepeatLayout {
    /// The block whose layout is copied. This is a group name or a list of refdes.
    pub from: Block,
    /// The blocks that get the layout. Each block is a group name or a list of
    /// refdes. Each block must have the same footprints, connected in the same
    /// way as `from`.
    pub to: Vec<Block>,
    /// A part that does not move. If it is in `from`, the part that matches it
    /// in each target does not move. If it is in a target, that part does not move.
    /// The default is the largest part of `from`.
    #[serde(default)]
    pub anchor: Option<String>,
    /// Put the anchor part of the target at this position instead (only for
    /// one target).
    #[serde(default)]
    pub at: Option<Placement>,
    /// Also copy the tracks and vias. They replace the old tracks and vias of
    /// the target (default true).
    #[serde(default = "super::yes")]
    pub routing: bool,
    /// Also move locked parts.
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SaveLayoutBlock {
    /// The block to save, after you lay it out. This is a group name or a
    /// list of refdes.
    pub from: Block,
    /// The file name without the extension, for example `servo_channel`.
    pub name: String,
    /// The anchor part of the layout (default: the largest part).
    #[serde(default)]
    pub anchor: Option<String>,
    /// Replace a saved layout that has the same name.
    #[serde(default)]
    pub replace: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PlaceLayoutBlock {
    /// The name of a saved layout (`layouts/<name>.json` in the board or in a
    /// workspace library).
    pub name: String,
    /// The blocks that get the layout. Each block is a group name or a list of
    /// refdes.
    pub to: Vec<Block>,
    /// A target part that does not move (default: the part that matches the
    /// anchor of the layout).
    #[serde(default)]
    pub anchor: Option<String>,
    /// Put the anchor part of the target at this position instead (only for
    /// one target).
    #[serde(default)]
    pub at: Option<Placement>,
    /// Also copy the tracks and vias (default true).
    #[serde(default = "super::yes")]
    pub routing: bool,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LayoutSaved {
    pub path: String,
    pub anchor: String,
    pub parts: usize,
    pub tracks: usize,
    pub vias: usize,
}

fn summary(what: &str, done: &[(String, Applied)]) -> (String, Vec<String>) {
    let mut lines = Vec::new();
    let mut notes = Vec::new();
    for (label, a) in done {
        let mut s = format!("{label}: {} parts around {}", a.parts, a.anchor);
        if a.tracks + a.vias + a.removed > 0 {
            s.push_str(&format!(", {} tracks, {} vias", a.tracks, a.vias));
            if a.removed > 0 {
                s.push_str(&format!(", {} old pieces removed", a.removed));
            }
        }
        lines.push(s);
        notes.extend(a.notes.iter().map(|n| format!("{label}: {n}")));
    }
    (format!("{what}: {}", lines.join("; ")), notes)
}

impl Project {
    fn block_parts(&self, b: &Block) -> Result<Vec<String>> {
        b.parts(&self.data).map_err(Error::invalid)
    }

    fn commit_layout(
        &mut self,
        d: ProjectData,
        done: Vec<(String, Applied)>,
        routing: bool,
        strict: bool,
        what: &str,
    ) -> Result<OpResult> {
        let mut t = Touched::file(FileKind::Placement);
        if routing {
            t = t.and(FileKind::Routing);
        }
        let (s, notes) = summary(what, &done);
        let mut r = self.commit(d, t, strict, s)?;
        r.notes.extend(notes);
        Ok(r)
    }

    pub fn repeat_layout(&mut self, p: RepeatLayout) -> Result<OpResult> {
        let from = self.block_parts(&p.from)?;
        let to: Vec<Vec<String>> =
            p.to.iter()
                .map(|b| self.block_parts(b))
                .collect::<Result<_>>()?;
        let mut d = self.data.clone();
        let o = ApplyOpts {
            at: p.at,
            routing: p.routing,
            force: p.force,
            ..ApplyOpts::default()
        };
        let done =
            reuse::repeat(&mut d, &from, &to, p.anchor.as_deref(), &o).map_err(Error::invalid)?;
        let done = p.to.iter().map(Block::label).zip(done).collect();
        let what = format!("layout of {} copied", p.from.label());
        self.commit_layout(d, done, p.routing, p.strict, &what)
    }

    /// The folder for a new saved layout: the first workspace library, or the
    /// board if there is no library.
    fn layout_save_dir(&self) -> PathBuf {
        let dirs = layout_dirs(self.root());
        dirs.get(1).unwrap_or(&dirs[0]).clone()
    }

    pub fn save_layout_block(&self, p: SaveLayoutBlock) -> Result<LayoutSaved> {
        if !valid_id(&p.name) {
            return Err(Error::invalid(format!(
                "The layout name `{}` is not valid. Use only letters, digits, `.`, `_` and `-`.",
                p.name
            )));
        }
        let parts = self.block_parts(&p.from)?;
        let b = reuse::extract(&self.data, &parts, p.anchor.as_deref(), &p.name)
            .map_err(Error::invalid)?;
        let dir = self.layout_save_dir();
        let path = dir.join(format!("{}.json", p.name));
        if path.exists() && !p.replace {
            return Err(Error::invalid(format!(
                "{} already exists. Set replace: true to replace it.",
                path.display()
            )));
        }
        std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        let text = json::to_string(&b).map_err(|e| Error::Json {
            path: path.clone(),
            source: e,
        })?;
        write_atomic(&path, text.as_bytes())?;
        Ok(LayoutSaved {
            path: path.display().to_string(),
            anchor: b.anchor.clone(),
            parts: b.parts.len(),
            tracks: b.tracks.len(),
            vias: b.vias.len(),
        })
    }

    /// Get a saved layout by its name, from `layouts/` in the board or in a library.
    pub fn load_layout_block(&self, name: &str) -> Result<LayoutBlock> {
        let dirs = layout_dirs(self.root());
        for d in &dirs {
            let f = d.join(format!("{name}.json"));
            if f.is_file() {
                let text = std::fs::read_to_string(&f).map_err(|e| Error::io(&f, e))?;
                return serde_json::from_str(&text)
                    .map_err(|e| Error::invalid(format!("{}: {e}", f.display())));
            }
        }
        let mut known: Vec<String> = dirs
            .iter()
            .filter_map(|d| std::fs::read_dir(d).ok())
            .flat_map(|rd| rd.flatten())
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.strip_suffix(".json").map(str::to_string)
            })
            .collect();
        known.sort();
        known.dedup();
        Err(Error::invalid(if known.is_empty() {
            format!("There is no saved layout `{name}`. Save one with save_layout_block.")
        } else {
            format!(
                "There is no saved layout `{name}`. The saved layouts are: {}.",
                known.join(", ")
            )
        }))
    }

    pub fn place_layout_block(&mut self, p: PlaceLayoutBlock) -> Result<OpResult> {
        let mut b = self.load_layout_block(&p.name)?;
        let to: Vec<Vec<String>> =
            p.to.iter()
                .map(|b| self.block_parts(b))
                .collect::<Result<_>>()?;
        if to.is_empty() {
            return Err(Error::invalid(
                "No target block is given. Give at least one block in `to`.",
            ));
        }
        if p.at.is_some() && to.len() > 1 {
            return Err(Error::invalid(
                "If you give `at`, you must give exactly one target.",
            ));
        }
        if let Some(a) = &p.anchor {
            let t = to.iter().find(|t| t.contains(a)).ok_or_else(|| {
                Error::invalid(format!("The anchor {a} is not in a target block."))
            })?;
            let m = reuse::match_block(&self.data, &b, t).map_err(Error::invalid)?;
            let local = m
                .parts
                .iter()
                .find(|(_, r)| *r == a)
                .map(|(k, _)| k.clone())
                .ok_or_else(|| Error::invalid("No part in the layout matches the anchor."))?;
            b = b.rebase(&local).map_err(Error::invalid)?;
        }
        let o = ApplyOpts {
            at: p.at,
            routing: p.routing,
            force: p.force,
            ..ApplyOpts::default()
        };
        let mut d = self.data.clone();
        let mut done = Vec::new();
        for (blk, t) in p.to.iter().zip(&to) {
            let a = reuse::apply(&mut d, &b, t, &o).map_err(Error::invalid)?;
            done.push((blk.label(), a));
        }
        let what = format!("layout {} placed", p.name);
        self.commit_layout(d, done, p.routing, p.strict, &what)
    }
}
