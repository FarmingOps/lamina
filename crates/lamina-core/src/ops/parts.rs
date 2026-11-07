use std::collections::BTreeMap;

use lamina_footprint::{Density, Options};
use lamina_model::error::Result;
use lamina_model::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::project::{OpResult, Project, Touched};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FootprintFromPackage {
    /// The package name, for example `R_0603`, `SOIC-8`, `TSSOP-28`, `SOT-223`,
    /// `QFN-32_5x5_P0.5_EP3.3x3.3`, `LQFP-48_7x7_P0.5`, `PinHeader_1x03_P2.54`.
    pub package: String,
    /// IPC density level A (most), B (nominal, default) or C (least).
    #[serde(default)]
    pub density: Option<Density>,
    /// The footprint id. The default is the package name.
    #[serde(default)]
    pub id: Option<String>,
    /// Replace a footprint that has the same id.
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BodySize {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FootprintFromPads {
    pub id: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Pads in footprint coordinates (top view, origin at the part centre).
    pub pads: Vec<Pad>,
    /// The size of the body outline, for the courtyard, silkscreen and fab drawings.
    #[serde(default)]
    pub body: Option<BodySize>,
    /// Body height in mm.
    #[serde(default)]
    pub height: Option<f64>,
    /// More graphics. If they include a courtyard, Lamina does not generate a courtyard.
    #[serde(default)]
    pub graphics: Vec<FpGraphic>,
    #[serde(default)]
    pub keepouts: Vec<FpKeepout>,
    /// The source of the numbers, for example
    /// `{"source": "datasheet", "document": "PCA9685 rev 4", "page": 48}`.
    pub provenance: Provenance,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FootprintId {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AddPart {
    /// The reference designator, for example `R1` or `U2`.
    pub refdes: String,
    /// The footprint id in the project.
    pub footprint: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub mpn: Option<String>,
    #[serde(default)]
    pub manufacturer: Option<String>,
    /// The LCSC part number, for example `C25804`.
    #[serde(default)]
    pub lcsc: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub dnp: bool,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    /// The block instance that the part is in (see `repeat_layout`).
    #[serde(default)]
    pub group: Option<String>,
    /// Replace a part that has the same refdes.
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AddParts {
    pub parts: Vec<AddPart>,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RemovePart {
    pub refdes: String,
    /// Also remove the pads of the part from the nets (default true).
    #[serde(default = "super::yes")]
    pub remove_from_nets: bool,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FootprintSummary {
    pub id: String,
    pub pads: usize,
    pub source: String,
    pub size: Option<Rect>,
    pub has_model: bool,
    pub used_by: Vec<String>,
}

impl Project {
    fn store_footprint(
        &mut self,
        fp: Footprint,
        overwrite: bool,
        summary: String,
    ) -> Result<OpResult> {
        if !valid_id(&fp.id) {
            return Err(Error::invalid(format!(
                "The footprint id `{}` is not valid. Use only letters, digits, `.`, `_` and `-`.",
                fp.id
            )));
        }
        if self.data.footprints.contains_key(&fp.id) && !overwrite {
            return Err(Error::invalid(format!(
                "The footprint `{}` already exists. Set overwrite: true to replace it.",
                fp.id
            )));
        }
        let id = fp.id.clone();
        let mut d = self.data.clone();
        d.footprints.insert(id.clone(), fp);
        self.commit(d, Touched::default().footprint(id), false, summary)
    }

    pub fn footprint_from_package(&mut self, p: FootprintFromPackage) -> Result<OpResult> {
        let opt = Options {
            density: p.density.unwrap_or_default(),
            silk_width: self.data.board.rules.silk_width,
            silk_to_pad: self.data.board.rules.silk_to_pad,
        };
        let fp = lamina_footprint::from_package(&p.package, &opt, p.id.as_deref())
            .map_err(Error::Invalid)?;
        let summary = format!(
            "footprint `{}`: {} pads from IPC-7351 {}",
            fp.id,
            fp.pads.len(),
            p.package
        );
        self.store_footprint(fp, p.overwrite, summary)
    }

    pub fn footprint_from_pads(&mut self, p: FootprintFromPads) -> Result<OpResult> {
        let mut fp = Footprint::new(p.id, p.provenance);
        fp.description = p.description;
        fp.pads = p.pads;
        fp.graphics = p.graphics;
        fp.keepouts = p.keepouts;
        fp.height = p.height;
        for k in &fp.keepouts {
            k.outline
                .check()
                .map_err(|e| Error::invalid(format!("keepout: {e}")))?;
        }
        let opt = Options {
            silk_width: self.data.board.rules.silk_width,
            silk_to_pad: self.data.board.rules.silk_to_pad,
            ..Options::default()
        };
        let fp = lamina_footprint::from_pads(fp, p.body.map(|b| (b.width, b.height)), &opt)
            .map_err(Error::Invalid)?;
        let summary = format!("footprint `{}`: {} pads", fp.id, fp.pads.len());
        self.store_footprint(fp, p.overwrite, summary)
    }

    pub fn get_footprint(&self, id: &str) -> Result<Footprint> {
        self.data
            .footprints
            .get(id)
            .cloned()
            .ok_or_else(|| Error::invalid(format!("The footprint `{id}` does not exist.")))
    }

    pub fn list_footprints(&self) -> Vec<FootprintSummary> {
        self.data
            .footprints
            .values()
            .map(|f| FootprintSummary {
                id: f.id.clone(),
                pads: f.pads.len(),
                source: match &f.provenance {
                    Provenance::Ipc7351 { package, .. } => format!("ipc7351 {package}"),
                    Provenance::Datasheet { document, .. } => format!("datasheet {document}"),
                    Provenance::Lcsc { part, .. } => format!("lcsc {part}"),
                    Provenance::Kicad {
                        library, footprint, ..
                    } => format!("kicad {library}:{footprint}"),
                    Provenance::Manual { .. } => "manual".into(),
                },
                size: f.courtyard_bbox(),
                has_model: f.model.is_some(),
                used_by: self
                    .data
                    .parts
                    .parts
                    .iter()
                    .filter(|(_, p)| p.footprint == f.id)
                    .map(|(r, _)| r.clone())
                    .collect(),
            })
            .collect()
    }

    fn insert_part(d: &mut ProjectData, p: AddPart) -> Result<String> {
        if !valid_refdes(&p.refdes) {
            return Err(Error::invalid(format!(
                "The refdes `{}` is not valid. Start with a letter, then use only letters, \
                 digits, `_` and `-`.",
                p.refdes
            )));
        }
        if !d.footprints.contains_key(&p.footprint) {
            return Err(Error::invalid(format!(
                "The footprint `{}` does not exist. Create it first with \
                 footprint_from_package, footprint_from_pads or import_part.",
                p.footprint
            )));
        }
        if d.parts.parts.contains_key(&p.refdes) && !p.replace {
            return Err(Error::invalid(format!(
                "{} already exists. Set replace: true to change it.",
                p.refdes
            )));
        }
        let part = Part {
            footprint: p.footprint,
            value: p.value,
            mpn: p.mpn,
            manufacturer: p.manufacturer,
            lcsc: p.lcsc,
            description: p.description,
            dnp: p.dnp,
            fields: p.fields,
            group: p.group,
        };
        d.parts.parts.insert(p.refdes.clone(), part);
        Ok(p.refdes)
    }

    pub fn add_part(&mut self, p: AddPart) -> Result<OpResult> {
        let strict = p.strict;
        let mut d = self.data.clone();
        let r = Self::insert_part(&mut d, p)?;
        self.commit(
            d,
            Touched::file(FileKind::Parts),
            strict,
            format!("part {r} added"),
        )
    }

    pub fn add_parts(&mut self, p: AddParts) -> Result<OpResult> {
        let mut d = self.data.clone();
        let mut names = Vec::new();
        for part in p.parts {
            names.push(Self::insert_part(&mut d, part)?);
        }
        self.commit(
            d,
            Touched::file(FileKind::Parts),
            p.strict,
            format!("{} part(s) added: {}", names.len(), names.join(" ")),
        )
    }

    pub fn remove_part(&mut self, p: RemovePart) -> Result<OpResult> {
        let mut d = self.data.clone();
        if d.parts.parts.remove(&p.refdes).is_none() {
            return Err(Error::invalid(format!(
                "The part {} does not exist.",
                p.refdes
            )));
        }
        let mut t = Touched::file(FileKind::Parts);
        if d.placement.placements.remove(&p.refdes).is_some() {
            t = t.and(FileKind::Placement);
        }
        if p.remove_from_nets {
            for net in d.nets.nets.values_mut() {
                net.pads.retain(|x| x.part != p.refdes);
            }
            t = t.and(FileKind::Nets);
        }
        self.commit(d, t, p.strict, format!("part {} removed", p.refdes))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SetModel {
    /// The footprint to change.
    pub footprint: String,
    /// The model file (STEP or OBJ). Lamina copies a file with an absolute path
    /// into `cache/models/user/`. A relative path is relative to `cache/models/`.
    /// `provider:id` (for example `kicad:LED_SMD:LED_0603_1608Metric`) takes a
    /// model from a provider: local files first, then online.
    pub file: String,
    /// The offset in mm. Lamina applies it after the rotation.
    #[serde(default)]
    pub offset: [f64; 3],
    /// Rotation in degrees about X, then Y, then Z.
    #[serde(default)]
    pub rotation: [f64; 3],
}

impl Project {
    pub fn set_model(&mut self, p: SetModel) -> Result<OpResult> {
        if !self.data.footprints.contains_key(&p.footprint) {
            return Err(Error::invalid(format!(
                "The footprint `{}` does not exist.",
                p.footprint
            )));
        }
        let src = std::path::Path::new(&p.file);
        let rel = if src.is_absolute() {
            let name = src
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| Error::invalid("The model path has no file name."))?;
            let rel = format!("user/{name}");
            let dst = self.root().join(MODEL_DIR).join(&rel);
            if src != dst {
                std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| Error::io(&dst, e))?;
                std::fs::copy(src, &dst).map_err(|e| Error::io(src, e))?;
            }
            rel
        } else if lamina_model::provider::split_name(&p.file).is_some() {
            self.provider_model_into_cache(&p.file)?
        } else {
            if p.file.contains("..") {
                return Err(Error::invalid(
                    "A relative model path must stay in cache/models. Do not use `..` in it.",
                ));
            }
            p.file.replace('\\', "/")
        };
        let path = self.root().join(MODEL_DIR).join(&rel);
        let mesh = lamina_step::load(&path)
            .map_err(|e| Error::invalid(format!("Lamina cannot load the model: {e}")))?;
        let (lo, hi) = mesh.bounds().unwrap_or(([0.0; 3], [0.0; 3]));
        let mut d = self.data.clone();
        let fp = d.footprints.get_mut(&p.footprint).unwrap();
        fp.model = Some(ModelRef {
            file: rel.clone(),
            offset: p.offset,
            rotation: p.rotation,
            scale: [1.0; 3],
        });
        let id = p.footprint.clone();
        let mut r = self.commit(
            d,
            Touched::default().footprint(id.clone()),
            false,
            format!("footprint `{id}` now uses model {rel}"),
        )?;
        r.notes.push(format!(
            "Model bounds before the offset: min ({:.3}, {:.3}, {:.3}), \
             max ({:.3}, {:.3}, {:.3}), {} triangles.",
            lo[0],
            lo[1],
            lo[2],
            hi[0],
            hi[1],
            hi[2],
            mesh.triangles()
        ));
        Ok(r)
    }
}
