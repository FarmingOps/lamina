use lamina_core::model::*;
use lamina_core::*;

fn new_project() -> (tempfile::TempDir, Project) {
    let dir = tempfile::tempdir().unwrap();
    let p = Project::create(&dir.path().join("b"), "demo").unwrap();
    (dir, p)
}

fn add_resistor(p: &mut Project, refdes: &str) {
    let mut fp = Footprint::new("R_0603", Provenance::Manual { note: "t".into() });
    for (n, x) in [("1", -0.8), ("2", 0.8)] {
        fp.pads.push(Pad::smd(
            n,
            Point::new(x, 0.0),
            PadShape::Rect {
                width: 0.9,
                height: 0.9,
            },
        ));
    }
    let mut d = p.data.clone();
    d.footprints.insert(fp.id.clone(), fp);
    d.parts.parts.insert(refdes.into(), Part::new("R_0603"));
    p.commit(
        d,
        Touched::file(FileKind::Parts).footprint("R_0603"),
        false,
        "add",
    )
    .unwrap();
}

#[test]
fn outline_and_status() {
    let (_t, mut p) = new_project();
    let r = p
        .set_outline(SetOutline {
            rect: Some(RectSpec {
                x: 0.0,
                y: 0.0,
                width: 50.0,
                height: 30.0,
                corner_radius: 1.0,
            }),
            contour: None,
            cutouts: None,
            strict: false,
        })
        .unwrap();
    assert!(r.fixed.iter().any(|f| f.starts_with("no_outline")), "{r:?}");
    let s = p.status();
    let o = s.outline.unwrap();
    assert!((o.width() - 50.0).abs() < 1e-9);
    // Open the project from disk again.
    let root = p.root().to_path_buf();
    drop(p);
    let p2 = Project::open(&root).unwrap();
    assert!(p2.data.board.outline.is_some());
}

#[test]
fn malformed_input_refused() {
    let (_t, mut p) = new_project();
    let e = p.set_outline(SetOutline {
        rect: Some(RectSpec {
            x: 0.0,
            y: 0.0,
            width: -1.0,
            height: 3.0,
            corner_radius: 0.0,
        }),
        contour: None,
        cutouts: None,
        strict: false,
    });
    assert!(e.is_err());
    add_resistor(&mut p, "R1");
    let e = p.connect(Connect {
        net: "GND".into(),
        pads: vec!["R1.7".parse().unwrap()],
        class: None,
        strict: false,
    });
    assert!(e.unwrap_err().to_string().contains("no pad `7`"));
}

#[test]
fn apply_and_report_vs_strict() {
    let (_t, mut p) = new_project();
    add_resistor(&mut p, "R1");
    p.connect(Connect {
        net: "A".into(),
        pads: vec!["R1.1".parse().unwrap()],
        class: None,
        strict: false,
    })
    .unwrap();
    // Put the same pad in a second net. The change is reported and applied.
    let r = p
        .connect(Connect {
            net: "B".into(),
            pads: vec!["R1.1".parse().unwrap()],
            class: None,
            strict: false,
        })
        .unwrap();
    assert_eq!(r.new_violations[0].rule, "pad_in_two_nets");
    assert!(p.data.nets.nets.contains_key("B"));
    // With strict, the change is refused and the project does not change.
    let before = p.data.clone();
    let e = p.connect(Connect {
        net: "C".into(),
        pads: vec!["R1.1".parse().unwrap()],
        class: None,
        strict: true,
    });
    assert!(matches!(e, Err(Error::Refused(_))));
    assert_eq!(before, p.data);
    let on_disk = ProjectData::load(p.root()).unwrap();
    assert!(!on_disk.nets.nets.contains_key("C"));
}

#[test]
fn unresolved_pads_are_warnings() {
    let (_t, mut p) = new_project();
    let r = p
        .connect(Connect {
            net: "VCC".into(),
            pads: vec!["U9.1".parse().unwrap()],
            class: None,
            strict: true,
        })
        .unwrap();
    assert!(
        r.new_violations
            .iter()
            .all(|v| v.severity == Severity::Warning)
    );
}

#[test]
fn stackup_refuses_stranding_copper() {
    let (_t, mut p) = new_project();
    p.set_stackup(SetStackup {
        copper_layers: Some(4),
        ..Default::default()
    })
    .unwrap();
    let mut d = p.data.clone();
    d.nets.nets.insert("N".into(), Net::default());
    d.routing.tracks.push(Track {
        id: "t1".into(),
        net: "N".into(),
        layer: "in2".parse().unwrap(),
        width: 0.2,
        points: vec![Point::new(0.0, 0.0), Point::new(1.0, 0.0)],
    });
    p.commit(
        d,
        Touched::file(FileKind::Routing).and(FileKind::Nets),
        true,
        "t",
    )
    .unwrap();
    assert!(
        p.set_stackup(SetStackup {
            copper_layers: Some(2),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        p.set_stackup(SetStackup {
            copper_layers: Some(3),
            ..Default::default()
        })
        .is_err()
    );
}

#[test]
fn lock_blocks_second_open_only_for_live_other_process() {
    let (_t, p) = new_project();
    // The same process can open the project again (the pid is the same).
    let again = Project::open(p.root());
    assert!(again.is_ok());
}

#[test]
fn ids() {
    assert_eq!(next_id("t", ["t1", "t9", "x3", "t10"]), "t11");
    assert_eq!(next_id("v", []), "v1");
}

#[test]
fn import_from_fixtures() {
    let (_t, mut p) = new_project();
    let fx =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../lamina-parts/tests/fixtures");
    p.set_source(std::sync::Arc::new(lamina_parts::DirSource { dir: fx }));
    let r = p
        .import_lcsc(ImportLcsc {
            lcsc: "C6186".into(),
            refdes: Some("U3".into()),
            value: None,
            footprint_id: None,
            refresh: false,
            step: false,
            replace: false,
            strict: true,
        })
        .unwrap();
    assert!(r.summary.contains("basic"), "{}", r.summary);
    assert!(
        r.notes.iter().any(|n| n.contains("3D model")),
        "{:?}",
        r.notes
    );
    let part = &p.data.parts.parts["U3"];
    assert_eq!(part.lcsc.as_deref(), Some("C6186"));
    let fp = &p.data.footprints[&part.footprint];
    let m = fp.model.as_ref().unwrap();
    assert!(p.root().join("cache/models").join(&m.file).is_file());
    assert!(p.root().join("cache/parts/C6186.json").is_file());
    // The second import uses the cached footprint again and adds one more part.
    let r = p
        .import_lcsc(ImportLcsc {
            lcsc: "C6186".into(),
            refdes: Some("U4".into()),
            value: Some("3V3 LDO".into()),
            footprint_id: None,
            refresh: false,
            step: false,
            replace: false,
            strict: false,
        })
        .unwrap();
    assert!(r.notes.iter().any(|n| n.contains("kept as is")));
    assert_eq!(p.data.parts.parts["U4"].value.as_deref(), Some("3V3 LDO"));
}

#[test]
fn date_format() {
    let d = today();
    assert_eq!(d.len(), 10);
    assert!(d.starts_with("20"));
}

#[test]
fn shared_editor_and_engine_see_each_other() {
    let (t, mut engine) = new_project();
    add_resistor(&mut engine, "R1");
    engine
        .place(Place {
            refdes: "R1".into(),
            x: 5.0,
            y: 5.0,
            rotation: None,
            side: None,
            locked: None,
            strict: false,
        })
        .unwrap();
    // The viewer opens the same folder when the engine has the lock.
    let root = t.path().join("b");
    let mut viewer = Project::open_shared(&root).unwrap();
    let mut d = viewer.data.clone();
    d.placement.placements.get_mut("R1").unwrap().x = 9.0;
    let r = viewer.apply(d, "drag R1").unwrap();
    assert_eq!(r.changed, vec!["placement.json".to_string()]);

    // Before its next change, the engine finds the edit of the viewer and keeps it.
    assert!(engine.sync().unwrap());
    assert!(!engine.sync().unwrap());
    engine
        .rotate_part(RotatePart {
            refdes: "R1".into(),
            rotation: Some(90.0),
            by: None,
            force: false,
            strict: false,
        })
        .unwrap();
    let pl = ProjectData::load(&root).unwrap().placement.placements["R1"];
    assert_eq!((pl.x, pl.rotation), (9.0, 90.0));
    assert!(viewer.sync().unwrap());
    assert_eq!(viewer.data.placement.placements["R1"].rotation, 90.0);
}

#[test]
fn kicad_footprint_import_and_board_round_trip() {
    let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../lamina-kicad/tests/fixtures/footprints");
    let (t, mut p) = new_project();
    p.set_kicad_source(std::sync::Arc::new(lamina_kicad::DirSource {
        footprints: fixtures.clone(),
        models: t.path().join("no-models"),
    }));
    // Downloaded "online" (from the fixture source), with a part.
    let r = p
        .import_kicad_footprint(ImportKicadFootprint {
            footprint: "Package_TO_SOT_SMD:SOT-23".into(),
            refdes: Some("Q1".into()),
            value: Some("BSS138".into()),
            footprint_id: None,
            model: true,
            online: true,
            replace: false,
            strict: false,
        })
        .unwrap();
    assert!(
        r.summary.contains("Q1: Package_TO_SOT_SMD:SOT-23"),
        "{}",
        r.summary
    );
    assert!(
        r.notes
            .iter()
            .any(|n| n.contains("not downloaded") || n.contains("not found")),
        "{:?}",
        r.notes
    );
    assert_eq!(p.data.footprints["SOT-23"].pads.len(), 3);
    assert_eq!(p.data.parts.parts["Q1"].value.as_deref(), Some("BSS138"));
    // If it is not found locally and online is false, there is a clear error.
    let e = p
        .import_kicad_footprint(ImportKicadFootprint {
            footprint: "Nope:Missing".into(),
            refdes: None,
            value: None,
            footprint_id: None,
            model: false,
            online: false,
            replace: false,
            strict: false,
        })
        .unwrap_err()
        .to_string();
    assert!(e.contains("not found"), "{e}");
    // A .kicad_mod path also works.
    let path = fixtures.join("Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod");
    p.import_kicad_footprint(ImportKicadFootprint {
        footprint: path.to_string_lossy().to_string(),
        refdes: Some("R1".into()),
        value: None,
        footprint_id: Some("R0603K".into()),
        model: false,
        online: false,
        replace: false,
        strict: false,
    })
    .unwrap();
    assert!(p.data.footprints.contains_key("R0603K"));
    assert_eq!(p.data.parts.parts["R1"].footprint, "R0603K");

    p.set_outline(SetOutline {
        rect: Some(RectSpec {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 15.0,
            corner_radius: 0.0,
        }),
        contour: None,
        cutouts: None,
        strict: false,
    })
    .unwrap();
    p.set_nets(SetNets {
        nets: [(
            "GND".to_string(),
            Net {
                pads: vec!["Q1.2".parse().unwrap(), "R1.1".parse().unwrap()],
                class: None,
            },
        )]
        .into(),
        replace: true,
        strict: false,
    })
    .unwrap();
    p.place(Place {
        refdes: "Q1".into(),
        x: 5.0,
        y: 5.0,
        rotation: None,
        side: None,
        locked: None,
        strict: false,
    })
    .unwrap();
    p.place(Place {
        refdes: "R1".into(),
        x: 12.0,
        y: 5.0,
        rotation: Some(90.0),
        side: Some(Side::Bottom),
        locked: None,
        strict: false,
    })
    .unwrap();
    let out = p.export_kicad(ExportKicad { path: None }).unwrap();
    assert!(out.path.ends_with("demo.kicad_pcb"), "{}", out.path);
    assert_eq!(out.parts, 2);

    // Import into a new project.
    let dir2 = t.path().join("imported");
    let mut q = Project::create(&dir2, "x").unwrap();
    let r = q
        .import_kicad_board(ImportKicadBoard {
            path: out.path.clone(),
            models: false,
            online: false,
            replace: false,
            strict: false,
        })
        .unwrap();
    assert!(r.summary.contains("2 parts"), "{}", r.summary);
    assert_eq!(q.data.board.name, "demo");
    assert_eq!(q.data.placement.placements["R1"].side, Side::Bottom);
    assert_eq!(q.data.placement.placements["R1"].rotation, 90.0);
    assert_eq!(q.data.nets.nets["GND"].pads.len(), 2);
    assert!(q.data.board.outline.is_some());
    assert!(
        q.import_kicad_board(ImportKicadBoard {
            path: out.path,
            models: false,
            online: false,
            replace: false,
            strict: false
        })
        .is_err(),
        "a second import needs replace"
    );
}

/// Two resistors in series as one block: IN - R.1, R.2 - S.1 (MID), S.2 - OUT.
fn add_pair(p: &mut Project, r: &str, s: &str, group: &str, n: &str) {
    add_resistor(p, r);
    add_resistor(p, s);
    let mut d = p.data.clone();
    for x in [r, s] {
        d.parts.parts.get_mut(x).unwrap().group = Some(group.into());
    }
    for (net, pads) in [
        (format!("IN{n}"), vec![format!("{r}.1")]),
        (format!("MID{n}"), vec![format!("{r}.2"), format!("{s}.1")]),
        (format!("OUT{n}"), vec![format!("{s}.2")]),
    ] {
        d.nets.nets.insert(
            net,
            Net {
                pads: pads.iter().map(|p| p.parse().unwrap()).collect(),
                class: None,
            },
        );
    }
    p.commit(
        d,
        Touched::file(FileKind::Parts).and(FileKind::Nets),
        false,
        "pair",
    )
    .unwrap();
}

#[test]
fn layout_blocks_repeat_save_and_place() {
    let t = tempfile::tempdir().unwrap();
    Workspace::create(t.path(), "ws").unwrap();
    let mut a = Project::create(&t.path().join("boards/a"), "a").unwrap();
    add_pair(&mut a, "R1", "R2", "ch1", "1");
    add_pair(&mut a, "R3", "R4", "ch2", "2");
    let pl = |x: f64, y: f64, rotation: f64| Placement {
        rotation,
        ..Placement::at(x, y)
    };
    a.place_many(PlaceMany {
        placements: [
            ("R1".to_string(), pl(10.0, 10.0, 0.0)),
            ("R2".to_string(), pl(13.0, 10.0, 0.0)),
            ("R3".to_string(), pl(10.0, 20.0, 90.0)),
        ]
        .into(),
        strict: false,
    })
    .unwrap();
    a.route_trace(
        serde_json::from_value(serde_json::json!({"layer": "top", "points": ["R1.2", "R2.1"]}))
            .unwrap(),
    )
    .unwrap();
    assert!(a.status().groups.contains_key("ch2"));

    // ch2 gets the layout of ch1 around R3, which is turned a quarter.
    let r = a
        .repeat_layout(
            serde_json::from_value(serde_json::json!({"from": "ch1", "to": ["ch2"]})).unwrap(),
        )
        .unwrap();
    assert!(
        r.summary.contains("ch2: 2 parts around R3"),
        "{}",
        r.summary
    );
    let r4 = a.data.placement.placements["R4"];
    assert!(
        (r4.x - 10.0).abs() < 1e-9 && (r4.y - 23.0).abs() < 1e-9,
        "{r4:?}"
    );
    assert_eq!(r4.rotation, 90.0);
    let mid2: Vec<&Track> = a
        .data
        .routing
        .tracks
        .iter()
        .filter(|t| t.net == "MID2")
        .collect();
    assert_eq!(mid2.len(), 1);
    assert_eq!(
        mid2[0].points,
        vec![Point::new(10.0, 20.8), Point::new(10.0, 22.2)]
    );
    assert!(
        !a.check()
            .iter()
            .any(|v| v.rule == "unrouted" && v.message.contains("MID2"))
    );

    // Save the layout in the workspace library, then place it on a different board.
    let saved = a
        .save_layout_block(
            serde_json::from_value(serde_json::json!({"from": "ch1", "name": "pair"})).unwrap(),
        )
        .unwrap();
    assert!(
        t.path().join("lib/layouts/pair.json").is_file(),
        "{}",
        saved.path
    );
    assert_eq!((saved.parts, saved.tracks), (2, 1));
    let e = a
        .save_layout_block(
            serde_json::from_value(serde_json::json!({"from": "ch1", "name": "pair"})).unwrap(),
        )
        .unwrap_err();
    assert!(e.to_string().contains("replace"), "{e}");

    let mut b = Project::create(&t.path().join("boards/b"), "b").unwrap();
    add_pair(&mut b, "Q7", "Q8", "x", "X");
    b.place(Place {
        refdes: "Q8".into(),
        x: 40.0,
        y: 5.0,
        rotation: Some(180.0),
        side: None,
        locked: None,
        strict: false,
    })
    .unwrap();
    let r = b
        .place_layout_block(
            serde_json::from_value(
                serde_json::json!({"name": "pair", "to": [["Q7", "Q8"]], "anchor": "Q8"}),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(r.summary.contains("around Q8"), "{}", r.summary);
    let q7 = b.data.placement.placements["Q7"];
    assert!(
        (q7.x - 43.0).abs() < 1e-9 && (q7.y - 5.0).abs() < 1e-9,
        "{q7:?}"
    );
    let mid: Vec<&Track> = b
        .data
        .routing
        .tracks
        .iter()
        .filter(|t| t.net == "MIDX")
        .collect();
    assert_eq!(mid.len(), 1);
    let e = b
        .place_layout_block(
            serde_json::from_value(serde_json::json!({"name": "nope", "to": ["x"]})).unwrap(),
        )
        .unwrap_err();
    assert!(e.to_string().contains("saved layouts are: pair"), "{e}");
}

#[test]
fn kicad_board_models_fall_back_and_resolve_project_paths() {
    let t = tempfile::tempdir().unwrap();
    let kicad = t.path().join("kicad");
    std::fs::create_dir_all(kicad.join("models")).unwrap();
    let corpus = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../lamina-step/corpus/C25804.step");
    std::fs::copy(&corpus, kicad.join("models/body.step")).unwrap();
    // First a library model that is not there, then a hidden model, then a
    // model in the project.
    let pcb = r#"(kicad_pcb (version 20240108) (generator "test")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
  (gr_rect (start 0 0) (end 20 20) (layer "Edge.Cuts") (width 0.1))
  (footprint "Lib:Part" (layer "F.Cu") (at 10 10)
    (property "Reference" "J1" (at 0 0) (layer "F.SilkS") (effects (font (size 1 1))))
    (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu" "F.Paste" "F.Mask"))
    (model "${KICAD10_3DMODEL_DIR}/Nope.3dshapes/Missing.step")
    (model "${KIPRJMOD}/models/hidden.step" (hide yes))
    (model "${KIPRJMOD}/models/body.step" (offset (xyz 0 0 1))))
  (footprint "Lib:Part2" (layer "F.Cu") (at 15 10)
    (property "Reference" "J2" (at 0 0) (layer "F.SilkS") (effects (font (size 1 1))))
    (pad "1" smd rect (at 0 0) (size 1.2 1) (layers "F.Cu" "F.Paste" "F.Mask"))
    (model "models/body.step"))
  (footprint "Lib:Hole" (layer "F.Cu") (at 3 3)
    (property "Reference" "H1" (at 0 0) (layer "F.SilkS") (effects (font (size 1 1))))
    (pad "" np_thru_hole circle (at 0 0) (size 3.2 3.2) (drill 3.2) (layers "*.Cu" "*.Mask"))))
"#;
    let file = kicad.join("b.kicad_pcb");
    std::fs::write(&file, pcb).unwrap();
    let mut p = Project::create(&t.path().join("board"), "x").unwrap();
    let r = p
        .import_kicad_board(ImportKicadBoard {
            path: file.to_string_lossy().into_owned(),
            models: true,
            online: false,
            replace: false,
            strict: false,
        })
        .unwrap();
    assert!(
        r.notes.iter().any(|n| n == "3D models: 2 of 2 resolved"),
        "{:?}",
        r.notes
    );
    let fp = &p.data.footprints[&p.data.parts.parts["J1"].footprint];
    let m = fp.model.as_ref().expect("project model used");
    assert_eq!(m.file, "kicad/models/body.step");
    assert_eq!(m.offset, [0.0, 0.0, 1.0]);
    // A relative path that has no variable is also read from the KiCad project.
    let fp2 = &p.data.footprints[&p.data.parts.parts["J2"].footprint];
    assert!(fp2.model.is_some());
    // The mounting hole has no body and is not drawn as a box.
    let hole = &p.data.footprints[&p.data.parts.parts["H1"].footprint];
    assert!(!hole.has_body());
    assert!(fp.has_body());
}

#[test]
fn frame_3d_fits_a_part_a_pad_or_a_region() {
    let (_t, mut p) = new_project();
    add_resistor(&mut p, "R1");
    p.place(Place {
        refdes: "R1".into(),
        x: 10.0,
        y: 5.0,
        rotation: Some(90.0),
        side: None,
        locked: None,
        strict: false,
    })
    .unwrap();
    // The part: its pads turned by 90 degrees, plus a 1.5 mm margin.
    let (lo, hi) = p
        .frame_3d(None, Some(&Target::Name("R1".into())))
        .unwrap()
        .unwrap();
    assert!((lo[0] - 8.05).abs() < 1e-3 && (hi[0] - 11.95).abs() < 1e-3);
    assert!((lo[1] - 2.25).abs() < 1e-3 && (hi[1] - 7.75).abs() < 1e-3);
    assert_eq!(lo[2], 0.0);
    assert!((hi[2] - 3.6).abs() < 1e-3);
    // A pad: a 5 mm box around its centre.
    let (lo, hi) = p
        .frame_3d(None, Some(&Target::Name("R1.1".into())))
        .unwrap()
        .unwrap();
    assert!((lo[0] - 7.5).abs() < 1e-3 && (hi[0] - 12.5).abs() < 1e-3);
    assert!((lo[1] - 1.7).abs() < 1e-3 && (hi[1] - 6.7).abs() < 1e-3);
    // A region wins over the focus.
    let r = Rect {
        min: Point::new(1.0, 2.0),
        max: Point::new(3.0, 4.0),
    };
    let (lo, hi) = p
        .frame_3d(Some(&r), Some(&Target::Name("R1".into())))
        .unwrap()
        .unwrap();
    assert_eq!((lo[0], lo[1], hi[0], hi[1]), (1.0, 2.0, 3.0, 4.0));
    // No focus and no region: fit the whole board.
    assert!(p.frame_3d(None, None).unwrap().is_none());
    // Unknown names are refused.
    assert!(p.frame_3d(None, Some(&Target::Name("R9".into()))).is_err());
    assert!(
        p.frame_3d(None, Some(&Target::Name("R1.7".into())))
            .is_err()
    );
}
