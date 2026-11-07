//! The parts vendor as a [`Provider`]: search, footprint, 3D model and the
//! order code of each part.

use std::sync::Arc;

use lamina_model::provider::{Bundle, Hit, ModelFile, PartInfo, Provider, ProviderInfo};

use crate::source::{HttpSource, PartSource};

pub const ID: &str = "lcsc";

pub struct LcscProvider {
    source: Arc<dyn PartSource>,
}

impl LcscProvider {
    pub fn new(source: Arc<dyn PartSource>) -> LcscProvider {
        LcscProvider { source }
    }

    pub fn online() -> LcscProvider {
        LcscProvider::new(Arc::new(HttpSource::default()))
    }
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

impl Provider for LcscProvider {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "LCSC / JLCPCB parts".into(),
            description: "Real parts with stock, price, the vendor footprint and the \
                          vendor 3D model. The id is the LCSC number. Search by keywords, \
                          an MPN or an LCSC number."
                .into(),
            example: "lcsc:C6186".into(),
            online: true,
        }
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>, String> {
        let hits = self.source.search(query, limit)?;
        Ok(hits
            .into_iter()
            .map(|h| Hit {
                provider: ID.into(),
                id: h.lcsc,
                title: h.mpn,
                package: h.package,
                manufacturer: h.manufacturer,
                description: h.description,
                stock: Some(h.stock),
                price: h.price,
                basic: Some(h.basic),
                datasheet: h.datasheet,
                local: false,
            })
            .collect())
    }

    fn fetch(&self, id: &str, model: bool) -> Result<Bundle, String> {
        let lcsc = id.trim().to_ascii_uppercase();
        let raw = self.source.component(&lcsc)?;
        let im = crate::convert(&raw, &lcsc, &today())?;
        let mut notes = im.notes;
        let mut model_file = None;
        match (&im.model, model) {
            (Some(info), true) => match self.source.model_obj(&info.uuid) {
                Ok(obj) => {
                    let height = crate::model_height(info, &obj);
                    // The STEP file has the same geometry in the same frame,
                    // with smooth surfaces. Use it when it downloads.
                    let (name, bytes) = match self.source.model_step(&info.uuid) {
                        Ok(b) if !b.is_empty() => (format!("{}.step", info.uuid), b),
                        _ => (format!("{}.obj", info.uuid), obj.clone().into_bytes()),
                    };
                    let r = crate::model_ref(info, &obj, name.clone());
                    notes.push(format!("3D model `{}`", info.title));
                    model_file = Some(ModelFile {
                        name,
                        bytes,
                        offset: r.offset,
                        rotation: r.rotation,
                        scale: r.scale,
                        height,
                    });
                }
                Err(e) => notes.push(format!("The 3D model was not downloaded: {e}")),
            },
            (None, _) => notes.push("The vendor has no 3D model for this footprint.".into()),
            (Some(_), false) => {}
        }
        let m = im.meta;
        Ok(Bundle {
            footprint: im.footprint,
            model: model_file,
            info: PartInfo {
                mpn: m.mpn,
                manufacturer: m.manufacturer,
                package: m.package,
                description: m.description,
                basic: m.basic,
                datasheet: m.datasheet,
                order_code: Some(lcsc),
            },
            notes,
        })
    }
}
