//! Import and export of KiCad footprints and boards.

use std::path::{Path, PathBuf};

use lamina_kicad::library;
use lamina_model::error::Result;
use lamina_model::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::project::{OpResult, Project, Touched};

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SearchKicadFootprints {
    /// Words that must all be in `Library:Name`, for example `sot-23` or `qfn 32 0.5`.
    pub query: String,
    /// The maximum number of results (default 20).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct KicadSearch {
    pub hits: Vec<library::Hit>,
    /// The number of library folders that Lamina searched.
    pub libraries: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImportKicadFootprint {
    /// `Library:Name` (for example `Resistor_SMD:R_0603_1608Metric`), a footprint
    /// name to find in the local libraries, or the path of a `.kicad_mod` file.
    pub footprint: String,
    /// Also add a part with this refdes.
    #[serde(default)]
    pub refdes: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    /// The footprint id to use instead of the KiCad name.
    #[serde(default)]
    pub footprint_id: Option<String>,
    /// Also get the STEP model (default true).
    #[serde(default = "yes")]
    pub model: bool,
    /// If the footprint is not in the local libraries, download it from the
    /// official KiCad libraries (default true). This needs `Library:Name`.
    #[serde(default = "yes")]
    pub online: bool,
    /// Replace a footprint or a part that has the same id.
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImportKicadBoard {
    /// The path of the `.kicad_pcb` file.
    pub path: String,
    /// Find the 3D models in the local KiCad libraries (default true).
    #[serde(default = "yes")]
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

/// KiCad footprint roots: `kicad_footprints` of the workspace, the KiCad
/// installation, then the `kicad/footprints` folders of the part library.
pub(crate) fn footprint_dirs(root: &Path, lib: &Library) -> Vec<PathBuf> {
    let mut v = Workspace::find(root)
        .map(|w| w.kicad_footprint_dirs())
        .unwrap_or_default();
    v.extend(library::env_footprint_dirs());
    v.extend(lib.kicad_footprint_dirs());
    v
}

/// KiCad model roots, in the same order as [`footprint_dirs`].
pub(crate) fn model_dirs(root: &Path, lib: &Library) -> Vec<PathBuf> {
    let mut v = Workspace::find(root)
        .map(|w| w.kicad_model_dirs())
        .unwrap_or_default();
    v.extend(library::env_model_dirs());
    v.extend(lib.kicad_model_dirs());
    v
}

impl Project {
    pub fn search_kicad_footprints(&self, p: SearchKicadFootprints) -> Result<KicadSearch> {
        let dirs = footprint_dirs(self.root(), self.library());
        let libraries = library::pretty_libs(&dirs).len();
        let hits = library::search(&dirs, &p.query, p.limit.unwrap_or(20));
        let hint = if libraries == 0 {
            Some(
                "There are no local KiCad libraries. Set kicad_footprints in lamina.json, \
                 or set KICAD9_FOOTPRINT_DIR. Also, import_kicad_footprint with `Library:Name` \
                 downloads from the official online libraries and keeps a copy in the part library."
                    .into(),
            )
        } else {
            None
        };
        Ok(KicadSearch {
            hits,
            libraries,
            hint,
        })
    }

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
            let mut roots = model_dirs(self.root(), self.library());
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
                        let _ = self.library().put(&kicad_model_rel(&lib, &name), &bytes);
                    }
                    Err(e) => {
                        notes.push(format!("3D model not downloaded: {e}"));
                        return None;
                    }
                }
            }
        }
        match lamina_step::load(&dst) {
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

    pub fn import_kicad_footprint(&mut self, p: ImportKicadFootprint) -> Result<OpResult> {
        let spec = p.footprint.trim();
        let as_path = Path::new(spec);
        let (text, library, origin) = if as_path.is_file() {
            let lib = as_path
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|s| s.to_str())
                .map(|s| s.trim_end_matches(".pretty").to_string())
                .unwrap_or_default();
            let text = std::fs::read_to_string(as_path).map_err(|e| Error::io(as_path, e))?;
            (text, lib, spec.replace('\\', "/"))
        } else if let Some(hit) = library::find(&footprint_dirs(self.root(), self.library()), spec)
        {
            let text = std::fs::read_to_string(&hit.path)
                .map_err(|e| Error::io(Path::new(&hit.path), e))?;
            (text, hit.library, hit.path)
        } else if p.online {
            let (lib, name) = spec.split_once(':').ok_or_else(|| {
                Error::invalid(format!(
                    "`{spec}` was not found in the local KiCad libraries. \
                     Give `Library:Name` to download it from the online libraries."
                ))
            })?;
            let text = self.kicad_source().footprint(lib, name).map_err(|e| {
                Error::invalid(format!(
                    "{spec}: {e}. Run `lamina fetch --kicad {spec}` on a computer with a \
                     network to put it in the library."
                ))
            })?;
            let _ = self
                .library()
                .put(&kicad_footprint_rel(lib, name), text.as_bytes());
            (
                text,
                lib.to_string(),
                format!("kicad-footprints/{lib}.pretty/{name}.kicad_mod"),
            )
        } else {
            return Err(Error::invalid(format!(
                "`{spec}` was not found in the local KiCad libraries. \
                 Give `Library:Name` and set online: true to download it."
            )));
        };
        let inst =
            lamina_kicad::parse_footprint(&text, &library, &origin).map_err(Error::Invalid)?;
        let mut fp = inst.footprint;
        if let Some(id) = &p.footprint_id {
            fp.id = id.clone();
        }
        if !valid_id(&fp.id) {
            return Err(Error::invalid(format!(
                "The footprint id `{}` cannot be used as a file name. Set footprint_id to a different id.",
                fp.id
            )));
        }
        let mut notes = inst.notes.clone();
        let mut d = self.data.clone();
        let mut touched = Touched::default();
        let fp_id = fp.id.clone();
        if d.footprints.contains_key(&fp_id) && !p.replace {
            notes.push(format!(
                "The footprint `{fp_id}` is already in the project. It is kept as is."
            ));
        } else {
            if p.model {
                if !inst.models.is_empty() {
                    // The project of a footprint file is the folder above its `.pretty` folder.
                    let dir = as_path.parent().filter(|_| as_path.is_file()).map(|d| {
                        match d.extension() {
                            Some(e) if e == "pretty" => d.parent().unwrap_or(d),
                            _ => d,
                        }
                    });
                    if let Some((mr, h)) = self.resolve_kicad_models(
                        &inst.library,
                        &inst.name,
                        &inst.models,
                        p.online,
                        dir,
                        &mut notes,
                    ) {
                        fp.model = Some(mr);
                        fp.height = h;
                    }
                } else {
                    notes.push("The footprint does not name a 3D model.".into());
                }
            }
            touched = touched.footprint(fp_id.clone());
            d.footprints.insert(fp_id.clone(), fp);
        }
        let pads = d.footprints[&fp_id].pads.len();
        let mut summary = format!(
            "{}:{} -> footprint `{fp_id}` with {pads} pads",
            inst.library, inst.name
        );
        let parts_generated = lamina_build::owned(self.root()).contains(&FileKind::Parts);
        if let Some(r) = p.refdes.as_deref() {
            if parts_generated {
                notes.push(format!(
                    "parts.json is generated from {}. Add the part there: \
                     {r}: {{ footprint: '{fp_id}'{} }}",
                    lamina_build::SOURCE_FILE,
                    p.value
                        .as_deref()
                        .map(|v| format!(", value: '{v}'"))
                        .unwrap_or_default()
                ));
                summary = format!("{r} not added (see notes): {summary}");
            } else {
                if !valid_refdes(r) {
                    return Err(Error::invalid(format!("The refdes `{r}` is not valid.")));
                }
                if d.parts.parts.contains_key(r) && !p.replace {
                    return Err(Error::invalid(format!(
                        "{r} already exists. Set replace: true to change it."
                    )));
                }
                let mut part = Part::new(fp_id.clone());
                part.value = p.value.clone();
                part.description = inst.properties.get("Description").cloned();
                d.parts.parts.insert(r.to_string(), part);
                touched = touched.and(FileKind::Parts);
                summary = format!("{r}: {summary}");
            }
        }
        let mut res = self.commit(d, touched, p.strict, summary)?;
        res.notes = notes;
        Ok(res)
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
