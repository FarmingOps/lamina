//! Parts from providers: search, import, and the library that makes them
//! work without a network. See `lamina_model::provider`.
//!
//! Lookup order for a part: the project cache, then the library, then the
//! provider. Each download is written to the project cache and to the
//! library store.

use std::path::Path;
use std::sync::Arc;

use lamina_model::error::Result;
use lamina_model::provider::{Bundle, Hit, Provider, ProviderInfo, Record, record_rel, split_name};
use lamina_model::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::lcsc::natural;
use crate::project::{OpResult, Project, Touched};

/// The cache folder of a project. `cache/parts` and `cache/models` are in it.
const CACHE_DIR: &str = "cache";

/// The providers that a project can use, in the search order.
#[derive(Clone, Default)]
pub struct Providers {
    list: Vec<Arc<dyn Provider>>,
}

impl Providers {
    pub fn new(list: Vec<Arc<dyn Provider>>) -> Providers {
        Providers { list }
    }

    /// The standard set: the parts vendor and the KiCad libraries. The local
    /// KiCad roots come from `lamina.json` of the workspace that holds `board`.
    pub fn standard(board: Option<&Path>) -> Providers {
        Providers::new(crate::wiring::standard(board).providers)
    }

    pub fn get(&self, id: &str) -> Option<&dyn Provider> {
        self.list
            .iter()
            .find(|p| p.info().id == id)
            .map(|p| p.as_ref())
    }

    pub fn infos(&self) -> Vec<ProviderInfo> {
        self.list.iter().map(|p| p.info()).collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn Provider> {
        self.list.iter().map(|p| p.as_ref())
    }

    fn ids(&self) -> String {
        self.list
            .iter()
            .map(|p| p.info().id)
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn need(&self, id: &str) -> Result<&dyn Provider> {
        self.get(id).ok_or_else(|| {
            Error::invalid(format!(
                "There is no provider `{id}`. The providers are: {}.",
                self.ids()
            ))
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SearchParts {
    /// Keywords, an MPN, a part number or a footprint name.
    pub query: String,
    /// Search only this provider. The default is all providers.
    #[serde(default)]
    pub provider: Option<String>,
    /// The maximum number of results for each provider (default 10).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Keep only the parts with no extra assembly fee.
    #[serde(default)]
    pub basic_only: bool,
    /// Remove the vendor parts that have no stock (default true).
    #[serde(default = "super::yes")]
    pub in_stock: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PartSearch {
    pub hits: Vec<Hit>,
    /// What did not work, for example a provider without a network. The
    /// hits of that provider then come from the library.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImportPart {
    /// The part as `provider:id`, for example `lcsc:C6186` or
    /// `kicad:LED_SMD:LED_0603_1608Metric`. The `providers` tool lists the
    /// providers and their id formats.
    pub part: String,
    /// Also add a part with this refdes.
    #[serde(default)]
    pub refdes: Option<String>,
    /// The part value (default: the MPN or the footprint name).
    #[serde(default)]
    pub value: Option<String>,
    /// The footprint id to use instead of the provider's name.
    #[serde(default)]
    pub footprint_id: Option<String>,
    /// Get the data again. Do not use the project cache or the library.
    #[serde(default)]
    pub refresh: bool,
    /// Also get the 3D model (default true).
    #[serde(default = "super::yes")]
    pub model: bool,
    /// Replace a footprint or a part that has the same id.
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImportParts {
    pub parts: Vec<ImportPart>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct FetchLibrary {
    /// Parts as `provider:id`, for example `["lcsc:C6186", "kicad:LED_SMD:LED_0603_1608Metric"]`.
    pub parts: Vec<String>,
    /// Download again, also when the library has the part.
    #[serde(default)]
    pub refresh: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Fetched {
    /// The folder that got the files.
    pub library: String,
    /// The parts that are now in the library.
    pub done: Vec<String>,
    /// The parts that were in the library already.
    pub kept: Vec<String>,
    /// The parts that failed, with the reason.
    pub failed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LibraryInfo {
    /// The folder that gets new downloads.
    pub store: String,
    /// All library folders, in the search order.
    pub roots: Vec<String>,
    /// The parts in the library, as `provider:id`.
    pub parts: Vec<String>,
}

/// Search the records of the library by words in the id, the MPN, the
/// manufacturer, the package or the description. All words must match.
pub fn search_library(lib: &Library, provider: Option<&str>, query: &str) -> Vec<Hit> {
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.to_ascii_lowercase())
        .collect();
    lib.records(provider)
        .into_iter()
        .filter(|r| {
            let i = &r.info;
            let hay = [
                Some(r.id.as_str()),
                i.mpn.as_deref(),
                i.manufacturer.as_deref(),
                i.package.as_deref(),
                i.description.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .map(|r| Hit {
            provider: r.provider.clone(),
            id: r.id.clone(),
            title: r.info.mpn.clone().unwrap_or_else(|| r.footprint.id.clone()),
            package: r.info.package.clone(),
            manufacturer: r.info.manufacturer.clone(),
            description: r.info.description.clone(),
            stock: None,
            price: None,
            basic: r.info.basic,
            datasheet: r.info.datasheet.clone(),
            local: true,
        })
        .collect()
}

pub fn library_info(lib: &Library) -> LibraryInfo {
    LibraryInfo {
        store: lib.store.display().to_string(),
        roots: lib.roots.iter().map(|r| r.display().to_string()).collect(),
        parts: lib
            .records(None)
            .iter()
            .map(|r| format!("{}:{}", r.provider, r.id))
            .collect(),
    }
}

/// Write a bundle as a record plus a model file under `base`: the `cache`
/// folder of a project, or a library root. Both have `parts/` and `models/`.
fn store_bundle(
    base: &Path,
    rec: &Record,
    model: Option<&lamina_model::provider::ModelFile>,
) -> Result<()> {
    if let (Some(m), Some(r)) = (model, rec.model()) {
        write_atomic(&base.join("models").join(&r.file), &m.bytes)?;
    }
    let text = serde_json::to_string(rec).map_err(|e| Error::invalid(e.to_string()))?;
    write_atomic(
        &base.join(record_rel(&rec.provider, &rec.id)),
        text.as_bytes(),
    )
}

/// Get a part into the library store only (no project).
pub fn fetch(lib: &Library, providers: &Providers, p: &FetchLibrary) -> Result<Fetched> {
    let mut r = Fetched {
        library: lib.store.display().to_string(),
        done: Vec::new(),
        kept: Vec::new(),
        failed: Vec::new(),
    };
    for name in &p.parts {
        let Some((pid, id)) = split_name(name) else {
            r.failed.push(format!(
                "{name}: a part is `provider:id` ({})",
                providers.ids()
            ));
            continue;
        };
        let full = format!("{pid}:{id}");
        let rel = record_rel(pid, id);
        if !p.refresh && lib.find(&rel).is_some() {
            r.kept.push(full);
            continue;
        }
        let res = providers
            .need(pid)
            .and_then(|prov| prov.fetch(id, true).map_err(Error::Invalid))
            .and_then(|b| {
                let rec = record_of(pid, id, b.clone());
                store_bundle(&lib.store, &rec, b.model.as_ref())
            });
        match res {
            Ok(()) => r.done.push(full),
            Err(e) => r.failed.push(format!("{full}: {e}")),
        }
    }
    Ok(r)
}

/// Make the record of a bundle. The model goes to `models/<provider>/<name>`.
fn record_of(provider: &str, id: &str, b: Bundle) -> Record {
    let mut fp = b.footprint;
    if let Some(m) = &b.model {
        fp.model = Some(ModelRef {
            file: format!("{provider}/{}", m.name),
            offset: m.offset,
            rotation: m.rotation,
            scale: m.scale,
        });
        if m.height.is_some() {
            fp.height = m.height;
        }
    }
    Record {
        provider: provider.into(),
        id: id.into(),
        footprint: fp,
        info: b.info,
        notes: b.notes,
    }
}

impl Project {
    pub fn providers(&self) -> &Providers {
        &self.providers
    }

    pub fn provider_infos(&self) -> Vec<ProviderInfo> {
        self.providers().infos()
    }

    pub fn search_parts(&self, p: SearchParts) -> Result<PartSearch> {
        if p.query.trim().is_empty() {
            return Err(Error::invalid(
                "The query is empty. Give one or more keywords.",
            ));
        }
        let limit = p.limit.unwrap_or(10).clamp(1, 50);
        let query = p.query.trim();
        let chosen: Vec<&dyn Provider> = match &p.provider {
            Some(id) => vec![self.providers().need(id)?],
            None => self.providers().iter().collect(),
        };
        let mut hits = Vec::new();
        let mut notes = Vec::new();
        for prov in chosen {
            let id = prov.info().id;
            match prov.search(query, limit * 2) {
                Ok(mut h) => {
                    if p.in_stock {
                        h.retain(|x| x.stock.is_none_or(|s| s > 0));
                    }
                    h.truncate(limit);
                    hits.extend(h);
                }
                Err(e) => {
                    let mut local = search_library(self.library(), Some(&id), query);
                    local.truncate(limit);
                    if local.is_empty() {
                        notes.push(format!("{id}: {e}"));
                    } else {
                        notes.push(format!(
                            "{id}: {e}. These hits come from the local library; stock and price are not known."
                        ));
                        hits.extend(local);
                    }
                }
            }
        }
        if p.basic_only {
            hits.retain(|h| h.basic == Some(true));
        }
        if hits.is_empty() && !notes.is_empty() {
            return Err(Error::Invalid(notes.join(" ")));
        }
        Ok(PartSearch { hits, notes })
    }

    /// Copy the model file of a record from the library when the cache does
    /// not have it. If no copy exists, the record loses its model.
    fn ensure_model_in_cache(&self, rec: &mut Record, notes: &mut Vec<String>) -> Result<()> {
        let Some(m) = rec.model().cloned() else {
            return Ok(());
        };
        let dst = self.root().join(MODEL_DIR).join(&m.file);
        if dst.is_file() {
            return Ok(());
        }
        match self.library().find(&format!("models/{}", m.file)) {
            Some(src) => {
                if let Some(d) = dst.parent() {
                    std::fs::create_dir_all(d).map_err(|e| Error::io(d, e))?;
                }
                std::fs::copy(&src, &dst).map_err(|e| Error::io(&src, e))?;
            }
            None => {
                notes.push(format!(
                    "The model file {} is not in the cache or in the library. The part has no model.",
                    m.file
                ));
                rec.footprint.model = None;
            }
        }
        Ok(())
    }

    /// The record of a part: from the project cache, the library, or the
    /// provider. A download is written to both.
    fn fetch_record(
        &self,
        pid: &str,
        id: &str,
        refresh: bool,
        model: bool,
        notes: &mut Vec<String>,
    ) -> Result<Record> {
        let rel = record_rel(pid, id);
        let cache = self.root().join(CACHE_DIR).join(&rel);
        let lib = self.library();
        if !refresh && cache.is_file() {
            let mut rec = read_record(&cache)?;
            self.ensure_model_in_cache(&mut rec, notes)?;
            return Ok(rec);
        }
        if let Some(p) = (!refresh).then(|| lib.find(&rel)).flatten() {
            let mut rec = read_record(&p)?;
            self.ensure_model_in_cache(&mut rec, notes)?;
            store_bundle(&self.root().join(CACHE_DIR), &rec, None)?;
            notes.push(format!(
                "{pid}:{id} came from the library ({})",
                p.display()
            ));
            return Ok(rec);
        }
        let prov = self.providers().need(pid)?;
        let b = prov.fetch(id, model).map_err(|e| {
            Error::Invalid(format!(
                "{e}. `{pid}:{id}` is not in the project cache or in the library. \
                 Run `lamina fetch {pid}:{id}` on a computer with a network to put it in the library."
            ))
        })?;
        let mut rec = record_of(pid, id, b.clone());
        store_bundle(&self.root().join(CACHE_DIR), &rec, b.model.as_ref())?;
        // Check that the model loads. Take the height from the mesh when the
        // provider did not know it.
        if let Some(m) = rec.model().cloned() {
            let path = self.root().join(MODEL_DIR).join(&m.file);
            match self.load_mesh(&path) {
                Ok(mesh) => {
                    if rec.footprint.height.is_none() {
                        rec.footprint.height =
                            mesh.bounds().map(|(_, hi)| json::round6(hi[2] as f64));
                    }
                    rec.notes.push(format!("3D model {}", m.file));
                }
                Err(e) => {
                    rec.notes.push(format!(
                        "The 3D model did not load, so the part has none: {e}"
                    ));
                    rec.footprint.model = None;
                }
            }
            store_bundle(&self.root().join(CACHE_DIR), &rec, None)?;
        }
        if let Err(e) = store_bundle(&lib.store, &rec, b.model.as_ref()) {
            notes.push(format!("The library did not get a copy: {e}"));
        }
        Ok(rec)
    }

    pub fn import_part(&mut self, p: ImportPart) -> Result<OpResult> {
        let (pid, id) = split_name(&p.part).ok_or_else(|| {
            Error::invalid(format!(
                "`{}` is not a part name. Use `provider:id`, for example `lcsc:C6186`. The providers are: {}.",
                p.part,
                self.providers().ids()
            ))
        })?;
        let (pid, id) = (pid.to_string(), id.to_string());
        let mut notes = Vec::new();
        let rec = self.fetch_record(&pid, &id, p.refresh, p.model, &mut notes)?;
        let mut fp = rec.footprint.clone();
        if let Some(fid) = &p.footprint_id {
            fp.id = fid.clone();
        }
        if !valid_id(&fp.id) {
            return Err(Error::invalid(format!(
                "The footprint id `{}` cannot be used as a file name. Set footprint_id to a different id.",
                fp.id
            )));
        }
        notes.extend(rec.notes.iter().cloned());
        let mut d = self.data.clone();
        let mut touched = Touched::default();
        let fp_id = fp.id.clone();
        if d.footprints.contains_key(&fp_id) && !p.replace && !p.refresh {
            notes.push(format!(
                "The footprint `{fp_id}` is already in the project. It is kept as is."
            ));
        } else {
            if !p.model {
                fp.model = None;
            }
            touched = touched.footprint(fp_id.clone());
            d.footprints.insert(fp_id.clone(), fp);
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
        let info = &rec.info;
        let class = match info.basic {
            Some(true) => ", basic",
            Some(false) => ", extended",
            None => "",
        };
        let mut summary = format!(
            "{pid}:{id} {} ({}){class}: footprint `{fp_id}` with {} pads [{}]",
            info.mpn.as_deref().unwrap_or(&fp_id),
            info.package.as_deref().unwrap_or("?"),
            fp.pads.len(),
            pads.join(" ")
        );
        let default_value = info.mpn.clone().unwrap_or_else(|| fp_id.clone());

        let parts_generated = self.source_owned().contains(&FileKind::Parts);
        if let (Some(r), true) = (p.refdes.as_deref(), parts_generated) {
            let q = |s: &str| s.replace('\\', "\\\\").replace('\'', "\\'");
            let mut fields = vec![format!("footprint: '{}'", q(&fp_id))];
            fields.push(format!(
                "value: '{}'",
                q(p.value.as_deref().unwrap_or(&default_value))
            ));
            if let Some(m) = &info.mpn {
                fields.push(format!("mpn: '{}'", q(m)));
            }
            if let Some(m) = &info.manufacturer {
                fields.push(format!("manufacturer: '{}'", q(m)));
            }
            if let Some(c) = &info.order_code {
                fields.push(format!("lcsc: '{}'", q(c)));
            }
            notes.push(format!(
                "parts.json is generated from {}. Add the part there: {r}: {{ {} }}",
                self.source_name().unwrap_or("the design source"),
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
            part.value = Some(p.value.unwrap_or(default_value));
            part.mpn = info.mpn.clone();
            part.manufacturer = info.manufacturer.clone();
            part.lcsc = info.order_code.clone();
            part.description = info.description.clone();
            d.parts.parts.insert(r.clone(), part);
            touched = touched.and(FileKind::Parts);
            summary = format!("{r}: {summary}");
        }
        let mut res = self.commit(d, touched, p.strict, summary)?;
        res.notes = notes;
        Ok(res)
    }

    /// Import several parts in one call. Stop at the first error.
    pub fn import_parts(&mut self, p: ImportParts) -> Result<Vec<OpResult>> {
        let mut out = Vec::new();
        for item in p.parts {
            let name = item.part.clone();
            out.push(
                self.import_part(item)
                    .map_err(|e| Error::invalid(format!("{name}: {e}")))?,
            );
        }
        Ok(out)
    }

    /// Fill the library with the providers of this project.
    pub fn fetch_library(&self, p: FetchLibrary) -> Result<Fetched> {
        fetch(self.library(), self.providers(), &p)
    }

    pub fn library_info(&self) -> LibraryInfo {
        library_info(self.library())
    }

    /// A model from a provider, written into `cache/models/<provider>/`.
    /// Gives the path relative to `cache/models/`.
    pub(crate) fn provider_model_into_cache(&self, name: &str) -> Result<String> {
        let (pid, id) = split_name(name).ok_or_else(|| {
            Error::invalid(format!(
                "`{name}` is not `provider:id`. The providers are: {}.",
                self.providers().ids()
            ))
        })?;
        let prov = self.providers().need(pid)?;
        let m = prov.model(id).map_err(|e| {
            Error::Invalid(format!(
                "{e}. Run `lamina fetch {pid}:{id}` on a computer with a network to put it in the library."
            ))
        })?;
        let rel = format!("{pid}/{}", m.name);
        write_atomic(&self.root().join(MODEL_DIR).join(&rel), &m.bytes)?;
        let _ = self.library().put(&format!("models/{rel}"), &m.bytes);
        Ok(rel)
    }
}

fn read_record(path: &Path) -> Result<Record> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    serde_json::from_str(&text).map_err(|e| Error::invalid(format!("{}: {e}", path.display())))
}
