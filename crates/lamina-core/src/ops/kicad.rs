//! Import and export of KiCad footprints and boards.

use std::path::{Path, PathBuf};

use lamina_kicad::library;
use lamina_model::error::Result;
use lamina_model::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::project::{OpResult, Project, Touched};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImportKicadBoard {
    /// The path of the `.kicad_pcb` file.
    pub path: String,
    /// Find the 3D models in the local KiCad libraries (default true).
    #[serde(default = "super::yes")]
    pub models: bool,
    /// Also download the STEP models that are not found locally (default false).
    /// This is slow for large boards.
    #[serde(default)]
    pub online: bool,
    /// Replace the board if the project already has one.
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExportKicad {
    /// The output file. The default is `output/<board name>.kicad_pcb` in the project.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct KicadExported {
    pub path: String,
    pub parts: usize,
    pub nets: usize,
    pub tracks: usize,
    pub zones: usize,
}

/// KiCad model roots: `kicad_models` of the workspace, then the KiCad installation.
fn model_dirs(root: &Path) -> Vec<PathBuf> {
    let mut v = Workspace::find(root)
        .map(|w| w.kicad_model_dirs())
        .unwrap_or_default();
    v.extend(library::env_model_dirs());
    v
}

impl Project {
    /// Put the STEP model of a KiCad footprint into `cache/models/kicad/`.
    fn resolve_kicad_model(
        &self,
        fp_library: &str,
        fp_name: &str,
        m: &lamina_kicad::KicadModel,
        online: bool,
        project_dir: Option<&Path>,
        notes: &mut Vec<String>,
    ) -> Option<(ModelRef, Option<f64>)> {
        let (lib, name) = library::model_lib_name(&m.path)
            .unwrap_or_else(|| (fp_library.to_string(), fp_name.to_string()));
        let rel = format!("kicad/{lib}/{name}.step");
        let dst = self.root().join(MODEL_DIR).join(&rel);
        // `${KIPRJMOD}` is the KiCad project folder. It is also the base of a
        // relative path that has no variable. These files are never online.
        // Lamina copies them again on each import, because the user can edit them.
        let in_project = m.path.starts_with("${KIPRJMOD}")
            || (!m.path.starts_with("${") && !Path::new(&m.path).is_absolute());
        let online = online && !in_project;
        if !dst.is_file() || in_project {
            let mut roots = model_dirs(self.root());
            if in_project {
                roots = project_dir.map(Path::to_path_buf).into_iter().collect();
            }
            let local = library::resolve_model(&roots, &m.path);
            match local {
                Some(src)
                    if src.extension().is_some_and(|e| {
                        matches!(e.to_str(), Some("step" | "STEP" | "stp" | "STP"))
                    }) =>
                {
                    if let Some(parent) = dst.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(e) = std::fs::copy(&src, &dst) {
                        notes.push(format!("The model file was not copied: {e}"));
                        return None;
                    }
                }
                Some(src) => {
                    notes.push(format!(
                        "Only a VRML model was found locally ({}). Lamina does not support VRML.",
                        src.display()
                    ));
                    if !online {
                        return None;
                    }
                }
                None => {}
            }
            if !dst.is_file() {
                if !online {
                    notes.push(format!(
                        "3D model {lib}/{name} was not found in the local libraries. \
                         Set online: true to download it."
                    ));
                    return None;
                }
                match self.kicad_source().model(&lib, &name) {
                    Ok(bytes) => {
                        if let Some(parent) = dst.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if let Err(e) = write_atomic(&dst, &bytes) {
                            notes.push(format!("The model file was not saved: {e}"));
                            return None;
                        }
                    }
                    Err(e) => {
                        notes.push(format!("3D model not downloaded: {e}"));
                        return None;
                    }
                }
            }
        }
        match self.load_mesh(&dst) {
            Ok(mesh) => {
                let height = mesh.bounds().map(|(_, hi)| json::round6(hi[2] as f64));
                notes.push(format!("3D model {rel}"));
                Some((
                    ModelRef {
                        file: rel,
                        offset: m.offset,
                        rotation: m.rotation,
                        scale: m.scale,
                    },
                    height,
                ))
            }
            Err(e) => {
                notes.push(format!("The STEP model did not load: {e}"));
                None
            }
        }
    }

    /// The first model of a footprint that resolves. Lamina tries all models
    /// locally first, then online. It keeps the notes of the failed tries only
    /// if no model resolves.
    fn resolve_kicad_models(
        &self,
        fp_library: &str,
        fp_name: &str,
        models: &[lamina_kicad::KicadModel],
        online: bool,
        project_dir: Option<&Path>,
        notes: &mut Vec<String>,
    ) -> Option<(ModelRef, Option<f64>)> {
        let mut tries = Vec::new();
        let passes: &[bool] = if online { &[false, true] } else { &[false] };
        for &net in passes {
            for m in models {
                let mut n = Vec::new();
                if let Some(r) =
                    self.resolve_kicad_model(fp_library, fp_name, m, net, project_dir, &mut n)
                {
                    notes.extend(n);
                    return Some(r);
                }
                if net || !online {
                    tries.extend(n);
                }
            }
        }
        tries.dedup();
        notes.extend(tries);
        None
    }

    pub fn import_kicad_board(&mut self, p: ImportKicadBoard) -> Result<OpResult> {
        let path = Path::new(&p.path);
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
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
        let imported = lamina_kicad::import_board(&text, &name).map_err(Error::Invalid)?;
        let mut notes = imported.notes;
        let mut d = imported.data;
        d.notes = self.data.notes.clone();
        // KiCad 6+ keeps rules and net classes in the project file beside the board.
        let pro = path.with_extension("kicad_pro");
        if let Ok(text) = std::fs::read_to_string(&pro) {
            match lamina_kicad::apply_project(&mut d, &text) {
                Ok(n) => notes.extend(n),
                Err(e) => notes.push(format!("{}: {e}", pro.display())),
            }
        }
        if p.models {
            let mut found = 0;
            for (fp_id, m) in &imported.models {
                let Some(fp) = d.footprints.get(fp_id) else {
                    continue;
                };
                let (lib, fname) = match &fp.provenance {
                    Provenance::Kicad {
                        library, footprint, ..
                    } => (library.clone(), footprint.clone()),
                    _ => (String::new(), fp_id.clone()),
                };
                let mut mnotes = Vec::new();
                if let Some((mr, h)) =
                    self.resolve_kicad_models(&lib, &fname, m, p.online, path.parent(), &mut mnotes)
                {
                    let fp = d.footprints.get_mut(fp_id).unwrap();
                    fp.model = Some(mr);
                    fp.height = h;
                    found += 1;
                } else {
                    notes.extend(mnotes.into_iter().filter(|n| !n.starts_with("3D model ")));
                }
            }
            notes.push(format!(
                "3D models: {found} of {} resolved{}",
                imported.models.len(),
                if found < imported.models.len() && !p.online {
                    ". Set online: true to download the others."
                } else {
                    ""
                }
            ));
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

    pub fn export_kicad(&self, p: ExportKicad) -> Result<KicadExported> {
        let path = match p.path {
            Some(s) => PathBuf::from(s),
            None => self
                .root()
                .join("output")
                .join(format!("{}.kicad_pcb", self.data.board.name)),
        };
        let text = lamina_kicad::export_board_with(&self.data, Some(&self.root().join(MODEL_DIR)));
        write_atomic(&path, text.as_bytes())?;
        Ok(KicadExported {
            path: path.display().to_string(),
            parts: self.data.parts.parts.len(),
            nets: self.data.nets.nets.len(),
            tracks: self.data.routing.tracks.len(),
            zones: self.data.routing.zones.len(),
        })
    }
}
