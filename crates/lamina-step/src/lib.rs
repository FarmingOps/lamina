//! 3D models: load and tessellate OBJ and STEP files, and keep a mesh cache.

pub use lamina_ports::mesh;
pub mod obj;
pub mod p21;
pub mod step;

pub use lamina_ports::mesh::{Mesh, MeshPart};

use std::path::Path;

/// Load a model file (OBJ or STEP). If the source has not changed, use the
/// cached mesh next to it.
pub fn load(path: &Path) -> Result<Mesh, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "obj" => obj::parse(&String::from_utf8_lossy(&bytes)),
        "step" | "stp" => {
            let hash = fnv(&bytes);
            let cache = path.with_extension(format!("{ext}.{hash:016x}.mesh"));
            if let Ok(b) = std::fs::read(&cache) {
                if let Some(m) = Mesh::from_bytes(&b) {
                    return Ok(m);
                }
            }
            let m = step::load(&bytes)?;
            let _ = std::fs::write(&cache, m.to_bytes());
            Ok(m)
        }
        _ => Err(format!("{}: unknown model format", path.display())),
    }
}

fn fnv(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for x in b {
        h ^= *x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
