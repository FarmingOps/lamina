//! Fabrication outputs: Gerber X2, Excellon, JLCPCB BOM and CPL, zip.

pub mod fab_import;
pub mod fabricator;
pub mod gerber;
pub mod gerber_format;
pub mod gerber_in;
pub mod reader;
pub mod step_out;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use lamina_geom::prim::Prim;
use lamina_model::*;
use lamina_scene::{Scene2D, SceneLayer};

pub use fabricator::{GerberFabricator, StepExporter};
pub use gerber_format::GerberFormat;
pub use lamina_ports::fab::Output;

fn copper_ext(i: u8, n: u8) -> (String, String) {
    let c = Copper::from_index(i, n);
    match c {
        Copper::Top => ("GTL".into(), "Copper,L1,Top".to_string()),
        Copper::Bottom => ("GBL".into(), format!("Copper,L{n},Bot")),
        Copper::Inner(k) => (format!("G{k}"), format!("Copper,L{},Inr", k + 1)),
    }
}

/// Make the Gerber files for all layers of the scene.
pub fn gerbers(s: &Scene2D, name: &str) -> Vec<Output> {
    let n = s.copper_layers;
    let mut jobs: Vec<(SceneLayer, String, String, &'static str)> = Vec::new();
    for i in 0..n {
        let (ext, f) = copper_ext(i, n);
        jobs.push((SceneLayer::Copper(i), ext, f, "Positive"));
    }
    for (side, t) in [(Side::Top, "Top"), (Side::Bottom, "Bot")] {
        let x = if side == Side::Top { 'T' } else { 'B' };
        jobs.push((
            SceneLayer::Mask(side),
            format!("G{x}S"),
            format!("Soldermask,{t}"),
            "Negative",
        ));
        jobs.push((
            SceneLayer::Paste(side),
            format!("G{x}P"),
            format!("Paste,{t}"),
            "Positive",
        ));
        jobs.push((
            SceneLayer::Silk(side),
            format!("G{x}O"),
            format!("Legend,{t}"),
            "Positive",
        ));
    }
    jobs.push((
        SceneLayer::Outline,
        "GKO".into(),
        "Profile,NP".into(),
        "Positive",
    ));
    let mut out = Vec::new();
    for (layer, ext, function, polarity) in jobs {
        let mut w = gerber::Writer::new();
        if let Some(d) = s.layer(layer) {
            // Draw the pours with a lower priority first. Then their clear holes
            // cannot erase a pour with a higher priority.
            for g in d.items[..d.pour].iter().rev() {
                w.add(g);
            }
            for g in &d.items[d.pour..] {
                w.add(g);
            }
        }
        out.push(Output {
            name: format!("{name}.{ext}"),
            contents: w
                .finish(&gerber::Function { function, polarity })
                .into_bytes(),
        });
    }
    out
}

/// Make the Excellon drill files: plated and non-plated.
pub fn drills(g: &lamina_geom::BoardGeometry, name: &str) -> Vec<Output> {
    let mut out = Vec::new();
    for plated in [true, false] {
        let mut tools: BTreeMap<i64, Vec<&Prim>> = BTreeMap::new();
        for d in g.drills.iter().filter(|d| d.plated == plated) {
            let dia = match &d.prim {
                Prim::Circle { r, .. } | Prim::Capsule { r, .. } => 2.0 * r,
                Prim::Poly { .. } => continue,
            };
            tools
                .entry((dia * 1000.0).round() as i64)
                .or_default()
                .push(&d.prim);
        }
        let mut s = String::new();
        s.push_str("M48\n");
        let _ = writeln!(
            s,
            "; #@! TF.FileFunction,{},1,{},{}",
            if plated { "Plated" } else { "NonPlated" },
            g.layers.max(2),
            if plated { "PTH" } else { "NPTH" }
        );
        s.push_str("FMAT,2\nMETRIC\n");
        for (k, (d, _)) in tools.iter().enumerate() {
            let _ = writeln!(s, "T{}C{:.3}", k + 1, *d as f64 / 1000.0);
        }
        s.push_str("%\nG90\nG05\n");
        for (k, (_, holes)) in tools.iter().enumerate() {
            let _ = writeln!(s, "T{}", k + 1);
            for h in holes {
                match h {
                    Prim::Circle { c, .. } => {
                        let _ = writeln!(s, "X{:.4}Y{:.4}", c.x, c.y);
                    }
                    Prim::Capsule { a, b, .. } => {
                        let _ = writeln!(s, "X{:.4}Y{:.4}G85X{:.4}Y{:.4}", a.x, a.y, b.x, b.y);
                    }
                    Prim::Poly { .. } => {}
                }
            }
        }
        s.push_str("T0\nM30\n");
        out.push(Output {
            name: format!("{name}-{}.DRL", if plated { "PTH" } else { "NPTH" }),
            contents: s.into_bytes(),
        });
    }
    out
}

fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Make the JLCPCB assembly BOM and the pick-and-place files.
pub fn assembly(d: &ProjectData, name: &str) -> (Vec<Output>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut groups: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
    for (r, p) in &d.parts.parts {
        if p.dnp {
            continue;
        }
        if p.lcsc.is_none() {
            warnings.push(format!(
                "{r} has no LCSC part number, so JLCPCB will not place it"
            ));
        }
        let key = (
            p.value.clone().or(p.mpn.clone()).unwrap_or_default(),
            p.footprint.clone(),
            p.lcsc.clone().unwrap_or_default(),
        );
        groups.entry(key).or_default().push(r.clone());
    }
    let mut bom = String::from("Comment,Designator,Footprint,LCSC Part #\n");
    for ((value, fp, lcsc), mut refs) in groups {
        refs.sort_by(|a, b| natural(a, b));
        let _ = writeln!(
            bom,
            "{},{},{},{}",
            csv(&value),
            csv(&refs.join(",")),
            csv(&fp),
            csv(&lcsc)
        );
    }
    let mut cpl = String::from("Designator,Mid X,Mid Y,Layer,Rotation\n");
    let mut placed: Vec<(&String, &Placement)> = d.placement.placements.iter().collect();
    placed.sort_by(|a, b| natural(a.0, b.0));
    for (r, pl) in placed {
        if d.parts.parts.get(r).is_none_or(|p| p.dnp) {
            continue;
        }
        let _ = writeln!(
            cpl,
            "{},{:.4}mm,{:.4}mm,{},{}",
            r,
            pl.x,
            pl.y,
            if pl.side == Side::Top {
                "Top"
            } else {
                "Bottom"
            },
            json::round6(pl.rotation)
        );
    }
    for r in d.parts.parts.keys() {
        if !d.placement.placements.contains_key(r) && !d.parts.parts[r].dnp {
            warnings.push(format!("{r} is not placed, so it is not in the CPL"));
        }
    }
    (
        vec![
            Output {
                name: format!("{name}-BOM.csv"),
                contents: bom.into_bytes(),
            },
            Output {
                name: format!("{name}-CPL.csv"),
                contents: cpl.into_bytes(),
            },
        ],
        warnings,
    )
}

fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let split = |s: &str| {
        let p = s.trim_end_matches(|c: char| c.is_ascii_digit());
        (p.to_string(), s[p.len()..].parse::<u64>().unwrap_or(0))
    };
    split(a).cmp(&split(b)).then(a.cmp(b))
}

/// A zip of the fabrication files, named `<name>-gerbers.zip`.
pub fn archive(fab: &[Output], name: &str) -> Result<Output, String> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for o in fab {
        z.start_file(o.name.as_str(), opt)
            .map_err(|e| e.to_string())?;
        z.write_all(&o.contents).map_err(|e| e.to_string())?;
    }
    let c = z.finish().map_err(|e| e.to_string())?;
    Ok(Output {
        name: format!("{name}-gerbers.zip"),
        contents: c.into_inner(),
    })
}

/// Write the files to `dir`, and a zip of the fabrication files.
pub fn write(
    dir: &Path,
    name: &str,
    fab: &[Output],
    asm: &[Output],
) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut paths = Vec::new();
    let zip = archive(fab, name)?;
    for o in fab.iter().chain(asm).chain(std::iter::once(&zip)) {
        let p = dir.join(&o.name);
        std::fs::write(&p, &o.contents).map_err(|e| format!("{}: {e}", p.display()))?;
        paths.push(p);
    }
    Ok(paths)
}
