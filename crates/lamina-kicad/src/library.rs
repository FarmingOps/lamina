//! The sources of KiCad footprints and 3D models: local `*.pretty` and
//! `*.3dshapes` folders, or the official online libraries.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct Hit {
    pub library: String,
    pub name: String,
    pub path: String,
}

const VERSIONS: [&str; 5] = ["10", "9", "8", "7", "6"];

fn env_dirs(suffix: &str, legacy: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for v in VERSIONS {
        if let Some(p) = std::env::var_os(format!("KICAD{v}_{suffix}")) {
            out.push(PathBuf::from(p));
        }
    }
    if let Some(p) = std::env::var_os(legacy) {
        out.push(PathBuf::from(p));
    }
    out
}

fn standard_dirs(kind: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        for v in VERSIONS {
            out.push(PathBuf::from(format!(
                r"C:\Program Files\KiCad\{v}.0\share\kicad\{kind}"
            )));
            // An install for the current user only (winget).
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                out.push(
                    PathBuf::from(local).join(format!(r"Programs\KiCad\{v}.0\share\kicad\{kind}")),
                );
            }
        }
    } else if cfg!(target_os = "macos") {
        out.push(PathBuf::from(format!(
            "/Applications/KiCad/KiCad.app/Contents/SharedSupport/{kind}"
        )));
    } else {
        out.push(PathBuf::from(format!("/usr/share/kicad/{kind}")));
        out.push(PathBuf::from(format!("/usr/local/share/kicad/{kind}")));
    }
    out
}

/// Footprint library roots from the environment and the standard installations.
pub fn env_footprint_dirs() -> Vec<PathBuf> {
    let mut v = env_dirs("FOOTPRINT_DIR", "KISYSMOD");
    v.extend(standard_dirs("footprints"));
    v.retain(|p| p.is_dir());
    v
}

/// 3D model library roots from the environment and the standard installations.
pub fn env_model_dirs() -> Vec<PathBuf> {
    let mut v = env_dirs("3DMODEL_DIR", "KISYS3DMOD");
    v.extend(standard_dirs("3dmodels"));
    v.retain(|p| p.is_dir());
    v
}

/// All `*.pretty` libraries under the roots. A root can also be a library.
pub fn pretty_libs(roots: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let nick = |p: &Path| {
        p.file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_suffix(".pretty"))
            .map(str::to_string)
    };
    for root in roots {
        if let Some(n) = nick(root) {
            out.push((n, root.clone()));
            continue;
        }
        let Ok(rd) = std::fs::read_dir(root) else {
            continue;
        };
        let mut libs: Vec<(String, PathBuf)> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .filter_map(|p| nick(&p).map(|n| (n, p)))
            .collect();
        libs.sort();
        out.extend(libs);
    }
    out
}

/// Footprints whose `Lib:Name` contains every word of `query`.
pub fn search(roots: &[PathBuf], query: &str, limit: usize) -> Vec<Hit> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    let mut out = Vec::new();
    for (lib, dir) in pretty_libs(roots) {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "kicad_mod"))
            .collect();
        files.sort();
        for f in files {
            let Some(name) = f.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let hay = format!("{lib}:{name}").to_lowercase();
            if words.iter().all(|w| hay.contains(w.as_str())) {
                out.push(Hit {
                    library: lib.clone(),
                    name: name.to_string(),
                    path: f.to_string_lossy().replace('\\', "/"),
                });
                if out.len() >= limit {
                    return out;
                }
            }
        }
    }
    out
}

/// A footprint by `Lib:Name`, or by name only, in all libraries.
pub fn find(roots: &[PathBuf], spec: &str) -> Option<Hit> {
    let (lib, name) = match spec.split_once(':') {
        Some((l, n)) => (Some(l), n),
        None => (None, spec),
    };
    for (nick, dir) in pretty_libs(roots) {
        if lib.is_some_and(|l| l != nick) {
            continue;
        }
        let f = dir.join(format!("{name}.kicad_mod"));
        if f.is_file() {
            return Some(Hit {
                library: nick,
                name: name.to_string(),
                path: f.to_string_lossy().replace('\\', "/"),
            });
        }
    }
    None
}

/// Strip KiCad's `${KICAD*_3DMODEL_DIR}/` prefix from a model path.
pub fn model_relative(kicad_path: &str) -> String {
    let p = kicad_path.replace('\\', "/");
    match p.find("}/") {
        Some(i) if p.starts_with("${") => p[i + 2..].to_string(),
        _ => p.trim_start_matches("./").to_string(),
    }
}

/// `(library, name)` of a model path like `Resistor_SMD.3dshapes/R_0603.step`.
pub fn model_lib_name(kicad_path: &str) -> Option<(String, String)> {
    let rel = model_relative(kicad_path);
    let (dir, file) = rel.rsplit_once('/')?;
    // Use only the last folder. The pair is the name of a file in the model
    // cache, so `..` or a full path must not get into it.
    let last = dir.rsplit('/').next()?;
    let lib = last.strip_suffix(".3dshapes").unwrap_or(last).to_string();
    let name = file
        .rsplit_once('.')
        .map(|(n, _)| n)
        .unwrap_or(file)
        .to_string();
    let plain = |s: &str| !s.is_empty() && s != "." && s != "..";
    (plain(&lib) && plain(&name)).then_some((lib, name))
}

/// A local file for a model path. STEP has priority over VRML.
pub fn resolve_model(roots: &[PathBuf], kicad_path: &str) -> Option<PathBuf> {
    let rel = model_relative(kicad_path);
    let stem = rel
        .rsplit_once('.')
        .map(|(s, _)| s.to_string())
        .unwrap_or(rel.clone());
    let mut candidates = vec![];
    for ext in ["step", "STEP", "stp", "STP"] {
        candidates.push(format!("{stem}.{ext}"));
    }
    candidates.push(rel.clone());
    for root in roots {
        for c in &candidates {
            let p = root.join(c);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // Use the path directly only if it is absolute. A relative path changes
    // with the current directory.
    let direct = PathBuf::from(&rel);
    (direct.is_absolute() && direct.is_file()).then_some(direct)
}

/// A source to download footprints and models from, when they are not local.
pub trait KicadSource: Send + Sync {
    fn footprint(&self, library: &str, name: &str) -> Result<String, String>;
    /// STEP bytes of a model.
    fn model(&self, library: &str, name: &str) -> Result<Vec<u8>, String>;
    fn describe(&self) -> String;
}

/// The official KiCad libraries on GitLab.
#[derive(Debug, Default)]
pub struct HttpSource;

const FOOTPRINTS: &str = "https://gitlab.com/kicad/libraries/kicad-footprints/-/raw/master";
const MODELS: &str = "https://gitlab.com/kicad/libraries/kicad-packages3D/-/raw/master";

fn get(url: &str) -> Result<Vec<u8>, String> {
    let mut resp = ureq::get(url)
        .header("User-Agent", "lamina/0.1")
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    resp.body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| format!("{url}: {e}"))
}

impl KicadSource for HttpSource {
    fn footprint(&self, library: &str, name: &str) -> Result<String, String> {
        let bytes = get(&format!("{FOOTPRINTS}/{library}.pretty/{name}.kicad_mod"))?;
        String::from_utf8(bytes).map_err(|e| e.to_string())
    }

    fn model(&self, library: &str, name: &str) -> Result<Vec<u8>, String> {
        get(&format!("{MODELS}/{library}.3dshapes/{name}.step"))
    }

    fn describe(&self) -> String {
        "the official KiCad libraries (gitlab.com/kicad/libraries)".into()
    }
}

/// Folders with the same layout as the libraries, for tests and offline mirrors.
#[derive(Debug, Clone)]
pub struct DirSource {
    pub footprints: PathBuf,
    pub models: PathBuf,
}

impl KicadSource for DirSource {
    fn footprint(&self, library: &str, name: &str) -> Result<String, String> {
        let p = self
            .footprints
            .join(format!("{library}.pretty/{name}.kicad_mod"));
        std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))
    }

    fn model(&self, library: &str, name: &str) -> Result<Vec<u8>, String> {
        let p = self.models.join(format!("{library}.3dshapes/{name}.step"));
        std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))
    }

    fn describe(&self) -> String {
        self.footprints.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/footprints")
    }

    #[test]
    fn search_and_find() {
        let roots = vec![fixtures()];
        let libs = pretty_libs(&roots);
        assert!(libs.iter().any(|(n, _)| n == "Resistor_SMD"));
        let hits = search(&roots, "0603", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].library, "Resistor_SMD");
        let hits = search(&roots, "usb c", 10);
        assert_eq!(hits[0].name, "USB_C_Receptacle_HRO_TYPE-C-31-M-12");
        assert!(find(&roots, "Package_TO_SOT_SMD:SOT-23").is_some());
        assert!(find(&roots, "SOT-23").is_some());
        assert!(find(&roots, "Resistor_SMD:SOT-23").is_none());
        // A .pretty folder that is given directly also works.
        let direct = vec![fixtures().join("LED_SMD.pretty")];
        assert_eq!(search(&direct, "led", 5).len(), 1);
    }

    #[test]
    fn model_paths() {
        let p = "${KICAD9_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.wrl";
        assert_eq!(
            model_relative(p),
            "Resistor_SMD.3dshapes/R_0603_1608Metric.wrl"
        );
        assert_eq!(
            model_lib_name(p),
            Some(("Resistor_SMD".into(), "R_0603_1608Metric".into()))
        );
        // For a path outside the project folder, the name uses only its last folder.
        assert_eq!(
            model_lib_name("${KIPRJMOD}/../../models/socket/DS1009.step"),
            Some(("socket".into(), "DS1009".into()))
        );
        assert_eq!(model_lib_name("${KIPRJMOD}/../x.step"), None);
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("Resistor_SMD.3dshapes");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("R_0603_1608Metric.wrl"), "x").unwrap();
        assert!(
            resolve_model(&[t.path().to_path_buf()], p)
                .unwrap()
                .ends_with("R_0603_1608Metric.wrl")
        );
        std::fs::write(d.join("R_0603_1608Metric.step"), "x").unwrap();
        assert!(
            resolve_model(&[t.path().to_path_buf()], p)
                .unwrap()
                .ends_with("R_0603_1608Metric.step")
        );
    }
}
