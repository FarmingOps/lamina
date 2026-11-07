use std::path::Path;

use lamina_model::error::Result;
use lamina_model::*;
use lamina_parts::PartHit;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::project::{OpResult, Project, Touched};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SearchPart {
    /// Keywords, an MPN or an LCSC number, for example `AMS1117-3.3`, `0603 10k`
    /// or `C6186`.
    pub query: String,
    /// The maximum number of results (default 10).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Find only JLCPCB basic parts.
    #[serde(default)]
    pub basic_only: bool,
    /// Remove the parts that have no stock from the results (default true).
    #[serde(default = "yes")]
    pub in_stock: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PartSearch {
    pub hits: Vec<PartHit>,
    /// Set when the results come from the local library, not from LCSC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImportLcsc {
    /// The LCSC number, for example `C6186`.
    pub lcsc: String,
    /// Also add a part with this refdes.
    #[serde(default)]
    pub refdes: Option<String>,
    /// The part value (default: the MPN).
    #[serde(default)]
    pub value: Option<String>,
    /// The footprint id to use instead of the EasyEDA package name.
    #[serde(default)]
    pub footprint_id: Option<String>,
    /// Download the data again. Do not use the project cache.
    #[serde(default)]
    pub refresh: bool,
    /// Also download the STEP model (default true).
    #[serde(default = "yes")]
    pub step: bool,
    /// Replace a part that has the same refdes.
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImportMany {
    pub parts: Vec<ImportLcsc>,
}

/// Today's date as YYYY-MM-DD (UTC).
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    // Civil-from-days (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn read_cached(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

impl Project {
    pub fn search_part(&self, p: SearchPart) -> Result<PartSearch> {
        if p.query.trim().is_empty() {
            return Err(Error::invalid(
                "The query is empty. Give one or more keywords.",
            ));
        }
        let limit = p.limit.unwrap_or(10).clamp(1, 50);
        let (mut hits, note) = match self.source().search(p.query.trim(), limit * 2) {
            Ok(h) => (h, None),
            Err(e) => {
                // No network: search the parts that the library has.
                let local = crate::library::search_library(self.library(), p.query.trim());
                if local.is_empty() {
                    return Err(Error::Invalid(format!(
                        "{e}. The local library has no part that matches."
                    )));
                }
                let note = format!(
                    "LCSC did not answer ({e}). These results come from the local library. \
                     The stock and the price are not known."
                );
                (local, Some(note))
            }
        };
        if p.basic_only {
            hits.retain(|h| h.basic);
        }
        if p.in_stock && note.is_none() {
            hits.retain(|h| h.stock > 0);
        }
        hits.truncate(limit);
        Ok(PartSearch { hits, note })
    }

    pub fn import_lcsc(&mut self, p: ImportLcsc) -> Result<OpResult> {
        let lcsc = p.lcsc.trim().to_ascii_uppercase();
        let root = self.root().to_path_buf();
        let cache = root.join(PARTS_DIR).join(format!("{lcsc}.json"));
        let lib_rel = part_rel(&lcsc);
        let cached = (!p.refresh)
            .then(|| read_cached(&cache))
            .flatten()
            .map(|t| (t, None))
            .or_else(|| {
                // Not in the project: use the library before the network.
                (!p.refresh)
                    .then(|| self.library().find(&lib_rel))
                    .flatten()
                    .and_then(|path| read_cached(&path).map(|t| (t, Some(path))))
            });
        let mut lib_note = None;
        let raw: serde_json::Value = match cached {
            Some((t, from)) => {
                let v = serde_json::from_str(&t)
                    .map_err(|e| Error::invalid(format!("{}: {e}", cache.display())))?;
                if let Some(path) = from {
                    write_atomic(&cache, t.as_bytes())?;
                    lib_note = Some(format!("{lcsc} came from the library ({})", path.display()));
                }
                v
            }
            None => {
                let v = self.source().component(&lcsc).map_err(|e| {
                    Error::Invalid(format!(
                        "{e}. {lcsc} is not in the project cache or in the library. \
                         Run `lamina fetch {lcsc}` on a computer with a network to put it in the library."
                    ))
                })?;
                let text = serde_json::to_string(&v).map_err(|e| Error::invalid(e.to_string()))?;
                write_atomic(&cache, text.as_bytes())?;
                if let Err(e) = self.library().put(&lib_rel, text.as_bytes()) {
                    lib_note = Some(format!("The library did not get a copy: {e}"));
                }
                v
            }
        };
        let mut im = lamina_parts::convert(&raw, &lcsc, &today()).map_err(Error::Invalid)?;
        if let Some(id) = &p.footprint_id {
            im.footprint.id = id.clone();
        }
        let mut notes = im.notes.clone();
        notes.extend(lib_note);
        let mut d = self.data.clone();
        let mut touched = Touched::default();
        let fp_id = im.footprint.id.clone();

        let reuse = !p.refresh && d.footprints.contains_key(&fp_id);
        if reuse {
            notes.push(format!(
                "The footprint `{fp_id}` is already in the project. It is kept as is."
            ));
        } else {
            if let Some(info) = &im.model {
                if let Some((m, h)) = self.fetch_easyeda_model(info, p.refresh, p.step, &mut notes)
                {
                    im.footprint.model = Some(m);
                    im.footprint.height = h;
                }
            } else {
                notes.push("EasyEDA has no 3D model for this footprint.".into());
            }
            touched = touched.footprint(fp_id.clone());
            d.footprints.insert(fp_id.clone(), im.footprint.clone());
        }

        let fp = &d.footprints[&fp_id];
        let pads: Vec<&str> = {
            let mut v: Vec<&str> = fp
                .pads
                .iter()
                .map(|x| x.number.as_str())
                .filter(|n| !n.is_empty())
                .collect();
            v.sort_by(|a, b| natural(a, b));
            v.dedup();
            v
        };
        let meta = &im.meta;
        let class = match meta.basic {
            Some(true) => "basic",
            Some(false) => "extended",
            None => "unknown class",
        };
        let mut summary = format!(
            "{lcsc} {} ({}), {class}: footprint `{fp_id}` with {} pads [{}]",
            meta.mpn.as_deref().unwrap_or("?"),
            meta.package.as_deref().unwrap_or("?"),
            fp.pads.len(),
            pads.join(" ")
        );

        let parts_generated = lamina_build::owned(self.root()).contains(&FileKind::Parts);
        if let (Some(r), true) = (p.refdes.as_deref(), parts_generated) {
            let q = |s: &str| s.replace('\\', "\\\\").replace('\'', "\\'");
            let mut fields = vec![format!("footprint: '{}'", q(&fp_id))];
            if let Some(v) = p.value.as_deref().or(meta.mpn.as_deref()) {
                fields.push(format!("value: '{}'", q(v)));
            }
            if let Some(m) = &meta.mpn {
                fields.push(format!("mpn: '{}'", q(m)));
            }
            if let Some(m) = &meta.manufacturer {
                fields.push(format!("manufacturer: '{}'", q(m)));
            }
            fields.push(format!("lcsc: '{lcsc}'"));
            notes.push(format!(
                "parts.json is generated from {}. Add the part there: {r}: {{ {} }}",
                lamina_build::SOURCE_FILE,
                fields.join(", ")
            ));
            summary = format!("{r} not added (see notes): {summary}");
        } else if let Some(r) = p.refdes {
            if !valid_refdes(&r) {
                return Err(Error::invalid(format!("The refdes `{r}` is not valid.")));
            }
            if d.parts.parts.contains_key(&r) && !p.replace {
                return Err(Error::invalid(format!(
                    "{r} already exists. Set replace: true to change it."
                )));
            }
            let mut part = Part::new(fp_id.clone());
            part.value = p.value.or_else(|| meta.mpn.clone());
            part.mpn = meta.mpn.clone();
            part.manufacturer = meta.manufacturer.clone();
            part.lcsc = Some(lcsc.clone());
            part.description = meta.description.clone();
            d.parts.parts.insert(r.clone(), part);
            touched = touched.and(FileKind::Parts);
            summary = format!("{r}: {summary}");
        }
        let mut res = self.commit(d, touched, p.strict, summary)?;
        res.notes = notes;
        Ok(res)
    }

    /// Import more than one LCSC part. Stop at the first error.
    pub fn import_many(&mut self, p: ImportMany) -> Result<Vec<OpResult>> {
        let mut out = Vec::new();
        for item in p.parts {
            let lcsc = item.lcsc.clone();
            out.push(
                self.import_lcsc(item)
                    .map_err(|e| Error::invalid(format!("{lcsc}: {e}")))?,
            );
        }
        Ok(out)
    }
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
    #[serde(default = "yes")]
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
                match lamina_step::load(&step_path) {
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
