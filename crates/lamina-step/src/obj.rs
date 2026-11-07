//! Wavefront OBJ, also the EasyEDA variant with inline materials
//! (`newmtl` ... `endmtl` blocks in the OBJ file).

use std::collections::HashMap;

use lamina_ports::mesh::{Mesh, MeshPart};

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Parse OBJ text. A face with more than three vertices becomes a fan of triangles.
pub fn parse(text: &str) -> Result<Mesh, String> {
    let mut verts: Vec<[f32; 3]> = Vec::new();
    let mut materials: HashMap<String, [f32; 3]> = HashMap::new();
    let mut current_mtl: Option<String> = None;
    let mut parts: Vec<MeshPart> = Vec::new();
    let mut part_for: HashMap<String, usize> = HashMap::new();
    let default = [0.55, 0.55, 0.58];

    for (ln, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let mut it = line.split_whitespace();
        let Some(tag) = it.next() else { continue };
        match tag {
            "v" => {
                let v: Vec<f32> = it.take(3).filter_map(|t| t.parse().ok()).collect();
                if v.len() != 3 {
                    return Err(format!("line {}: the vertex is not valid", ln + 1));
                }
                verts.push([v[0], v[1], v[2]]);
            }
            "newmtl" => {
                current_mtl = it.next().map(str::to_string);
            }
            "Kd" => {
                let v: Vec<f32> = it.take(3).filter_map(|t| t.parse().ok()).collect();
                if let (Some(name), 3) = (&current_mtl, v.len()) {
                    // EasyEDA writes display (sRGB) colours.
                    materials.insert(
                        name.clone(),
                        [
                            srgb_to_linear(v[0]),
                            srgb_to_linear(v[1]),
                            srgb_to_linear(v[2]),
                        ],
                    );
                }
            }
            "endmtl" => {}
            "usemtl" => {
                current_mtl = it.next().map(str::to_string);
            }
            "f" => {
                let idx: Vec<usize> = it
                    .filter_map(|t| t.split('/').next())
                    .filter_map(|t| t.parse::<isize>().ok())
                    .filter_map(|i| {
                        if i > 0 {
                            Some(i as usize - 1)
                        } else if i < 0 {
                            verts.len().checked_sub((-i) as usize)
                        } else {
                            None
                        }
                    })
                    .collect();
                if idx.len() < 3 {
                    continue;
                }
                if idx.iter().any(|&i| i >= verts.len()) {
                    return Err(format!("line {}: the face index is out of range", ln + 1));
                }
                let key = current_mtl.clone().unwrap_or_default();
                let pi = *part_for.entry(key.clone()).or_insert_with(|| {
                    parts.push(MeshPart {
                        color: materials.get(&key).copied().unwrap_or(default),
                        ..Default::default()
                    });
                    parts.len() - 1
                });
                for k in 1..idx.len() - 1 {
                    parts[pi].push_triangle(verts[idx[0]], verts[idx[k]], verts[idx[k + 1]]);
                }
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        return Err("the OBJ has no faces".into());
    }
    Ok(Mesh { parts }.compact())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_materials() {
        let src = "newmtl 1\nKd 1 0 0\nendmtl\nnewmtl 2\nKd 0 0 1\nendmtl\n\
                   v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n\
                   usemtl 1\nf 1// 2// 3//\nusemtl 2\nf 1 3 4\n";
        let m = parse(src).unwrap();
        assert_eq!(m.parts.len(), 2);
        assert_eq!(m.triangles(), 2);
        assert_eq!(m.parts[0].color, [1.0, 0.0, 0.0]);
        assert_eq!(m.parts[0].normals[0], [0.0, 0.0, 1.0]);
    }

    #[test]
    fn quads_and_negative_indices() {
        let src = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf -4 -3 -2 -1\n";
        let m = parse(src).unwrap();
        assert_eq!(m.triangles(), 2);
    }
}
