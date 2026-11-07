//! The parts vendor as a [`Provider`]: search, footprint, 3D model and the
//! order code of each part.

use std::sync::Arc;

use lamina_model::Point;
use lamina_model::provider::{Bundle, Hit, ModelFile, PartInfo, Provider, ProviderInfo};

use crate::ModelInfo;
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

    /// The model file of a model record. The STEP file has the same geometry
    /// in the same frame as the OBJ, with smooth surfaces. Use it when it
    /// downloads. The OBJ gives the placement and the height in both cases.
    fn model_file(&self, info: &ModelInfo) -> Result<ModelFile, String> {
        let obj = self.source.model_obj(&info.uuid)?;
        let height = crate::model_height(info, &obj);
        let (name, bytes) = match self.source.model_step(&info.uuid) {
            Ok(b) if !b.is_empty() => (format!("{}.step", info.uuid), b),
            _ => (format!("{}.obj", info.uuid), obj.clone().into_bytes()),
        };
        let r = crate::model_ref(info, &obj, name.clone());
        Ok(ModelFile {
            name,
            bytes,
            offset: r.offset,
            rotation: r.rotation,
            scale: r.scale,
            height,
        })
    }
}

/// A model id for [`Provider::model`]: the model uuid, then its placement
/// after `?`: `rotation=x,y,z&z=<mm>&center=x,y&title=<text>`. Each key is
/// optional. The board import makes these ids. An LCSC number is also a
/// model id: it gives the model of that part.
pub fn model_id(info: &ModelInfo) -> String {
    let mut q = Vec::new();
    if info.rotation != [0.0; 3] {
        q.push(format!(
            "rotation={},{},{}",
            info.rotation[0], info.rotation[1], info.rotation[2]
        ));
    }
    if info.z != 0.0 {
        q.push(format!("z={}", info.z));
    }
    if let Some(c) = info.outline_center {
        q.push(format!("center={},{}", c.x, c.y));
    }
    if !info.title.is_empty() {
        q.push(format!("title={}", info.title.replace('&', " ")));
    }
    if q.is_empty() {
        info.uuid.clone()
    } else {
        format!("{}?{}", info.uuid, q.join("&"))
    }
}

/// Read a model id back. See [`model_id`].
fn parse_model_id(id: &str) -> Result<ModelInfo, String> {
    let id = id.trim();
    let (uuid, query) = id.split_once('?').unwrap_or((id, ""));
    if uuid.is_empty() || !uuid.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("`{uuid}` is not a model uuid."));
    }
    let mut info = ModelInfo {
        uuid: uuid.to_string(),
        title: uuid.to_string(),
        rotation: [0.0; 3],
        z: 0.0,
        outline_center: None,
    };
    let num = |s: &str| -> Result<f64, String> {
        s.trim()
            .parse()
            .map_err(|_| format!("`{s}` in `{id}` is not a number."))
    };
    for item in query.split('&').filter(|s| !s.is_empty()) {
        let (key, value) = item
            .split_once('=')
            .ok_or_else(|| format!("`{item}` in `{id}` is not `key=value`."))?;
        match key {
            "rotation" => {
                let v: Vec<&str> = value.split(',').collect();
                if v.len() != 3 {
                    return Err(format!("`rotation` in `{id}` needs three numbers."));
                }
                info.rotation = [num(v[0])?, num(v[1])?, num(v[2])?];
            }
            "z" => info.z = num(value)?,
            "center" => {
                let (x, y) = value
                    .split_once(',')
                    .ok_or_else(|| format!("`center` in `{id}` needs two numbers."))?;
                info.outline_center = Some(Point::new(num(x)?, num(y)?));
            }
            "title" => info.title = value.to_string(),
            _ => return Err(format!("`{key}` in `{id}` is not a model key.")),
        }
    }
    Ok(info)
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
            (Some(info), true) => match self.model_file(info) {
                Ok(m) => {
                    notes.push(format!("3D model `{}`", info.title));
                    model_file = Some(m);
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

    fn model(&self, id: &str) -> Result<ModelFile, String> {
        let id = id.trim();
        let is_part = id.len() > 1
            && id.starts_with(['C', 'c'])
            && id[1..].chars().all(|c| c.is_ascii_digit());
        if is_part {
            return self
                .fetch(id, true)?
                .model
                .ok_or_else(|| format!("{id} has no 3D model."));
        }
        self.model_file(&parse_model_id(id)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_ids_round_trip() {
        let info = ModelInfo {
            uuid: "abc123".into(),
            title: "SOT-223".into(),
            rotation: [0.0, 0.0, 180.0],
            z: 0.5,
            outline_center: Some(Point::new(0.1, -0.2)),
        };
        let id = model_id(&info);
        assert_eq!(
            id,
            "abc123?rotation=0,0,180&z=0.5&center=0.1,-0.2&title=SOT-223"
        );
        let back = parse_model_id(&id).unwrap();
        assert_eq!(back.rotation, info.rotation);
        assert_eq!(back.z, info.z);
        assert_eq!(back.outline_center, info.outline_center);
        assert_eq!(back.title, info.title);
        assert!(parse_model_id("abc?rotation=1").is_err());
        assert!(parse_model_id("a/b").is_err());
    }
}
