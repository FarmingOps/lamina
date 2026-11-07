//! The KiCad libraries as a [`Provider`]: generic footprints and package
//! models from the folders on this computer, or from the official online
//! libraries.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lamina_model::provider::{Bundle, Hit, ModelFile, PartInfo, Provider, ProviderInfo};

use crate::library::{self, HttpSource, KicadSource};

pub const ID: &str = "kicad";

pub struct KicadProvider {
    source: Arc<dyn KicadSource>,
    footprint_dirs: Vec<PathBuf>,
    model_dirs: Vec<PathBuf>,
}

impl KicadProvider {
    /// A provider with these local roots and this online source.
    pub fn new(
        source: Arc<dyn KicadSource>,
        footprint_dirs: Vec<PathBuf>,
        model_dirs: Vec<PathBuf>,
    ) -> KicadProvider {
        KicadProvider {
            source,
            footprint_dirs,
            model_dirs,
        }
    }

    /// The official online libraries, after the given local roots and the
    /// roots of a KiCad installation.
    pub fn online(mut footprint_dirs: Vec<PathBuf>, mut model_dirs: Vec<PathBuf>) -> KicadProvider {
        footprint_dirs.extend(library::env_footprint_dirs());
        model_dirs.extend(library::env_model_dirs());
        KicadProvider::new(Arc::new(HttpSource), footprint_dirs, model_dirs)
    }

    /// The STEP of a model path: local roots first, then online.
    fn model_bytes(
        &self,
        kicad_path: &str,
        extra_root: Option<&Path>,
        online: bool,
    ) -> Result<(String, Vec<u8>), String> {
        let (lib, name) = library::model_lib_name(kicad_path)
            .ok_or_else(|| format!("`{kicad_path}` is not a model path."))?;
        let file = format!("{lib}/{name}.step");
        let mut roots = self.model_dirs.clone();
        if let Some(r) = extra_root {
            roots.insert(0, r.to_path_buf());
        }
        if let Some(p) = library::resolve_model(&roots, kicad_path) {
            let is_step = p
                .extension()
                .is_some_and(|e| matches!(e.to_str(), Some("step" | "STEP" | "stp" | "STP")));
            if is_step {
                let b = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                return Ok((file, b));
            }
            if !online {
                return Err(format!(
                    "only a VRML model was found ({}); Lamina does not read VRML",
                    p.display()
                ));
            }
        }
        if !online {
            return Err(format!("{lib}/{name} is not in the local libraries"));
        }
        let b = self.source.model(&lib, &name)?;
        Ok((file, b))
    }
}

impl Provider for KicadProvider {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "KiCad libraries".into(),
            description: "Generic footprints by package, with generic 3D models. The id \
                          is `Library:Name`, a footprint name in the local libraries, or \
                          the path of a `.kicad_mod` file. Search looks in the local \
                          libraries only. A model id is `Library:Name` or the path of \
                          a STEP file, with `?offset=x,y,z&rotation=x,y,z&scale=x,y,z` \
                          when the placement is not the identity."
                .into(),
            example: "kicad:LED_SMD:LED_0603_1608Metric".into(),
            online: true,
        }
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>, String> {
        if library::pretty_libs(&self.footprint_dirs).is_empty() {
            return Err(
                "There are no local KiCad libraries to search. Set kicad_footprints in \
                        lamina.json, install KiCad, or import `kicad:Library:Name` directly."
                    .into(),
            );
        }
        Ok(library::search(&self.footprint_dirs, query, limit)
            .into_iter()
            .map(|h| Hit {
                provider: ID.into(),
                id: format!("{}:{}", h.library, h.name),
                title: h.name,
                package: Some(h.library),
                manufacturer: None,
                description: None,
                stock: None,
                price: None,
                basic: None,
                datasheet: None,
                local: true,
            })
            .collect())
    }

    fn fetch(&self, id: &str, model: bool) -> Result<Bundle, String> {
        let spec = id.trim();
        let as_path = Path::new(spec);
        // The folder above a `.pretty` folder is the KiCad project of a file.
        let mut project_dir = None;
        let (text, library, origin) = if as_path.is_file() {
            let parent = as_path.parent();
            let lib = parent
                .and_then(|d| d.file_name())
                .and_then(|s| s.to_str())
                .map(|s| s.trim_end_matches(".pretty").to_string())
                .unwrap_or_default();
            project_dir = parent.map(|d| match d.extension() {
                Some(e) if e == "pretty" => d.parent().unwrap_or(d).to_path_buf(),
                _ => d.to_path_buf(),
            });
            let text = std::fs::read_to_string(as_path)
                .map_err(|e| format!("{}: {e}", as_path.display()))?;
            (text, lib, spec.replace('\\', "/"))
        } else if let Some(hit) = library::find(&self.footprint_dirs, spec) {
            let text =
                std::fs::read_to_string(&hit.path).map_err(|e| format!("{}: {e}", hit.path))?;
            (text, hit.library, hit.path)
        } else {
            let (lib, name) = spec.split_once(':').ok_or_else(|| {
                format!(
                    "`{spec}` was not found in the local libraries. Give `Library:Name` \
                     to get it from the online libraries."
                )
            })?;
            let text = self.source.footprint(lib, name)?;
            (
                text,
                lib.to_string(),
                format!("kicad-footprints/{lib}.pretty/{name}.kicad_mod"),
            )
        };
        let inst = crate::parse_footprint(&text, &library, &origin)?;
        let mut notes = inst.notes.clone();
        let mut model_file = None;
        if model {
            if inst.models.is_empty() {
                notes.push("The footprint names no 3D model.".into());
            }
            let mut tries = Vec::new();
            for m in &inst.models {
                let in_project = m.path.starts_with("${KIPRJMOD}")
                    || (!m.path.starts_with("${") && !Path::new(&m.path).is_absolute());
                match self.model_bytes(&m.path, project_dir.as_deref(), !in_project) {
                    Ok((name, bytes)) => {
                        model_file = Some(ModelFile {
                            name,
                            bytes,
                            offset: m.offset,
                            rotation: m.rotation,
                            scale: m.scale,
                            height: None,
                        });
                        break;
                    }
                    Err(e) => tries.push(format!("3D model {}: {e}", m.path)),
                }
            }
            if model_file.is_none() {
                notes.extend(tries);
            }
        }
        Ok(Bundle {
            footprint: inst.footprint,
            model: model_file,
            info: PartInfo {
                package: Some(inst.name.clone()),
                description: Some(format!("KiCad footprint {}:{}", inst.library, inst.name)),
                ..Default::default()
            },
            notes,
        })
    }

    fn model(&self, id: &str) -> Result<ModelFile, String> {
        let ModelRequest {
            spec,
            offset,
            rotation,
            scale,
        } = parse_model_id(id)?;
        let as_path = Path::new(&spec);
        let (file, bytes) = if as_path.is_absolute() {
            let (lib, name) = library::model_lib_name(&spec)
                .ok_or_else(|| format!("`{spec}` is not a model path."))?;
            let is_step = as_path
                .extension()
                .is_some_and(|e| matches!(e.to_str(), Some("step" | "STEP" | "stp" | "STP")));
            if !is_step {
                return Err(format!(
                    "only a VRML model was found ({spec}); Lamina does not read VRML"
                ));
            }
            let b = std::fs::read(as_path).map_err(|e| format!("{spec}: {e}"))?;
            (format!("{lib}/{name}.step"), b)
        } else {
            let (lib, name) = spec
                .split_once(':')
                .or_else(|| spec.split_once('/'))
                .filter(|(l, n)| !l.is_empty() && !n.is_empty())
                .ok_or_else(|| {
                    format!(
                        "`{spec}` must be `Library:Name`, for example `LED_SMD:LED_0603_1608Metric`."
                    )
                })?;
            self.model_bytes(&format!("{lib}.3dshapes/{name}.step"), None, true)?
        };
        Ok(ModelFile {
            name: file,
            bytes,
            offset,
            rotation,
            scale,
            height: None,
        })
    }
}

/// A model id for [`Provider::model`]: `Library:Name`, or the full path of a
/// STEP file, with the placement after `?` when it is not the identity:
/// `offset=x,y,z&rotation=x,y,z&scale=x,y,z`.
pub fn model_id(spec: &str, offset: [f64; 3], rotation: [f64; 3], scale: [f64; 3]) -> String {
    let xyz = |v: [f64; 3]| format!("{},{},{}", v[0], v[1], v[2]);
    let mut q = Vec::new();
    if offset != [0.0; 3] {
        q.push(format!("offset={}", xyz(offset)));
    }
    if rotation != [0.0; 3] {
        q.push(format!("rotation={}", xyz(rotation)));
    }
    if scale != [1.0; 3] {
        q.push(format!("scale={}", xyz(scale)));
    }
    if q.is_empty() {
        spec.to_string()
    } else {
        format!("{spec}?{}", q.join("&"))
    }
}

/// A model id, read back: the spec and the placement.
struct ModelRequest {
    spec: String,
    offset: [f64; 3],
    rotation: [f64; 3],
    scale: [f64; 3],
}

/// Split a model id into its spec and its placement. See [`model_id`].
fn parse_model_id(id: &str) -> Result<ModelRequest, String> {
    let id = id.trim();
    let (spec, query) = id.split_once('?').unwrap_or((id, ""));
    let mut offset = [0.0; 3];
    let mut rotation = [0.0; 3];
    let mut scale = [1.0; 3];
    for item in query.split('&').filter(|s| !s.is_empty()) {
        let (key, value) = item
            .split_once('=')
            .ok_or_else(|| format!("`{item}` in `{id}` is not `key=x,y,z`."))?;
        let mut v = [0.0; 3];
        let parts: Vec<&str> = value.split(',').collect();
        if parts.len() != 3 {
            return Err(format!("`{key}` in `{id}` needs three numbers."));
        }
        for (slot, s) in v.iter_mut().zip(parts) {
            *slot = s
                .trim()
                .parse()
                .map_err(|_| format!("`{s}` in `{id}` is not a number."))?;
        }
        match key {
            "offset" => offset = v,
            "rotation" => rotation = v,
            "scale" => scale = v,
            _ => return Err(format!("`{key}` in `{id}` is not a placement key.")),
        }
    }
    Ok(ModelRequest {
        spec: spec.to_string(),
        offset,
        rotation,
        scale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_ids_round_trip() {
        let id = model_id("Lib:Name", [0.0, 0.0, 1.0], [0.0; 3], [1.0; 3]);
        assert_eq!(id, "Lib:Name?offset=0,0,1");
        let m = parse_model_id(&id).unwrap();
        assert_eq!(m.spec, "Lib:Name");
        assert_eq!(
            (m.offset, m.rotation, m.scale),
            ([0.0, 0.0, 1.0], [0.0; 3], [1.0; 3])
        );
        assert_eq!(
            model_id("Lib:Name", [0.0; 3], [0.0; 3], [1.0; 3]),
            "Lib:Name"
        );
        assert!(parse_model_id("Lib:Name?offset=1,2").is_err());
        assert!(parse_model_id("Lib:Name?tilt=1,2,3").is_err());
    }
}
