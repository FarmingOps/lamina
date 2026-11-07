//! The part library: fill it before a trip, search it without a network.

use std::path::Path;

use std::sync::Arc;

use lamina_model::error::Result;
use lamina_model::*;
use lamina_parts::PartHit;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::project::Project;

/// The sources that a fetch downloads from: a parts vendor and a footprint
/// library.
#[derive(Clone)]
pub struct Sources {
    pub parts: Arc<dyn lamina_parts::PartSource>,
    pub footprints: Arc<dyn lamina_kicad::KicadSource>,
}

impl Sources {
    /// The online sources that Lamina uses by default.
    pub fn online() -> Sources {
        Sources {
            parts: Arc::new(lamina_parts::HttpSource::default()),
            footprints: Arc::new(lamina_kicad::HttpSource),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct FetchLibrary {
    /// LCSC numbers to download, for example `["C6186", "C14663"]`.
    #[serde(default)]
    pub lcsc: Vec<String>,
    /// KiCad footprints as `Library:Name`, for example
    /// `["LED_SMD:LED_0603_1608Metric"]`. Lamina also gets their STEP models.
    #[serde(default)]
    pub kicad: Vec<String>,
    /// Download again, also when the library has the files.
    #[serde(default)]
    pub refresh: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Fetched {
    /// The folder that got the files.
    pub library: String,
    /// The items that are now in the library.
    pub done: Vec<String>,
    /// The items that were in the library already.
    pub kept: Vec<String>,
    /// The items that failed, with the reason.
    pub failed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LibraryInfo {
    /// The folder that gets new downloads.
    pub store: String,
    /// All library folders, in the search order.
    pub roots: Vec<String>,
    /// The LCSC numbers in the library.
    pub parts: Vec<String>,
    /// The number of KiCad footprint files in the library.
    pub kicad_footprints: usize,
}

/// Search the parts of the library by words in the MPN, the manufacturer,
/// the package, the description or the LCSC number. All words must match.
pub fn search_library(lib: &Library, query: &str) -> Vec<PartHit> {
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let mut out = Vec::new();
    for (lcsc, path) in lib.part_files() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(raw) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Ok(im) = lamina_parts::convert(&raw, &lcsc, &crate::lcsc::today()) else {
            continue;
        };
        let meta = im.meta;
        let hay = [
            Some(lcsc.as_str()),
            meta.mpn.as_deref(),
            meta.manufacturer.as_deref(),
            meta.package.as_deref(),
            meta.description.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
        if words.iter().all(|w| hay.contains(w.as_str())) {
            out.push(PartHit {
                lcsc: lcsc.clone(),
                mpn: meta.mpn.clone().unwrap_or_default(),
                manufacturer: meta.manufacturer.clone(),
                package: meta.package.clone(),
                description: meta.description.clone(),
                category: None,
                stock: meta.stock.unwrap_or(0),
                price: meta.price,
                basic: meta.basic.unwrap_or(false),
                datasheet: meta.datasheet.clone(),
            });
        }
    }
    out
}

/// Describe a library.
pub fn library_info(lib: &Library) -> LibraryInfo {
    let kicad_footprints = lib
        .kicad_footprint_dirs()
        .iter()
        .map(|d| count_files(d, "kicad_mod"))
        .sum();
    LibraryInfo {
        store: lib.store.display().to_string(),
        roots: lib.roots.iter().map(|r| r.display().to_string()).collect(),
        parts: lib.part_files().into_iter().map(|(n, _)| n).collect(),
        kicad_footprints,
    }
}

fn count_files(dir: &Path, ext: &str) -> usize {
    let mut n = 0;
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            n += count_files(&p, ext);
        } else if p.extension().is_some_and(|x| x == ext) {
            n += 1;
        }
    }
    n
}

/// Download parts and KiCad footprints into the library store. This needs no
/// project. Each item is reported once: done, kept or failed.
pub fn fetch(lib: &Library, sources: &Sources, p: &FetchLibrary) -> Result<Fetched> {
    let mut r = Fetched {
        library: lib.store.display().to_string(),
        done: Vec::new(),
        kept: Vec::new(),
        failed: Vec::new(),
    };
    for raw in &p.lcsc {
        let lcsc = raw.trim().to_ascii_uppercase();
        if lcsc.is_empty() {
            continue;
        }
        match fetch_part(lib, sources.parts.as_ref(), &lcsc, p.refresh) {
            Ok(true) => r.done.push(lcsc),
            Ok(false) => r.kept.push(lcsc),
            Err(e) => r.failed.push(format!("{lcsc}: {e}")),
        }
    }
    for spec in &p.kicad {
        let spec = spec.trim();
        if spec.is_empty() {
            continue;
        }
        match fetch_footprint(lib, sources.footprints.as_ref(), spec, p.refresh) {
            Ok(true) => r.done.push(spec.to_string()),
            Ok(false) => r.kept.push(spec.to_string()),
            Err(e) => r.failed.push(format!("{spec}: {e}")),
        }
    }
    Ok(r)
}

/// Get one LCSC part and its models. True if something was downloaded.
fn fetch_part(
    lib: &Library,
    src: &dyn lamina_parts::PartSource,
    lcsc: &str,
    refresh: bool,
) -> Result<bool> {
    let rel = part_rel(lcsc);
    let mut got = false;
    let raw: serde_json::Value = match (!refresh).then(|| lib.find(&rel)).flatten() {
        Some(p) => {
            let t = std::fs::read_to_string(&p).map_err(|e| Error::io(&p, e))?;
            serde_json::from_str(&t).map_err(|e| Error::invalid(format!("{}: {e}", p.display())))?
        }
        None => {
            let v = src.component(lcsc).map_err(Error::Invalid)?;
            let text = serde_json::to_string(&v).map_err(|e| Error::invalid(e.to_string()))?;
            lib.put(&rel, text.as_bytes())?;
            got = true;
            v
        }
    };
    let im = lamina_parts::convert(&raw, lcsc, &crate::lcsc::today()).map_err(Error::Invalid)?;
    if let Some(info) = im.model {
        let obj = format!("models/lcsc/{}.obj", info.uuid);
        if refresh || lib.find(&obj).is_none() {
            let t = src.model_obj(&info.uuid).map_err(Error::Invalid)?;
            lib.put(&obj, t.as_bytes())?;
            got = true;
        }
        let step = format!("models/lcsc/{}.step", info.uuid);
        if refresh || lib.find(&step).is_none() {
            // A missing STEP is not an error: the OBJ is enough to render.
            if let Ok(b) = src.model_step(&info.uuid) {
                lib.put(&step, &b)?;
                got = true;
            }
        }
    }
    Ok(got)
}

/// Get one library footprint and the models it names. True if something was downloaded.
fn fetch_footprint(
    lib: &Library,
    src: &dyn lamina_kicad::KicadSource,
    spec: &str,
    refresh: bool,
) -> Result<bool> {
    let (library, name) = spec.split_once(':').ok_or_else(|| {
        Error::invalid(format!(
            "`{spec}` must be `Library:Name`, for example `LED_SMD:LED_0603_1608Metric`."
        ))
    })?;
    let rel = kicad_footprint_rel(library, name);
    let mut got = false;
    let text = match (!refresh).then(|| lib.find(&rel)).flatten() {
        Some(p) => std::fs::read_to_string(&p).map_err(|e| Error::io(&p, e))?,
        None => {
            let t = src.footprint(library, name).map_err(Error::Invalid)?;
            lib.put(&rel, t.as_bytes())?;
            got = true;
            t
        }
    };
    let inst = lamina_kicad::parse_footprint(&text, library, &rel).map_err(Error::Invalid)?;
    for m in &inst.models {
        let Some((mlib, mname)) = lamina_kicad::library::model_lib_name(&m.path) else {
            continue;
        };
        let mrel = kicad_model_rel(&mlib, &mname);
        if !refresh && lib.find(&mrel).is_some() {
            continue;
        }
        match src.model(&mlib, &mname) {
            Ok(b) => {
                lib.put(&mrel, &b)?;
                got = true;
            }
            Err(e) => {
                return Err(Error::invalid(format!(
                    "the footprint is in the library, but its model {mlib}/{mname} failed: {e}"
                )));
            }
        }
    }
    Ok(got)
}

impl Project {
    /// Fill the library with the sources of this project.
    pub fn fetch_library(&self, p: FetchLibrary) -> Result<Fetched> {
        fetch(self.library(), &self.sources(), &p)
    }

    pub fn library_info(&self) -> LibraryInfo {
        library_info(self.library())
    }
}
