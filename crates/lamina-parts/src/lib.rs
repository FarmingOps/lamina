//! Search for parts and import them from LCSC / JLCPCB / EasyEDA.

pub mod board;
pub mod easyeda;
pub mod provider;
pub mod source;

pub use easyeda::{Imported, ModelInfo, PartMeta, convert, model_height, model_ref};
pub use source::{DirSource, HttpSource, PartHit, PartSource};

#[cfg(test)]
mod tests {
    use super::*;
    use lamina_model::*;

    fn fixtures() -> DirSource {
        DirSource {
            dir: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        }
    }

    fn import(lcsc: &str) -> Imported {
        let src = fixtures();
        convert(&src.component(lcsc).unwrap(), lcsc, "2026-11-05").unwrap()
    }

    #[test]
    fn sot223_regulator() {
        let im = import("C6186");
        let fp = &im.footprint;
        assert_eq!(fp.id, "SOT-223-3_L6.5-W3.4-P2.30-LS7.0-BR");
        assert_eq!(fp.pads.len(), 4);
        // Pins 1..3 are on the right at x = +2.97. EasyEDA draws pin 1 at the bottom.
        let p1 = fp.pad("1").unwrap();
        let p3 = fp.pad("3").unwrap();
        let tab = fp.pad("4").unwrap();
        assert!((p1.at.x - 2.97).abs() < 1e-3, "{:?}", p1.at);
        assert!(p1.at.y < -2.0 && p3.at.y > 2.0);
        assert!(tab.at.x < -2.9);
        let (w, h) = tab.shape.size();
        assert!((w - 2.34).abs() < 0.01 && (h - 3.6).abs() < 0.01, "{w} {h}");
        assert_eq!(im.meta.basic, Some(true));
        assert_eq!(im.meta.mpn.as_deref(), Some("AMS1117-3.3"));
        let m = im.model.unwrap();
        let c = m.outline_center.unwrap();
        assert!(c.x.abs() < 0.2 && c.y.abs() < 0.2, "{c:?}");
    }

    #[test]
    fn sot223_model_placement() {
        let im = import("C6186");
        let info = im.model.unwrap();
        let obj = fixtures().model_obj(&info.uuid).unwrap();
        let r = model_ref(&info, &obj, "x.obj".into());
        assert_eq!(r.rotation, [0.0, 0.0, 180.0]);
        assert!(
            r.offset[0].abs() < 0.2 && r.offset[1].abs() < 0.2,
            "{:?}",
            r.offset
        );
        let h = model_height(&info, &obj).unwrap();
        assert!((h - 1.7).abs() < 0.05, "{h}");
    }

    #[test]
    fn usb_c_slots_and_holes() {
        let im = import("C165948");
        let fp = &im.footprint;
        let slots = fp
            .pads
            .iter()
            .filter(|p| {
                matches!(
                    p.kind,
                    PadKind::Through {
                        drill: Drill::Slot { .. },
                        ..
                    }
                )
            })
            .count();
        assert_eq!(slots, 4);
        let npth = fp
            .pads
            .iter()
            .filter(|p| matches!(p.kind, PadKind::Through { plated: false, .. }))
            .count();
        assert_eq!(npth, 2);
        assert!(fp.pad("A5").is_some() && fp.pad("B8").is_some());
        let slot = fp.pad("1").unwrap();
        if let PadKind::Through {
            drill: Drill::Slot { width, height },
            ..
        } = slot.kind
        {
            assert!(
                (width - 0.8).abs() < 0.01 && (height - 1.5).abs() < 0.01,
                "{width} {height}"
            );
        }
    }

    #[test]
    fn header_and_arcs() {
        let im = import("C2937625");
        assert_eq!(im.footprint.pads.len(), 3);
        let p1 = im.footprint.pad("1").unwrap();
        assert!(matches!(
            p1.kind,
            PadKind::Through {
                drill: Drill::Round { .. },
                ..
            }
        ));
        let im = import("C14663");
        let arcs = im
            .footprint
            .graphics
            .iter()
            .filter(|g| matches!(g.shape, Shape::Arc { .. }))
            .count();
        assert_eq!(arcs, 4);
        for g in &im.footprint.graphics {
            if let Shape::Arc { angle, .. } = g.shape {
                assert!(angle.abs() > 80.0 && angle.abs() < 100.0, "{angle}");
            }
        }
    }

    #[test]
    fn module_and_tssop() {
        let im = import("C2913198");
        assert!(im.footprint.pads.len() >= 41, "{}", im.footprint.pads.len());
        assert!(im.notes.iter().any(|n| n.contains("VIA")));
        let im = import("C2678753");
        assert_eq!(im.footprint.pads.len(), 28);
        let (_, w) = im.footprint.pad("1").unwrap().shape.size();
        assert!(w < 0.5);
    }

    #[test]
    fn search_parse() {
        let hits = fixtures().search("PCA9685", 5).unwrap();
        assert!(
            hits.iter()
                .any(|h| h.lcsc == "C2678753" && h.package.as_deref() == Some("TSSOP-28"))
        );
        assert!(hits.iter().all(|h| !h.basic));
    }

    #[test]
    fn all_fixtures_convert() {
        for c in [
            "C6186", "C165948", "C2937625", "C14663", "C2913198", "C2678753", "C84256", "C318884",
            "C474881",
        ] {
            let im = import(c);
            assert!(valid_id(&im.footprint.id), "{c}: {}", im.footprint.id);
            for p in &im.footprint.pads {
                p.shape
                    .check()
                    .unwrap_or_else(|e| panic!("{c} pad {}: {e}", p.number));
            }
            assert!(im.model.is_some(), "{c} has no 3D model");
        }
    }
}
