//! Triangle meshes shared by the OBJ and STEP loaders.

/// A triangle mesh split into parts with one colour each.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub parts: Vec<MeshPart>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshPart {
    /// Linear RGB, 0..1.
    pub color: [f32; 3],
    /// Flat-shaded triangles: three positions and three normals per triangle.
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
}

impl Mesh {
    pub fn triangles(&self) -> usize {
        self.parts.iter().map(|p| p.positions.len() / 3).sum()
    }

    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        let mut any = false;
        for p in self.parts.iter().flat_map(|p| p.positions.iter()) {
            any = true;
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        any.then_some((lo, hi))
    }

    /// Merge parts with the same colour.
    pub fn compact(mut self) -> Mesh {
        let mut out: Vec<MeshPart> = Vec::new();
        for p in self.parts.drain(..) {
            if p.positions.is_empty() {
                continue;
            }
            match out.iter_mut().find(|q| q.color == p.color) {
                Some(q) => {
                    q.positions.extend(p.positions);
                    q.normals.extend(p.normals);
                }
                None => out.push(p),
            }
        }
        Mesh { parts: out }
    }

    /// Serialize to a small binary format for the mesh cache.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"LMSH");
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&(self.parts.len() as u32).to_le_bytes());
        for p in &self.parts {
            for c in p.color {
                b.extend_from_slice(&c.to_le_bytes());
            }
            b.extend_from_slice(&(p.positions.len() as u32).to_le_bytes());
            for v in p.positions.iter().chain(p.normals.iter()) {
                for c in v {
                    b.extend_from_slice(&c.to_le_bytes());
                }
            }
        }
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<Mesh> {
        let mut r = Reader { b, i: 0 };
        if r.take(4)? != b"LMSH" || r.u32()? != 1 {
            return None;
        }
        let n = r.u32()? as usize;
        let mut parts = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            let color = [r.f32()?, r.f32()?, r.f32()?];
            let k = r.u32()? as usize;
            if k > 50_000_000 {
                return None;
            }
            let mut read = || -> Option<Vec<[f32; 3]>> {
                let mut v = Vec::with_capacity(k);
                for _ in 0..k {
                    v.push([r.f32()?, r.f32()?, r.f32()?]);
                }
                Some(v)
            };
            let positions = read()?;
            let normals = read()?;
            parts.push(MeshPart {
                color,
                positions,
                normals,
            });
        }
        Some(Mesh { parts })
    }
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.i..self.i + n)?;
        self.i += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}

pub fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l < 1e-20 {
        [0.0, 0.0, 1.0]
    } else {
        [n[0] / l, n[1] / l, n[2] / l]
    }
}

impl MeshPart {
    pub fn push_triangle(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3]) {
        let n = face_normal(a, b, c);
        self.positions.extend_from_slice(&[a, b, c]);
        self.normals.extend_from_slice(&[n, n, n]);
    }

    /// Add a triangle with per-vertex normals.
    pub fn push_smooth(&mut self, p: [[f32; 3]; 3], n: [[f32; 3]; 3]) {
        self.positions.extend_from_slice(&p);
        self.normals.extend_from_slice(&n);
    }
}

// ---------------------------------------------------------------- ports

use std::path::Path;

use lamina_model::ProjectData;

/// Reads a 3D model file (STEP or OBJ) into a mesh.
pub trait MeshLoader: Send + Sync {
    fn load(&self, path: &Path) -> Result<Mesh, String>;
}

#[derive(Debug, Clone)]
pub struct StepOptions {
    /// Also drill the vias through the board. This is off by default, because
    /// there are many tiny holes.
    pub vias: bool,
    /// Include the part models (default true).
    pub parts: bool,
}

impl Default for StepOptions {
    fn default() -> Self {
        StepOptions {
            vias: false,
            parts: true,
        }
    }
}

#[derive(Debug, Default)]
pub struct StepExport {
    /// The file, as bytes and not as text. Copied part files can have names in
    /// other encodings. The export keeps these names exactly as they are.
    pub bytes: Vec<u8>,
    /// The number of parts with their exact STEP model.
    pub exact: usize,
    /// The number of parts as faceted meshes.
    pub meshed: usize,
    pub notes: Vec<String>,
}

/// Writes the assembled board as a STEP file. `root` is the board folder,
/// where the model files are.
pub trait StepWriter: Send + Sync {
    fn export(
        &self,
        data: &ProjectData,
        root: &Path,
        name: &str,
        options: &StepOptions,
    ) -> Result<StepExport, String>;
}
