//! Specctra DSN export and SES import. An external autorouter, for example
//! Freerouting, uses these files. Coordinates are in micrometres with Y up,
//! the same as the Lamina frame.

use std::collections::{BTreeMap, BTreeSet};

use lamina_kicad::layers::{layer_from_kicad, layer_to_kicad};
use lamina_model::sexpr::{self, Node, Sx, n};
use lamina_model::*;

fn r6(v: f64) -> f64 {
    json::round6(v)
}

/// Convert millimetres to the micrometres of the DSN.
fn um(v: f64) -> f64 {
    (v * 1000.0 * 10.0).round() / 10.0
}

/// The padstack name for a pad. `[T]` is top only, `[A]` is all layers.
fn padstack_name(pad: &Pad, through: bool) -> String {
    let side = if through { "A" } else { "T" };
    let (w, h) = pad.shape.size();
    let dims = format!("{}x{}", um(w), um(h))
        .replace(".0x", "x")
        .replace(".0", "");
    match &pad.shape {
        PadShape::Circle { diameter } => format!("Round[{side}]Pad_{}_um", um(*diameter)),
        PadShape::Rect { .. } => format!("Rect[{side}]Pad_{dims}_um"),
        PadShape::Oval { .. } => format!("Oval[{side}]Pad_{dims}_um"),
        PadShape::RoundRect { radius, .. } => {
            format!("RoundRect[{side}]Pad_{dims}_{}_um", um(*radius))
        }
        PadShape::Polygon { points } => {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for p in points {
                for b in format!("{},{};", p.x, p.y).bytes() {
                    h ^= b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
            }
            format!("Poly[{side}]Pad_{dims}_{h:08x}_um")
        }
    }
}

fn via_name(diameter: f64, drill: f64) -> String {
    format!("Via[0-1]_{}:{}_um", um(diameter), um(drill))
}

/// Get the sizes from a `Via[0-1]_600:300_um` name.
pub fn via_sizes(name: &str) -> Option<(f64, f64)> {
    let body = name.split('_').nth(1)?;
    let (d, dr) = body.split_once(':')?;
    Some((
        d.parse::<f64>().ok()? / 1000.0,
        dr.parse::<f64>().ok()? / 1000.0,
    ))
}

/// The pad outline as a DSN shape, in pad-local coordinates (not rotated), in µm.
fn pad_shape_node(pad: &Pad, layer: &str) -> Node {
    match &pad.shape {
        PadShape::Circle { diameter } => n("circle").sym(layer).num(um(*diameter)),
        PadShape::Rect { width, height } => n("rect").sym(layer).nums(&[
            um(-width / 2.0),
            um(-height / 2.0),
            um(width / 2.0),
            um(height / 2.0),
        ]),
        PadShape::Oval { width, height } => {
            // A path between the two end centres. Its width is the smaller dimension.
            if width >= height {
                let dx = (width - height) / 2.0;
                n("path")
                    .sym(layer)
                    .num(um(*height))
                    .nums(&[um(-dx), 0.0, um(dx), 0.0])
            } else {
                let dy = (height - width) / 2.0;
                n("path")
                    .sym(layer)
                    .num(um(*width))
                    .nums(&[0.0, um(-dy), 0.0, um(dy)])
            }
        }
        PadShape::RoundRect {
            width,
            height,
            radius,
        } => {
            let mut node = n("polygon").sym(layer).num(0.0);
            let (hx, hy, r) = (
                width / 2.0,
                height / 2.0,
                radius.min(width.min(*height) / 2.0),
            );
            let corners = [
                (hx - r, hy - r, 0.0),
                (-hx + r, hy - r, 90.0),
                (-hx + r, -hy + r, 180.0),
                (hx - r, -hy + r, 270.0),
            ];
            for (cx, cy, a0) in corners {
                for k in 0..=4 {
                    let a = (a0 + 90.0 * k as f64 / 4.0).to_radians();
                    node = node.num(um(cx + r * a.cos())).num(um(cy + r * a.sin()));
                }
            }
            node
        }
        PadShape::Polygon { points } => {
            let mut node = n("polygon").sym(layer).num(0.0);
            for p in points {
                node = node.num(um(p.x)).num(um(p.y));
            }
            node
        }
    }
}

/// Export the board as a Specctra DSN. If `reroute_all` is false, the existing
/// tracks and vias go out as protected wiring.
pub fn export_dsn(d: &ProjectData, name: &str, reroute_all: bool) -> String {
    let copper = d.board.stackup.copper_layers;
    let layers = Layer::all_copper(copper);
    let lname = |l: Layer| layer_to_kicad(l);
    let rules = &d.board.rules;
    let ring = rules.via_diameter - rules.via_drill;

    // Structure.
    let mut structure = n("structure");
    for (i, l) in layers.iter().enumerate() {
        structure = structure.child(
            n("layer")
                .sym(lname(*l))
                .child(n("type").sym("signal"))
                .child(n("property").child(n("index").num(i as f64))),
        );
    }
    let mut boundary = n("path").sym("pcb").num(0.0);
    if let Some(o) = &d.board.outline {
        let pts = o.flatten(0.01);
        for p in pts.iter().chain(pts.first()) {
            boundary = boundary.num(um(p.x)).num(um(p.y));
        }
    }
    structure = structure.child(n("boundary").child(boundary));
    for c in &d.board.cutouts {
        let mut poly = n("polygon").sym("signal").num(0.0);
        for p in c.flatten(0.01) {
            poly = poly.num(um(p.x)).num(um(p.y));
        }
        structure = structure.child(n("keepout").str("").child(poly));
    }
    for k in &d.routing.keepouts {
        let kind = match (k.forbid.tracks, k.forbid.vias) {
            (true, true) => "keepout",
            (true, false) => "wire_keepout",
            (false, true) => "via_keepout",
            (false, false) => continue,
        };
        for l in &k.layers {
            let mut poly = n("polygon").sym(lname(*l)).num(0.0);
            for p in k.outline.flatten(0.01) {
                poly = poly.num(um(p.x)).num(um(p.y));
            }
            structure = structure.child(n(kind).str(&k.id).child(poly));
        }
    }
    // Via padstacks: the default size and each size that a net class sets.
    let mut vias: BTreeSet<(u64, u64)> = BTreeSet::new();
    let key = |d: f64, dr: f64| ((d * 1e6) as u64, (dr * 1e6) as u64);
    vias.insert(key(rules.via_diameter, rules.via_drill));
    for c in rules.net_classes.values() {
        vias.insert(key(
            c.via_diameter.unwrap_or(rules.via_diameter),
            c.via_drill.unwrap_or(rules.via_drill),
        ));
    }
    let via_list: Vec<(f64, f64)> = vias
        .iter()
        .map(|(a, b)| (*a as f64 / 1e6, *b as f64 / 1e6))
        .collect();
    let mut via_node = n("via");
    for (dia, dr) in &via_list {
        via_node = via_node.str(via_name(*dia, *dr));
    }
    // Routers know copper clearance, but not hole spacing, so ask for a
    // copper gap between vias (and vias and pins) that keeps holes apart.
    let via_gap = rules.clearance.max(rules.hole_to_hole - ring);
    structure = structure.child(via_node).child(
        n("rule")
            .child(n("width").num(um(rules.track_width)))
            .child(n("clearance").num(um(rules.clearance)))
            .child(
                n("clearance")
                    .num(um(rules.clearance))
                    .child(n("type").sym("default_smd")),
            )
            .child(
                n("clearance")
                    .num(um(rules.clearance / 4.0))
                    .child(n("type").sym("smd_smd")),
            )
            .child(
                n("clearance")
                    .num(um(via_gap))
                    .child(n("type").sym("via_via")),
            )
            .child(
                n("clearance")
                    .num(um(via_gap))
                    .child(n("type").sym("via_pin")),
            ),
    );

    // Placement and library.
    let mut by_fp: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (refdes, part) in &d.parts.parts {
        if d.placement.placements.contains_key(refdes) && d.footprints.contains_key(&part.footprint)
        {
            by_fp
                .entry(part.footprint.as_str())
                .or_default()
                .push(refdes);
        }
    }
    let mut placement = n("placement");
    let mut library = n("library");
    let mut padstacks: BTreeMap<String, Node> = BTreeMap::new();
    for (fp_id, refs) in &by_fp {
        let fp = &d.footprints[*fp_id];
        let mut comp = n("component").str(*fp_id);
        for r in refs {
            let pl = &d.placement.placements[*r];
            let part = &d.parts.parts[*r];
            let mut place = n("place")
                .str(*r)
                .num(um(pl.x))
                .num(um(pl.y))
                .sym(if pl.side == Side::Top {
                    "front"
                } else {
                    "back"
                })
                .num(pl.rotation);
            if let Some(v) = &part.value {
                place = place.child(n("PN").str(v));
            }
            comp = comp.child(place);
        }
        placement = placement.child(comp);

        let mut image = n("image").str(*fp_id);
        if let Some(cy) = fp.courtyard_bbox() {
            image = image.child(n("outline").child(n("rect").sym("signal").nums(&[
                um(cy.min.x),
                um(cy.min.y),
                um(cy.max.x),
                um(cy.max.y),
            ])));
        }
        for pad in &fp.pads {
            let through = matches!(pad.kind, PadKind::Through { .. });
            if matches!(pad.kind, PadKind::Through { plated: false, .. }) {
                // An unplated hole keeps routes away like a keepout. The keepout is
                // wide enough that a via next to it still meets hole-to-hole.
                let d_hole = match pad.kind {
                    PadKind::Through { drill, .. } => drill.min_size(),
                    _ => 0.0,
                };
                let margin = (rules.hole_to_hole - ring / 2.0).max(0.0);
                image = image.child(
                    n("keepout").str("").child(
                        n("circle")
                            .sym("signal")
                            .num(um(d_hole + 2.0 * margin))
                            .num(um(pad.at.x))
                            .num(um(pad.at.y)),
                    ),
                );
                continue;
            }
            if pad.number.is_empty() {
                continue;
            }
            let ps = padstack_name(pad, through);
            padstacks.entry(ps.clone()).or_insert_with(|| {
                let mut node = n("padstack").str(&ps);
                if through {
                    for l in &layers {
                        node = node.child(n("shape").child(pad_shape_node(pad, &lname(*l))));
                    }
                } else {
                    node = node.child(n("shape").child(pad_shape_node(pad, "F.Cu")));
                }
                node.child(n("attach").sym("off"))
            });
            let mut pin = n("pin").str(&ps);
            if pad.rotation != 0.0 {
                pin = pin.child(n("rotate").num(pad.rotation));
            }
            image = image.child(pin.str(&pad.number).num(um(pad.at.x)).num(um(pad.at.y)));
        }
        library = library.child(image);
    }
    for (_, node) in padstacks {
        library = library.child(node);
    }
    for (dia, dr) in &via_list {
        let mut node = n("padstack").str(via_name(*dia, *dr));
        for l in &layers {
            node = node.child(n("shape").child(n("circle").sym(lname(*l)).num(um(*dia))));
        }
        library = library.child(node.child(n("attach").sym("off")));
    }

    // Network: nets, pins and classes.
    let mut network = n("network");
    let placed = |pr: &PadRef| d.placement.placements.contains_key(&pr.part);
    for (name, net) in &d.nets.nets {
        let mut pins = n("pins");
        for pr in net.pads.iter().filter(|p| placed(p)) {
            pins = pins.sym(format!("{}-{}", pr.part, pr.pad));
        }
        network = network.child(n("net").str(name).child(pins));
    }
    let mut classes: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (name, net) in &d.nets.nets {
        classes
            .entry(net.class.clone().unwrap_or_default())
            .or_default()
            .push(name);
    }
    for (class, nets) in &classes {
        let nc = rules.class(Some(class).filter(|c| !c.is_empty()).map(String::as_str));
        let dia = nc.via_diameter.unwrap_or(rules.via_diameter);
        let dr = nc.via_drill.unwrap_or(
            rules
                .via_diameter
                .min(nc.via_drill.unwrap_or(rules.via_drill)),
        );
        let mut node = n("class").str(if class.is_empty() { "default" } else { class });
        for net in nets {
            node = node.str(*net);
        }
        network = network.child(
            node.child(n("circuit").child(n("use_via").str(via_name(dia, dr))))
                .child(
                    n("rule")
                        .child(n("width").num(um(nc.track_width.unwrap_or(rules.track_width))))
                        .child(n("clearance").num(um(nc.clearance.unwrap_or(rules.clearance)))),
                ),
        );
    }

    // The existing wiring. Keep it as it is.
    let mut wiring = n("wiring");
    if !reroute_all {
        for t in &d.routing.tracks {
            if !d.nets.nets.contains_key(&t.net) {
                continue;
            }
            let mut path = n("path").sym(lname(t.layer)).num(um(t.width));
            for p in &t.points {
                path = path.num(um(p.x)).num(um(p.y));
            }
            wiring = wiring.child(
                n("wire")
                    .child(path)
                    .child(n("net").str(&t.net))
                    .child(n("type").sym("protect")),
            );
        }
        for v in &d.routing.vias {
            if !d.nets.nets.contains_key(&v.net) {
                continue;
            }
            wiring = wiring.child(
                n("via")
                    .str(via_name(v.diameter, v.drill))
                    .num(um(v.at.x))
                    .num(um(v.at.y))
                    .child(n("net").str(&v.net))
                    .child(n("type").sym("protect")),
            );
        }
    }

    let root = n("pcb")
        .str(format!("{name}.dsn"))
        .child(
            n("parser")
                .child(n("string_quote").sym("\""))
                .child(n("space_in_quoted_tokens").sym("on"))
                .child(n("host_cad").str("Lamina"))
                .child(n("host_version").str(env!("CARGO_PKG_VERSION"))),
        )
        .child(n("resolution").sym("um").num(10.0))
        .child(n("unit").sym("um"))
        .child(structure)
        .child(placement)
        .child(library)
        .child(network)
        .child(wiring);
    sexpr::write(&root.done())
}

pub use lamina_ports::router::Session;

/// Read a Specctra session (`.ses`) that Freerouting wrote.
pub fn import_ses(src: &str, d: &ProjectData) -> Result<Session, String> {
    let root = sexpr::parse(src)?;
    if !root.is("session") {
        return Err(format!(
            "this is not a Specctra session (it starts with `{}`)",
            root.name().unwrap_or("?")
        ));
    }
    let routes = root
        .child("routes")
        .ok_or("the session has no routes section")?;
    // Values are in `unit` divided by `resolution`.
    let unit = routes
        .child("resolution")
        .and_then(|r| r.arg(0))
        .unwrap_or("um")
        .to_string();
    let res = routes
        .child("resolution")
        .and_then(|r| r.arg_num(1))
        .unwrap_or(1.0)
        .max(1e-9);
    let to_mm = |v: f64| -> f64 {
        let base = v / res;
        r6(match unit.as_str() {
            "um" => base / 1000.0,
            "mm" => base,
            "mil" => base * 0.0254,
            "inch" => base * 25.4,
            _ => base / 1000.0,
        })
    };
    let copper = d.board.stackup.copper_layers;
    // The via padstacks that the session defines, by name.
    let mut via_defs: BTreeMap<String, f64> = BTreeMap::new();
    if let Some(lib) = routes.child("library_out") {
        for ps in lib.children("padstack") {
            let name = ps.arg(0).unwrap_or("").to_string();
            let dia = ps
                .children("shape")
                .filter_map(|s| s.child("circle"))
                .filter_map(|c| c.arg_num(1))
                .fold(0.0_f64, f64::max);
            if dia > 0.0 {
                via_defs.insert(name, to_mm(dia));
            }
        }
    }
    let mut s = Session::default();
    let Some(network) = routes.child("network_out") else {
        return Ok(s);
    };
    let mut tid = 0;
    let mut vid = 0;
    for net in network.children("net") {
        let name = net.arg(0).unwrap_or("").to_string();
        if !d.nets.nets.contains_key(&name) {
            s.notes.push(format!(
                "net {name} is not on the board. Its routes are not used"
            ));
            continue;
        }
        s.nets.insert(name.clone());
        for wire in net.children("wire") {
            let Some(path) = wire.child("path") else {
                continue;
            };
            let Some(layer) = path.arg(0).and_then(layer_from_kicad) else {
                s.notes.push(format!(
                    "a wire is on an unknown layer {}",
                    path.arg(0).unwrap_or("?")
                ));
                continue;
            };
            if !layer.exists_in(copper) {
                continue;
            }
            let width = to_mm(path.arg_num(1).unwrap_or(0.0));
            let nums: Vec<f64> = path.args()[2..].iter().filter_map(Sx::number).collect();
            let mut points: Vec<Point> = nums
                .chunks(2)
                .filter(|c| c.len() == 2)
                .map(|c| Point::new(to_mm(c[0]), to_mm(c[1])))
                .collect();
            points.dedup_by(|a, b| a.dist(*b) < 1e-6);
            if points.len() < 2 || width <= 0.0 {
                continue;
            }
            tid += 1;
            s.tracks.push(Track {
                id: format!("t{tid}"),
                net: name.clone(),
                layer,
                width,
                points,
            });
        }
        for via in net.children("via") {
            let ps = via.arg(0).unwrap_or("").to_string();
            let (Some(x), Some(y)) = (via.arg_num(1), via.arg_num(2)) else {
                continue;
            };
            let (diameter, drill) = match via_sizes(&ps) {
                Some(v) => v,
                None => (
                    via_defs
                        .get(&ps)
                        .copied()
                        .unwrap_or(d.board.rules.via_diameter),
                    d.board.rules.via_drill,
                ),
            };
            vid += 1;
            s.vias.push(Via {
                id: format!("v{vid}"),
                net: name.clone(),
                at: Point::new(to_mm(x), to_mm(y)),
                diameter: r6(diameter),
                drill: r6(drill.min(diameter - 0.05)),
                from: None,
                to: None,
            });
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> ProjectData {
        let mut d = ProjectData::new("t");
        d.board.outline = Some(Contour::rect(0.0, 0.0, 20.0, 10.0));
        let mut fp = Footprint::new("R0603", Provenance::Manual { note: "t".into() });
        for (num, x) in [("1", -0.825), ("2", 0.825)] {
            fp.pads.push(Pad::smd(
                num,
                Point::new(x, 0.0),
                PadShape::RoundRect {
                    width: 0.8,
                    height: 0.95,
                    radius: 0.2,
                },
            ));
        }
        d.footprints.insert("R0603".into(), fp);
        let mut hdr = Footprint::new("HDR2", Provenance::Manual { note: "t".into() });
        hdr.pads.push(Pad::through(
            "1",
            Point::new(0.0, 0.0),
            PadShape::Rect {
                width: 1.7,
                height: 1.7,
            },
            Drill::Round { diameter: 1.0 },
        ));
        hdr.pads.push(Pad::through(
            "2",
            Point::new(2.54, 0.0),
            PadShape::Oval {
                width: 1.7,
                height: 2.0,
            },
            Drill::Round { diameter: 1.0 },
        ));
        d.footprints.insert("HDR2".into(), hdr);
        d.parts.parts.insert("R1".into(), Part::new("R0603"));
        d.parts.parts.insert("R2".into(), Part::new("R0603"));
        d.parts.parts.insert("J1".into(), Part::new("HDR2"));
        d.placement
            .placements
            .insert("R1".into(), Placement::at(4.0, 5.0));
        d.placement.placements.insert(
            "R2".into(),
            Placement {
                x: 10.0,
                y: 5.0,
                rotation: 90.0,
                side: Side::Bottom,
                locked: false,
            },
        );
        d.placement
            .placements
            .insert("J1".into(), Placement::at(15.0, 5.0));
        d.nets.nets.insert(
            "GND".into(),
            Net {
                pads: vec![PadRef::new("R1", "2"), PadRef::new("J1", "1")],
                class: Some("power".into()),
            },
        );
        d.nets.nets.insert(
            "SIG".into(),
            Net {
                pads: vec![PadRef::new("R1", "1"), PadRef::new("R2", "1")],
                class: None,
            },
        );
        d.board.rules.net_classes.insert(
            "power".into(),
            NetClass {
                clearance: None,
                track_width: Some(0.5),
                via_diameter: Some(0.8),
                via_drill: Some(0.4),
            },
        );
        d.routing.tracks.push(Track {
            id: "t1".into(),
            net: "SIG".into(),
            layer: Layer::TOP,
            width: 0.25,
            points: vec![Point::new(3.175, 5.0), Point::new(6.0, 5.0)],
        });
        d
    }

    #[test]
    fn dsn_has_the_expected_sections() {
        let d = board();
        let text = export_dsn(&d, "t", false);
        let root = sexpr::parse(&text).unwrap();
        assert!(root.is("pcb"));
        let st = root.child("structure").unwrap();
        assert_eq!(st.children("layer").count(), 2);
        let boundary = st.child("boundary").unwrap().child("path").unwrap();
        assert_eq!(boundary.arg(0), Some("pcb"));
        assert!(
            boundary
                .args()
                .iter()
                .filter_map(Sx::number)
                .any(|v| (v - 20000.0).abs() < 1e-6)
        );
        let vias: Vec<&str> = st.child("via").unwrap().atoms();
        assert!(
            vias.contains(&"Via[0-1]_600:300_um") && vias.contains(&"Via[0-1]_800:400_um"),
            "{vias:?}"
        );
        let pl = root.child("placement").unwrap();
        let comps: Vec<&Sx> = pl.children("component").collect();
        assert_eq!(comps.len(), 2);
        let r2 = comps
            .iter()
            .flat_map(|c| c.children("place"))
            .find(|p| p.arg(0) == Some("R2"))
            .unwrap();
        assert_eq!(r2.atoms(), vec!["R2", "10000", "5000", "back", "90"]);
        let lib = root.child("library").unwrap();
        let image = lib
            .children("image")
            .find(|i| i.arg(0) == Some("R0603"))
            .unwrap();
        assert_eq!(image.children("pin").count(), 2);
        assert!(
            lib.children("padstack")
                .any(|p| p.arg(0).unwrap().starts_with("RoundRect[T]Pad_800x950"))
        );
        assert!(
            lib.children("padstack")
                .any(|p| p.arg(0).unwrap().starts_with("Oval[A]Pad_1700x2000"))
        );
        let net = root.child("network").unwrap();
        let gnd = net
            .children("net")
            .find(|x| x.arg(0) == Some("GND"))
            .unwrap();
        assert_eq!(gnd.child("pins").unwrap().atoms(), vec!["R1-2", "J1-1"]);
        let power = net
            .children("class")
            .find(|c| c.arg(0) == Some("power"))
            .unwrap();
        assert_eq!(power.child("rule").unwrap().child_num("width"), Some(500.0));
        let rule = st.child("rule").unwrap();
        let via_via = rule
            .children("clearance")
            .find(|c| c.child_text("type") == Some("via_via"))
            .unwrap();
        // With hole_to_hole 0.5 and a 0.6/0.3 via, the copper gap must be 0.2 mm.
        assert_eq!(via_via.arg_num(0), Some(200.0));
        let wiring = root.child("wiring").unwrap();
        assert_eq!(wiring.children("wire").count(), 1);
        assert!(!export_dsn(&d, "t", true).contains("(wire"));
    }

    #[test]
    fn via_names_round_trip() {
        assert_eq!(via_sizes("Via[0-1]_600:300_um"), Some((0.6, 0.3)));
        assert_eq!(via_sizes(&via_name(0.8, 0.4)), Some((0.8, 0.4)));
        assert_eq!(via_sizes("Round[A]Pad_1000_um"), None);
    }

    #[test]
    fn reads_a_freerouting_session() {
        let d = board();
        let ses = r#"(session "t.ses"
  (base_design "t.dsn")
  (placement (resolution um 10)
    (component "R0603" (place R1 40000 50000 front 0)))
  (was_is)
  (routes
    (resolution um 10)
    (parser (host_cad "Lamina") (host_version "0.1"))
    (library_out
      (padstack "Via[0-1]_600:300_um" (shape (circle F.Cu 6000 0 0)) (shape (circle B.Cu 6000 0 0)) (attach off)))
    (network_out
      (net "SIG"
        (wire (path F.Cu 2500 31750 50000 60000 50000 60000 70000))
        (via "Via[0-1]_600:300_um" 60000 70000)
        (wire (path B.Cu 2500 60000 70000 100000 58250)))
      (net "NOPE" (wire (path F.Cu 2500 0 0 10000 0))))))"#;
        let s = import_ses(ses, &d).unwrap();
        assert_eq!(s.tracks.len(), 2);
        assert_eq!(
            s.tracks[0].points,
            vec![
                Point::new(3.175, 5.0),
                Point::new(6.0, 5.0),
                Point::new(6.0, 7.0)
            ]
        );
        assert_eq!(s.tracks[0].width, 0.25);
        assert_eq!(s.tracks[1].layer, Layer::BOTTOM);
        assert_eq!(s.vias.len(), 1);
        assert_eq!(
            (s.vias[0].at, s.vias[0].diameter, s.vias[0].drill),
            (Point::new(6.0, 7.0), 0.6, 0.3)
        );
        assert!(s.nets.contains("SIG") && !s.nets.contains("NOPE"));
        assert!(s.notes.iter().any(|n| n.contains("NOPE")));
    }
}
