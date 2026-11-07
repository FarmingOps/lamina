use std::path::Path;

use lamina_model::error::Result;
use lamina_model::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::project::{OpResult, Project, Touched};

pub use lamina_parts::provider::today;

fn read_cached(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Compare pad numbers so that 2 < 10 and A2 < A10.
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    fn split(s: &str) -> (String, u64) {
        let p = s.trim_end_matches(|c: char| c.is_ascii_digit());
        (p.to_string(), s[p.len()..].parse().unwrap_or(0))
    }
    let (pa, na) = split(a);
    let (pb, nb) = split(b);
    pa.cmp(&pb).then(na.cmp(&nb)).then(a.cmp(b))
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImportEasyedaBoard {
    /// The path of the EasyEDA PCB JSON file (File > Export > EasyEDA in the
    /// standard editor).
    pub path: String,
    /// Download the 3D models of the parts from EasyEDA (default true).
    #[serde(default = "super::yes")]
    pub models: bool,
    /// Replace the board if the project already has one.
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub strict: bool,
}

impl Project {
    /// Put an EasyEDA 3D model into `cache/models/lcsc/`. Use the STEP file if it loads.
    pub(crate) fn fetch_easyeda_model(
        &self,
        info: &lamina_parts::ModelInfo,
        refresh: bool,
        step: bool,
        notes: &mut Vec<String>,
    ) -> Option<(ModelRef, Option<f64>)> {
        let root = self.root().to_path_buf();
        let rel_obj = format!("lcsc/{}.obj", info.uuid);
        let obj_path = root.join(MODEL_DIR).join(&rel_obj);
        let lib = self.library();
        let lib_obj = format!("models/{rel_obj}");
        let obj = match (!refresh).then(|| read_cached(&obj_path)).flatten() {
            Some(t) => t,
            None => match (!refresh)
                .then(|| lib.find(&lib_obj))
                .flatten()
                .and_then(|p| read_cached(&p))
            {
                Some(t) => {
                    if let Err(e) = write_atomic(&obj_path, t.as_bytes()) {
                        notes.push(format!("The 3D model was not saved: {e}"));
                        return None;
                    }
                    t
                }
                None => match self.source().model_obj(&info.uuid) {
                    Ok(t) => {
                        if let Err(e) = write_atomic(&obj_path, t.as_bytes()) {
                            notes.push(format!("The 3D model was not saved: {e}"));
                            return None;
                        }
                        let _ = lib.put(&lib_obj, t.as_bytes());
                        t
                    }
                    Err(e) => {
                        notes.push(format!("The 3D model was not downloaded: {e}"));
                        return None;
                    }
                },
            },
        };
        let mut model = lamina_parts::model_ref(info, &obj, rel_obj);
        let height = lamina_parts::model_height(info, &obj);
        notes.push(format!("3D model `{}`", info.title));
        if step {
            let rel_step = format!("lcsc/{}.step", info.uuid);
            let step_path = root.join(MODEL_DIR).join(&rel_step);
            let lib_step = format!("models/{rel_step}");
            if refresh || !step_path.exists() {
                let from_lib = (!refresh).then(|| lib.find(&lib_step)).flatten();
                match from_lib {
                    Some(src) => {
                        if let Some(parent) = step_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if let Err(e) = std::fs::copy(&src, &step_path) {
                            notes.push(format!("The STEP file was not copied: {e}"));
                        }
                    }
                    None => match self.source().model_step(&info.uuid) {
                        Ok(b) => {
                            if let Err(e) = write_atomic(&step_path, &b) {
                                notes.push(format!("The STEP file was not saved: {e}"));
                            }
                            let _ = lib.put(&lib_step, &b);
                        }
                        Err(e) => notes.push(format!("The STEP file was not downloaded: {e}")),
                    },
                }
            }
            // The STEP file has the same geometry in the same frame, but with
            // smooth surfaces. Use it if it loads.
            if step_path.exists() {
                match self.load_mesh(&step_path) {
                    Ok(_) => model.file = rel_step,
                    Err(e) => notes.push(format!(
                        "The STEP file did not load, so Lamina uses the OBJ file: {e}"
                    )),
                }
            }
        }
        Some((model, height))
    }

    pub fn import_easyeda_board(&mut self, p: ImportEasyedaBoard) -> Result<OpResult> {
        let path = Path::new(&p.path);
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| Error::Json {
            path: path.to_path_buf(),
            source: e,
        })?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("board")
            .to_string();
        let cur = &self.data;
        let has_board = !cur.parts.parts.is_empty()
            || !cur.routing.tracks.is_empty()
            || cur.board.outline.is_some();
        if has_board && !p.replace {
            return Err(Error::Refused(
                "The project already has a board. Set replace: true to replace it.".into(),
            ));
        }
        let imported = lamina_parts::board::import_board(&doc, &name).map_err(Error::Invalid)?;
        let mut notes = imported.notes;
        let mut d = imported.data;
        d.notes = self.data.notes.clone();
        if p.models {
            let mut found = 0;
            for (fp_id, info) in &imported.models {
                let mut mnotes = Vec::new();
                if let Some((m, h)) = self.fetch_easyeda_model(info, false, true, &mut mnotes) {
                    if let Some(fp) = d.footprints.get_mut(fp_id) {
                        fp.model = Some(m);
                        fp.height = h;
                        found += 1;
                    }
                } else {
                    notes.extend(mnotes);
                }
            }
            notes.push(format!("3D models: {found} of {}", imported.models.len()));
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
        let summary = format!(
            "imported {}: {} parts, {} nets, {} tracks, {} vias, {} zones",
            path.display(),
            d.parts.parts.len(),
            d.nets.nets.len(),
            d.routing.tracks.len(),
            d.routing.vias.len(),
            d.routing.zones.len()
        );
        let mut res = self.commit(d, touched, p.strict, summary)?;
        res.notes = notes;
        Ok(res)
    }
}
