//! STEP (AP214) export of the assembled board. The board is a solid with its
//! outline, cut-outs and drilled holes. The 3D model of each part is an
//! assembly component. The export copies all of each part STEP file, so its
//! exact geometry and colours stay the same. Models that are only meshes (OBJ)
//! become faceted surfaces.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use lamina_geom::Owner;
use lamina_geom::prim::Prim;
use lamina_model::*;

pub use lamina_ports::mesh::{StepExport, StepOptions};

// ---------------------------------------------------------------- writer

struct W {
    out: Vec<u8>,
    next: u32,
}

impl W {
    fn add(&mut self, body: impl AsRef<str>) -> u32 {
        let id = self.next;
        self.next += 1;
        self.out
            .extend_from_slice(format!("#{id}={};\n", body.as_ref()).as_bytes());
        id
    }

    /// Reserve an id. Define it later.
    fn reserve(&mut self) -> u32 {
        let id = self.next;
        self.next += 1;
        id
    }

    fn define(&mut self, id: u32, body: impl AsRef<str>) {
        self.out
            .extend_from_slice(format!("#{id}={};\n", body.as_ref()).as_bytes());
    }

    fn point(&mut self, p: [f64; 3]) -> u32 {
        self.add(format!(
            "CARTESIAN_POINT('',({},{},{}))",
            r(p[0]),
            r(p[1]),
            r(p[2])
        ))
    }

    fn dir(&mut self, d: [f64; 3]) -> u32 {
        self.add(format!(
            "DIRECTION('',({},{},{}))",
            r(d[0]),
            r(d[1]),
            r(d[2])
        ))
    }

    fn axis(&mut self, o: [f64; 3], z: [f64; 3], x: [f64; 3]) -> u32 {
        let p = self.point(o);
        let zd = self.dir(z);
        let xd = self.dir(x);
        self.add(format!("AXIS2_PLACEMENT_3D('',#{p},#{zd},#{xd})"))
    }
}

/// A STEP real. It always has a decimal point.
fn r(v: f64) -> String {
    let v = if v.abs() < 1e-12 { 0.0 } else { v };
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{v:.0}.");
    }
    let s = format!("{v:.9}");
    let s = s.trim_end_matches('0');
    s.to_string()
}

/// A STEP string literal. Non-ASCII text is encoded as `\X2\`.
fn s(text: &str) -> String {
    let mut out = String::from("'");
    let mut wide = String::new();
    let flush = |out: &mut String, wide: &mut String| {
        if !wide.is_empty() {
            out.push_str("\\X2\\");
            out.push_str(wide);
            out.push_str("\\X0\\");
            wide.clear();
        }
    };
    for c in text.chars() {
        if c.is_ascii() && !c.is_ascii_control() {
            flush(&mut out, &mut wide);
            match c {
                '\'' => out.push_str("''"),
                '\\' => out.push_str("\\\\"),
                c => out.push(c),
            }
        } else if (c as u32) <= 0xFFFF {
            let _ = write!(wide, "{:04X}", c as u32);
        }
    }
    flush(&mut out, &mut wide);
    out.push('\'');
    out
}

fn ids(v: &[u32]) -> String {
    v.iter()
        .map(|i| format!("#{i}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn b(v: bool) -> &'static str {
    if v { ".T." } else { ".F." }
}

// ---------------------------------------------------------------- f64 transforms (same as the 3D scene)

type M4 = [[f64; 4]; 4];

fn mul(a: &M4, b: &M4) -> M4 {
    let mut m = [[0.0; 4]; 4];
    for (c, col) in m.iter_mut().enumerate() {
        for (row, v) in col.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][row] * b[c][k]).sum();
        }
    }
    m
}

fn translate(x: f64, y: f64, z: f64) -> M4 {
    [
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [0., 0., 1., 0.],
        [x, y, z, 1.],
    ]
}

fn rot_x(deg: f64) -> M4 {
    let (s, c) = deg.to_radians().sin_cos();
    [
        [1., 0., 0., 0.],
        [0., c, s, 0.],
        [0., -s, c, 0.],
        [0., 0., 0., 1.],
    ]
}

fn rot_y(deg: f64) -> M4 {
    let (s, c) = deg.to_radians().sin_cos();
    [
        [c, 0., -s, 0.],
        [0., 1., 0., 0.],
        [s, 0., c, 0.],
        [0., 0., 0., 1.],
    ]
}

fn rot_z(deg: f64) -> M4 {
    let (s, c) = deg.to_radians().sin_cos();
    [
        [c, s, 0., 0.],
        [-s, c, 0., 0.],
        [0., 0., 1., 0.],
        [0., 0., 0., 1.],
    ]
}

fn apply(m: &M4, p: [f64; 3], w: f64) -> [f64; 3] {
    let mut o = [0.0; 3];
    for (row, v) in o.iter_mut().enumerate() {
        *v = m[0][row] * p[0] + m[1][row] * p[1] + m[2][row] * p[2] + m[3][row] * w;
    }
    o
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-15);
    [v[0] / l, v[1] / l, v[2] / l]
}

/// The transform of the model of a part in the board frame (rigid: no scale).
fn model_transform(pl: &Placement, mr: &ModelRef, thickness: f64) -> M4 {
    let base = mul(&translate(pl.x, pl.y, 0.0), &rot_z(pl.rotation));
    let place = match pl.side {
        Side::Top => mul(&base, &translate(0.0, 0.0, thickness)),
        Side::Bottom => mul(&base, &rot_y(180.0)),
    };
    let local = mul(
        &translate(mr.offset[0], mr.offset[1], mr.offset[2]),
        &mul(
            &rot_z(mr.rotation[2]),
            &mul(&rot_y(mr.rotation[1]), &rot_x(mr.rotation[0])),
        ),
    );
    mul(&place, &local)
}

// ---------------------------------------------------------------- board solid

/// Reverse the direction of a contour. Arcs keep their centres and change their sense.
fn reversed(c: &Contour) -> Contour {
    let n = c.vertices.len();
    let mut v = Vec::with_capacity(n);
    for k in (0..n).rev() {
        // Edge k goes from vertex k to k+1. When reversed, it starts at k+1.
        let next = c.vertices[(k + 1) % n];
        v.push(Vertex {
            x: next.x,
            y: next.y,
            arc: c.vertices[k].arc.map(|a| ArcTo {
                center: a.center,
                ccw: !a.ccw,
            }),
        });
    }
    Contour { vertices: v }
}

/// A full circle as two half arcs, so that each edge has two different ends.
fn split_circle(c: &Contour) -> Contour {
    if c.vertices.len() != 1 {
        return c.clone();
    }
    let v = c.vertices[0];
    let a = v.arc.expect("one-vertex contour is a circle");
    let p = v.point();
    let opp = Point::new(2.0 * a.center.x - p.x, 2.0 * a.center.y - p.y);
    Contour {
        vertices: vec![
            Vertex {
                x: p.x,
                y: p.y,
                arc: Some(a),
            },
            Vertex {
                x: opp.x,
                y: opp.y,
                arc: Some(a),
            },
        ],
    }
}

/// Turn a loop so that the material is on its left: outer CCW, holes CW.
fn oriented(c: &Contour, outer: bool) -> Contour {
    let c = split_circle(c);
    if (c.signed_area() > 0.0) == outer {
        c
    } else {
        reversed(&c)
    }
}

fn circle_loop(center: Point, radius: f64) -> Contour {
    Contour {
        vertices: vec![
            Vertex {
                x: center.x + radius,
                y: center.y,
                arc: Some(ArcTo { center, ccw: true }),
            },
            Vertex {
                x: center.x - radius,
                y: center.y,
                arc: Some(ArcTo { center, ccw: true }),
            },
        ],
    }
}

fn slot_loop(a: Point, bb: Point, radius: f64) -> Contour {
    let d = bb.sub(a);
    let len = d.x.hypot(d.y).max(1e-12);
    let n = Point::new(-d.y / len * radius, d.x / len * radius);
    Contour {
        vertices: vec![
            Vertex::line(a.x - n.x, a.y - n.y),
            Vertex {
                x: bb.x - n.x,
                y: bb.y - n.y,
                arc: Some(ArcTo {
                    center: bb,
                    ccw: true,
                }),
            },
            Vertex::line(bb.x + n.x, bb.y + n.y),
            Vertex {
                x: a.x + n.x,
                y: a.y + n.y,
                arc: Some(ArcTo {
                    center: a,
                    ccw: true,
                }),
            },
        ],
    }
}

/// The edges and side faces of one profile loop, extruded from z = 0 to `t`.
struct LoopEdges {
    /// The bottom and top edge of each segment, and true if the loop goes in
    /// the direction of the edge.
    bottom: Vec<(u32, bool)>,
    top: Vec<(u32, bool)>,
}

fn extrude_loop(w: &mut W, c: &Contour, t: f64, faces: &mut Vec<u32>) -> LoopEdges {
    let n = c.vertices.len();
    let up = [0.0, 0.0, 1.0];
    let xdir = [1.0, 0.0, 0.0];
    let mut pb = Vec::with_capacity(n);
    let mut vb = Vec::with_capacity(n);
    let mut vt = Vec::with_capacity(n);
    let mut ev = Vec::with_capacity(n);
    for v in &c.vertices {
        let p0 = w.point([v.x, v.y, 0.0]);
        let p1 = w.point([v.x, v.y, t]);
        let b0 = w.add(format!("VERTEX_POINT('',#{p0})"));
        let b1 = w.add(format!("VERTEX_POINT('',#{p1})"));
        let d = w.dir(up);
        let vec = w.add(format!("VECTOR('',#{d},{})", r(t)));
        let line = w.add(format!("LINE('',#{p0},#{vec})"));
        let e = w.add(format!("EDGE_CURVE('',#{b0},#{b1},#{line},.T.)"));
        pb.push(p0);
        vb.push(b0);
        vt.push(b1);
        ev.push(e);
    }
    let mut bottom = Vec::with_capacity(n);
    let mut top = Vec::with_capacity(n);
    for i in 0..n {
        let j = (i + 1) % n;
        let a = c.vertices[i].point();
        let bpt = c.vertices[j].point();
        let (eb, et, fwd, surface, same) = match c.vertices[i].arc {
            None => {
                let d = bpt.sub(a);
                let len = d.x.hypot(d.y).max(1e-12);
                let dir = [d.x / len, d.y / len, 0.0];
                let edge = |z: f64, v0: u32, v1: u32, w: &mut W| {
                    let p = w.point([a.x, a.y, z]);
                    let dd = w.dir(dir);
                    let vec = w.add(format!("VECTOR('',#{dd},{})", r(len)));
                    let line = w.add(format!("LINE('',#{p},#{vec})"));
                    w.add(format!("EDGE_CURVE('',#{v0},#{v1},#{line},.T.)"))
                };
                let eb = edge(0.0, vb[i], vb[j], w);
                let et = edge(t, vt[i], vt[j], w);
                // The outward normal is to the right of the direction (material on the left).
                let ax = w.axis([a.x, a.y, 0.0], [dir[1], -dir[0], 0.0], dir);
                let plane = w.add(format!("PLANE('',#{ax})"));
                (eb, et, true, plane, true)
            }
            Some(arc) => {
                let rad = a.dist(arc.center);
                let edge = |z: f64, v0: u32, v1: u32, w: &mut W| {
                    let ax = w.axis([arc.center.x, arc.center.y, z], up, xdir);
                    let circle = w.add(format!("CIRCLE('',#{ax},{})", r(rad)));
                    // Circles go counter-clockwise. A clockwise arc is the edge
                    // from its end to its start.
                    if arc.ccw {
                        w.add(format!("EDGE_CURVE('',#{v0},#{v1},#{circle},.T.)"))
                    } else {
                        w.add(format!("EDGE_CURVE('',#{v1},#{v0},#{circle},.T.)"))
                    }
                };
                let eb = edge(0.0, vb[i], vb[j], w);
                let et = edge(t, vt[i], vt[j], w);
                let ax = w.axis([arc.center.x, arc.center.y, 0.0], up, xdir);
                let cyl = w.add(format!("CYLINDRICAL_SURFACE('',#{ax},{})", r(rad)));
                (eb, et, arc.ccw, cyl, arc.ccw)
            }
        };
        // The side face, as seen from outside: bottom forward, up, top back, down.
        let oe = [
            w.add(format!("ORIENTED_EDGE('',*,*,#{eb},{})", b(fwd))),
            w.add(format!("ORIENTED_EDGE('',*,*,#{},.T.)", ev[j])),
            w.add(format!("ORIENTED_EDGE('',*,*,#{et},{})", b(!fwd))),
            w.add(format!("ORIENTED_EDGE('',*,*,#{},.F.)", ev[i])),
        ];
        let lp = w.add(format!("EDGE_LOOP('',({}))", ids(&oe)));
        let bound = w.add(format!("FACE_OUTER_BOUND('',#{lp},.T.)"));
        faces.push(w.add(format!(
            "ADVANCED_FACE('',(#{bound}),#{surface},{})",
            b(same)
        )));
        bottom.push((eb, fwd));
        top.push((et, fwd));
    }
    let _ = pb;
    LoopEdges { bottom, top }
}

/// Make the board as a closed B-rep solid. Return the id of the solid.
fn board_solid(w: &mut W, outer: &Contour, inner: &[Contour], t: f64) -> u32 {
    let mut faces = Vec::new();
    let outer = oriented(outer, true);
    let inner: Vec<Contour> = inner.iter().map(|c| oriented(c, false)).collect();
    let mut loops = vec![extrude_loop(w, &outer, t, &mut faces)];
    for c in &inner {
        loops.push(extrude_loop(w, c, t, &mut faces));
    }
    // Top: the loops in their direction (the outer loop is counter-clockwise from above).
    let mut top_bounds = Vec::new();
    // Bottom: the face normal points down, so each loop goes in the opposite direction.
    let mut bottom_bounds = Vec::new();
    for (k, l) in loops.iter().enumerate() {
        let te: Vec<u32> = l
            .top
            .iter()
            .map(|(e, fwd)| w.add(format!("ORIENTED_EDGE('',*,*,#{e},{})", b(*fwd))))
            .collect();
        let be: Vec<u32> = l
            .bottom
            .iter()
            .rev()
            .map(|(e, fwd)| w.add(format!("ORIENTED_EDGE('',*,*,#{e},{})", b(!*fwd))))
            .collect();
        let tl = w.add(format!("EDGE_LOOP('',({}))", ids(&te)));
        let bl = w.add(format!("EDGE_LOOP('',({}))", ids(&be)));
        let kind = if k == 0 {
            "FACE_OUTER_BOUND"
        } else {
            "FACE_BOUND"
        };
        top_bounds.push(w.add(format!("{kind}('',#{tl},.T.)")));
        bottom_bounds.push(w.add(format!("{kind}('',#{bl},.T.)")));
    }
    let ax_t = w.axis([0.0, 0.0, t], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let pl_t = w.add(format!("PLANE('',#{ax_t})"));
    faces.push(w.add(format!(
        "ADVANCED_FACE('',({}),#{pl_t},.T.)",
        ids(&top_bounds)
    )));
    let ax_b = w.axis([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let pl_b = w.add(format!("PLANE('',#{ax_b})"));
    faces.push(w.add(format!(
        "ADVANCED_FACE('',({}),#{pl_b},.F.)",
        ids(&bottom_bounds)
    )));
    let shell = w.add(format!("CLOSED_SHELL('',({}))", ids(&faces)));
    w.add(format!("MANIFOLD_SOLID_BREP('PCB',#{shell})"))
}

fn style(w: &mut W, item: u32, rgb: [f64; 3], ctx: u32) {
    let c = w.add(format!(
        "COLOUR_RGB('',{},{},{})",
        r(rgb[0]),
        r(rgb[1]),
        r(rgb[2])
    ));
    let fac = w.add(format!("FILL_AREA_STYLE_COLOUR('',#{c})"));
    let fas = w.add(format!("FILL_AREA_STYLE('',(#{fac}))"));
    let ssfa = w.add(format!("SURFACE_STYLE_FILL_AREA(#{fas})"));
    let sss = w.add(format!("SURFACE_SIDE_STYLE('',(#{ssfa}))"));
    let ssu = w.add(format!("SURFACE_STYLE_USAGE(.BOTH.,#{sss})"));
    let psa = w.add(format!("PRESENTATION_STYLE_ASSIGNMENT((#{ssu}))"));
    let si = w.add(format!("STYLED_ITEM('color',(#{psa}),#{item})"));
    w.add(format!(
        "MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION('',(#{si}),#{ctx})"
    ));
}

// ---------------------------------------------------------------- products

struct Ctx {
    product_ctx: u32,
    pd_ctx: u32,
    geo: u32,
}

fn contexts(w: &mut W) -> Ctx {
    let app = w.add("APPLICATION_CONTEXT('core data for automotive mechanical design processes')");
    w.add(format!(
        "APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{app})"
    ));
    let product_ctx = w.add(format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
    let pd_ctx = w.add(format!(
        "PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"
    ));
    let len = w.add("( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) )");
    let ang = w.add("( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) )");
    let sol = w.add("( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() )");
    let unc = w.add(format!(
        "UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-07),#{len},'distance_accuracy_value','confusion accuracy')"
    ));
    let geo = w.add(format!(
        "( GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc})) GLOBAL_UNIT_ASSIGNED_CONTEXT((#{len},#{ang},#{sol})) REPRESENTATION_CONTEXT('Context #1','3D Context with UNIT and UNCERTAINTY') )"
    ));
    Ctx {
        product_ctx,
        pd_ctx,
        geo,
    }
}

/// Make a product with its definition. Return (product definition, its shape).
fn product(w: &mut W, c: &Ctx, name: &str) -> (u32, u32) {
    let n = s(name);
    let p = w.add(format!("PRODUCT({n},{n},'',(#{}))", c.product_ctx));
    w.add(format!("PRODUCT_RELATED_PRODUCT_CATEGORY('part',$,(#{p}))"));
    let pdf = w.add(format!("PRODUCT_DEFINITION_FORMATION('','',#{p})"));
    let pd = w.add(format!(
        "PRODUCT_DEFINITION('design','',#{pdf},#{})",
        c.pd_ctx
    ));
    let pds = w.add(format!("PRODUCT_DEFINITION_SHAPE('','',#{pd})"));
    (pd, pds)
}

/// Put `child` (product definition and representation) in `parent`.
fn place(w: &mut W, parent: (u32, u32), child: (u32, u32), name: &str, k: usize, m: &M4) -> u32 {
    let o = apply(m, [0.0; 3], 1.0);
    let z = unit(apply(m, [0.0, 0.0, 1.0], 0.0));
    let x = unit(apply(m, [1.0, 0.0, 0.0], 0.0));
    let parent_axis = w.axis(o, z, x);
    let child_axis = w.axis([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let nauo = w.add(format!(
        "NEXT_ASSEMBLY_USAGE_OCCURRENCE('NAUO{k}',{},'',#{},#{},$)",
        s(name),
        parent.0,
        child.0
    ));
    let pds = w.add(format!(
        "PRODUCT_DEFINITION_SHAPE('Placement','Placement of an item',#{nauo})"
    ));
    let idt = w.add(format!(
        "ITEM_DEFINED_TRANSFORMATION('','',#{child_axis},#{parent_axis})"
    ));
    let rr = w.add(format!(
        "( REPRESENTATION_RELATIONSHIP('','',#{},#{}) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{idt}) SHAPE_REPRESENTATION_RELATIONSHIP() )",
        child.1, parent.1
    ));
    w.add(format!(
        "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{rr},#{pds})"
    ));
    parent_axis
}

// ---------------------------------------------------------------- copies of part files

/// The entities of the DATA section of a STEP file, with `offset` added to each
/// `#n`, and the largest id found. This works on bytes, so text in all
/// encodings stays the same.
fn renumber(text: &[u8], offset: u32) -> Result<(Vec<u8>, u32), String> {
    let find = |from: usize, pat: &[u8]| {
        text[from..]
            .windows(pat.len())
            .position(|w| w == pat)
            .map(|k| k + from)
    };
    let start = find(0, b"DATA;").ok_or("no DATA section")? + 5;
    let b = text;
    let mut out = Vec::with_capacity(b.len() - start + 1024);
    let mut i = start;
    let mut max = 0u32;
    let mut seg = start;
    while i < b.len() {
        match b[i] {
            b'\'' => {
                i += 1;
                while i < b.len() {
                    if b[i] == b'\'' {
                        if b.get(i + 1) == Some(&b'\'') {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = find(i + 2, b"*/").map(|k| k + 2).unwrap_or(b.len());
            }
            b'#' => {
                let s0 = i + 1;
                let mut e = s0;
                while e < b.len() && b[e].is_ascii_digit() {
                    e += 1;
                }
                if e > s0 {
                    let n: u32 = std::str::from_utf8(&b[s0..e])
                        .ok()
                        .and_then(|t| t.parse().ok())
                        .ok_or("The entity id is not valid")?;
                    max = max.max(n);
                    out.extend_from_slice(&b[seg..i]);
                    out.extend_from_slice(format!("#{}", n + offset).as_bytes());
                    seg = e;
                }
                i = e;
            }
            b'E' if b[i..].starts_with(b"ENDSEC;") => {
                out.extend_from_slice(&b[seg..i]);
                return Ok((out, max));
            }
            _ => i += 1,
        }
    }
    Err("The DATA section has no ENDSEC".into())
}

/// The top product definition of a STEP file and its shape representation.
fn root_of(text: &[u8]) -> Option<(u32, u32)> {
    let m = lamina_step::p21::parse(text).ok()?;
    let mut children = std::collections::HashSet::new();
    for e in m.values() {
        if e.kind() == "NEXT_ASSEMBLY_USAGE_OCCURRENCE" {
            if let Some(c) = e.args().get(4).and_then(|a| a.id()) {
                children.insert(c);
            }
        }
    }
    // product definition shape -> product definition
    let mut pds_of: HashMap<u32, u32> = HashMap::new();
    for (id, e) in &m {
        if e.kind() == "PRODUCT_DEFINITION_SHAPE" {
            if let Some(pd) = e.args().get(2).and_then(|a| a.id()) {
                pds_of.insert(*id, pd);
            }
        }
    }
    let mut roots: Vec<(u32, u32)> = Vec::new();
    for e in m.values() {
        if e.kind() != "SHAPE_DEFINITION_REPRESENTATION" {
            continue;
        }
        let (Some(pds), Some(rep)) = (
            e.args().first().and_then(|a| a.id()),
            e.args().get(1).and_then(|a| a.id()),
        ) else {
            continue;
        };
        let Some(pd) = pds_of.get(&pds) else { continue };
        if m.get(pd).is_some_and(|x| x.kind() == "PRODUCT_DEFINITION") && !children.contains(pd) {
            roots.push((*pd, rep));
        }
    }
    roots.sort();
    roots.into_iter().next()
}

/// Make a mesh into a faceted surface model. Return (product definition, rep).
fn mesh_product(w: &mut W, c: &Ctx, name: &str, mesh: &lamina_step::Mesh) -> (u32, u32) {
    let (pd, pds) = product(w, c, name);
    let mut points: HashMap<[u32; 3], u32> = HashMap::new();
    let mut shells = Vec::new();
    for part in &mesh.parts {
        let mut faces = Vec::new();
        for tri in part.positions.chunks(3) {
            if tri.len() < 3 {
                continue;
            }
            let p: Vec<[f64; 3]> = tri
                .iter()
                .map(|v| [v[0] as f64, v[1] as f64, v[2] as f64])
                .collect();
            let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
            let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
            let nrm = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let len = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt();
            let elen = (e1[0] * e1[0] + e1[1] * e1[1] + e1[2] * e1[2]).sqrt();
            if len < 1e-12 || elen < 1e-12 {
                continue;
            }
            let pid: Vec<u32> = tri
                .iter()
                .map(|v| {
                    let k = [v[0].to_bits(), v[1].to_bits(), v[2].to_bits()];
                    *points
                        .entry(k)
                        .or_insert_with(|| w.point([v[0] as f64, v[1] as f64, v[2] as f64]))
                })
                .collect();
            let lp = w.add(format!("POLY_LOOP('',({}))", ids(&pid)));
            let bound = w.add(format!("FACE_OUTER_BOUND('',#{lp},.T.)"));
            let nd = w.dir(unit(nrm));
            let xd = w.dir([e1[0] / elen, e1[1] / elen, e1[2] / elen]);
            let ax = w.add(format!("AXIS2_PLACEMENT_3D('',#{},#{nd},#{xd})", pid[0]));
            let plane = w.add(format!("PLANE('',#{ax})"));
            faces.push(w.add(format!("FACE_SURFACE('',(#{bound}),#{plane},.T.)")));
        }
        if faces.is_empty() {
            continue;
        }
        let shell = w.add(format!("OPEN_SHELL('',({}))", ids(&faces)));
        let lin = |v: f32| -> f64 {
            // Mesh colours are linear. STEP colours are display RGB.
            let v = v.clamp(0.0, 1.0) as f64;
            if v <= 0.0031308 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            }
        };
        shells.push((
            shell,
            [lin(part.color[0]), lin(part.color[1]), lin(part.color[2])],
        ));
    }
    let sm = w.add(format!(
        "SHELL_BASED_SURFACE_MODEL('',({}))",
        ids(&shells.iter().map(|x| x.0).collect::<Vec<_>>())
    ));
    for (shell, rgb) in &shells {
        style(w, *shell, *rgb, c.geo);
    }
    let origin = w.axis([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let rep = w.add(format!(
        "MANIFOLD_SURFACE_SHAPE_REPRESENTATION({},(#{sm},#{origin}),#{})",
        s(name),
        c.geo
    ));
    w.add(format!("SHAPE_DEFINITION_REPRESENTATION(#{pds},#{rep})"));
    (pd, rep)
}

// ---------------------------------------------------------------- export

pub fn export_step(
    d: &ProjectData,
    root: &Path,
    name: &str,
    opt: &StepOptions,
) -> Result<StepExport, String> {
    let outline = d
        .board
        .outline
        .as_ref()
        .ok_or("The board has no outline. Set an outline and try again.")?;
    let t = d.board.stackup.thickness;
    let mut w = W {
        out: Vec::new(),
        next: 1,
    };
    let mut out = StepExport::default();
    let c = contexts(&mut w);

    // Holes through the board.
    let g = lamina_geom::build(d);
    let mut inner: Vec<Contour> = d.board.cutouts.clone();
    let mut vias = 0;
    for dr in &g.drills {
        if matches!(dr.owner, Owner::Via { .. }) && !opt.vias {
            vias += 1;
            continue;
        }
        match &dr.prim {
            Prim::Circle { c, r } => inner.push(circle_loop(*c, *r)),
            Prim::Capsule { a, b: bb, r } => inner.push(slot_loop(*a, *bb, *r)),
            Prim::Poly { outer, .. } => {
                if let Some(cc) = contour_from_points(outer.clone()) {
                    inner.push(cc);
                }
            }
        }
    }
    if vias > 0 {
        out.notes.push(format!(
            "{vias} via hole(s) are not in the file. To drill them, set vias: true"
        ));
    }

    // The assembly and the board in it.
    let (top_pd, top_pds) = product(&mut w, &c, name);
    let top_rep = w.reserve();
    let (board_pd, board_pds) = product(&mut w, &c, &format!("{name} PCB"));
    let solid = board_solid(&mut w, outline, &inner, t);
    let board_axis = w.axis([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let board_rep = w.add(format!(
        "ADVANCED_BREP_SHAPE_REPRESENTATION({},(#{solid},#{board_axis}),#{})",
        s(&format!("{name} PCB")),
        c.geo
    ));
    w.add(format!(
        "SHAPE_DEFINITION_REPRESENTATION(#{board_pds},#{board_rep})"
    ));
    let mask = lamina_scene::theme::mask(&d.board.stackup.mask_color);
    style(
        &mut w,
        solid,
        [
            mask[0] as f64 / 255.0,
            mask[1] as f64 / 255.0,
            mask[2] as f64 / 255.0,
        ],
        c.geo,
    );
    let mut axes = vec![w.axis([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0])];
    axes.push(place(
        &mut w,
        (top_pd, top_rep),
        (board_pd, board_rep),
        "PCB",
        0,
        &translate(0.0, 0.0, 0.0),
    ));

    // Parts.
    let mut models: HashMap<String, Option<Placed>> = HashMap::new();
    let mut k = 0;
    if opt.parts {
        for (refdes, pl) in &d.placement.placements {
            let Some(part) = d.parts.parts.get(refdes) else {
                continue;
            };
            if part.dnp {
                continue;
            }
            let Some(mr) = d
                .footprints
                .get(&part.footprint)
                .and_then(|f| f.model.as_ref())
            else {
                continue;
            };
            let Some(path) = find_model(root, &mr.file) else {
                out.notes
                    .push(format!("{refdes}: the model {} is not found", mr.file));
                continue;
            };
            let scaled = mr.scale.iter().any(|s| (s - 1.0).abs() > 1e-9);
            let key = format!(
                "{}|{}",
                path.display(),
                if scaled {
                    format!("{:?}", mr.scale)
                } else {
                    String::new()
                }
            );
            let entry = match models.get(&key) {
                Some(e) => *e,
                None => {
                    let e = add_model(&mut w, &c, &path, mr, scaled, &mut out);
                    models.insert(key, e);
                    e
                }
            };
            let Some((child, exact)) = entry else {
                continue;
            };
            if exact {
                out.exact += 1;
            } else {
                out.meshed += 1;
            }
            k += 1;
            let m = model_transform(pl, mr, t);
            axes.push(place(&mut w, (top_pd, top_rep), child, refdes, k, &m));
        }
    }
    w.define(
        top_rep,
        format!(
            "SHAPE_REPRESENTATION({},({}),#{})",
            s(name),
            ids(&axes),
            c.geo
        ),
    );
    w.add(format!(
        "SHAPE_DEFINITION_REPRESENTATION(#{top_pds},#{top_rep})"
    ));

    let file = format!("{name}.step");
    let mut text = String::new();
    text.push_str("ISO-10303-21;\nHEADER;\n");
    text.push_str("FILE_DESCRIPTION(('Lamina board assembly'),'2;1');\n");
    let _ = writeln!(
        text,
        "FILE_NAME({},'{}',(''),(''),'Lamina {}','Lamina','');",
        s(&file),
        crate_date(),
        env!("CARGO_PKG_VERSION")
    );
    text.push_str(
        "FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\nENDSEC;\nDATA;\n",
    );
    let mut bytes = text.into_bytes();
    bytes.extend_from_slice(&w.out);
    bytes.extend_from_slice(b"ENDSEC;\nEND-ISO-10303-21;\n");
    out.bytes = bytes;
    Ok(out)
}

/// A model in the file: (product definition, representation), and true if it
/// is the exact STEP geometry and not a mesh.
type Placed = ((u32, u32), bool);

/// Copy a model file into the export (or make it a mesh).
fn add_model(
    w: &mut W,
    c: &Ctx,
    path: &Path,
    mr: &ModelRef,
    scaled: bool,
    out: &mut StepExport,
) -> Option<Placed> {
    let name = path
        .file_stem()
        .and_then(|x| x.to_str())
        .unwrap_or("model")
        .to_string();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !scaled && matches!(ext.as_str(), "step" | "stp") {
        if let Ok(text) = std::fs::read(path) {
            if let Some((pd, rep)) = root_of(&text) {
                let offset = w.next;
                if let Ok((body, max)) = renumber(&text, offset) {
                    let lead = body
                        .iter()
                        .take_while(|c| matches!(c, b'\r' | b'\n'))
                        .count();
                    w.out.extend_from_slice(&body[lead..]);
                    if w.out.last() != Some(&b'\n') {
                        w.out.push(b'\n');
                    }
                    w.next = offset + max + 1;
                    return Some(((pd + offset, rep + offset), true));
                }
            }
            out.notes.push(format!(
                "{name}: the STEP file has no product structure, so it is exported as a mesh"
            ));
        }
    }
    match lamina_step::load(path) {
        Ok(mut mesh) => {
            if scaled {
                for p in mesh.parts.iter_mut() {
                    for v in p.positions.iter_mut() {
                        for (c, k) in v.iter_mut().zip(mr.scale) {
                            *c *= k as f32;
                        }
                    }
                }
            }
            Some((mesh_product(w, c, &name, &mesh), false))
        }
        Err(e) => {
            out.notes
                .push(format!("{name}: the model does not load ({e})"));
            None
        }
    }
}

fn crate_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> ProjectData {
        let mut d = ProjectData::new("t");
        d.board.outline = Some(Contour::rounded_rect(0.0, 0.0, 30.0, 20.0, 2.0));
        d.board.cutouts.push(Contour::rect(20.0, 5.0, 4.0, 6.0));
        d.board.holes.push(Hole {
            id: "h1".into(),
            at: Point::new(5.0, 5.0),
            drill: Drill::Round { diameter: 3.2 },
            plated: false,
            pad_diameter: None,
            net: None,
            rotation: 0.0,
        });
        d.board.holes.push(Hole {
            id: "h2".into(),
            at: Point::new(10.0, 15.0),
            drill: Drill::Slot {
                width: 4.0,
                height: 1.0,
            },
            plated: false,
            pad_diameter: None,
            net: None,
            rotation: 0.0,
        });
        d
    }

    #[test]
    fn numbers_and_strings() {
        assert_eq!(r(1.0), "1.");
        assert_eq!(r(-0.0), "0.");
        assert_eq!(r(0.25), "0.25");
        assert_eq!(r(1e-13), "0.");
        assert_eq!(s("it's"), "'it''s'");
        assert_eq!(s("Ω5"), "'\\X2\\03A9\\X0\\5'");
    }

    #[test]
    fn renumbering_skips_strings_and_comments() {
        let src = "HEADER;ENDSEC;\nDATA;\n#1=PRODUCT('#9 not a ref','x','',(#2));\n/* #3 */\n#2=PRODUCT_CONTEXT('',#1,'m');\nENDSEC;\n";
        let (out, max) = renumber(src.as_bytes(), 100).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(max, 2);
        assert!(
            out.contains("#101=PRODUCT('#9 not a ref','x','',(#102));"),
            "{out}"
        );
        assert!(out.contains("/* #3 */"));
        assert!(out.contains("#102=PRODUCT_CONTEXT('',#101,'m');"));
    }

    #[test]
    fn renumbering_keeps_other_encodings() {
        // GBK bytes in a name, as EasyEDA writes them.
        let mut src = b"DATA;\n#7=PRODUCT('".to_vec();
        src.extend_from_slice(&[0xb6, 0xcb, 0xd7, 0xd3]);
        src.extend_from_slice(b"','',(#8));\nENDSEC;\n");
        let (out, max) = renumber(&src, 10).unwrap();
        assert_eq!(max, 8);
        let mut want = b"\n#17=PRODUCT('".to_vec();
        want.extend_from_slice(&[0xb6, 0xcb, 0xd7, 0xd3]);
        want.extend_from_slice(b"','',(#18));\n");
        assert_eq!(out, want);
    }

    #[test]
    fn board_solid_reads_back() {
        let d = board();
        let x = export_step(&d, Path::new("."), "t", &StepOptions::default()).unwrap();
        let mesh = lamina_step::step::load(&x.bytes).unwrap();
        let (lo, hi) = mesh.bounds().unwrap();
        assert!(
            lo[0].abs() < 1e-3 && lo[1].abs() < 1e-3 && lo[2].abs() < 1e-3,
            "{lo:?}"
        );
        assert!(
            (hi[0] - 30.0).abs() < 1e-3
                && (hi[1] - 20.0).abs() < 1e-3
                && (hi[2] - 1.6).abs() < 1e-3,
            "{hi:?}"
        );
        // The area of the faces that point straight up: the board minus its holes.
        let mut up = 0.0f64;
        let mut side = 0.0f64;
        for p in &mesh.parts {
            for (tri, n) in p.positions.chunks(3).zip(p.normals.chunks(3)) {
                let a = [
                    tri[1][0] - tri[0][0],
                    tri[1][1] - tri[0][1],
                    tri[1][2] - tri[0][2],
                ];
                let bb = [
                    tri[2][0] - tri[0][0],
                    tri[2][1] - tri[0][1],
                    tri[2][2] - tri[0][2],
                ];
                let area = 0.5
                    * ((a[1] * bb[2] - a[2] * bb[1]).powi(2)
                        + (a[2] * bb[0] - a[0] * bb[2]).powi(2)
                        + (a[0] * bb[1] - a[1] * bb[0]).powi(2))
                    .sqrt() as f64;
                if n[0][2] > 0.99 && tri[0][2] > 1.0 {
                    up += area;
                } else if n[0][2].abs() < 0.01 {
                    side += area;
                }
            }
        }
        let corners = 4.0 * (4.0 - std::f64::consts::PI);
        let hole = std::f64::consts::PI * 1.6 * 1.6;
        let slot = 3.0 * 1.0 + std::f64::consts::PI * 0.25;
        let want = 600.0 - corners - 24.0 - hole - slot;
        assert!((up - want).abs() < 0.5, "top area {up} vs {want}");
        // Walls: the outline perimeter and the walls of the cut-out, hole and slot.
        let perim = 2.0 * (26.0 + 16.0)
            + 2.0 * std::f64::consts::PI * 2.0
            + 20.0
            + std::f64::consts::PI * 3.2
            + 2.0 * 3.0
            + std::f64::consts::PI;
        assert!(
            (side - perim * 1.6).abs() < 1.0,
            "side area {side} vs {}",
            perim * 1.6
        );
        let text = String::from_utf8_lossy(&x.bytes);
        assert!(text.contains("MANIFOLD_SOLID_BREP('PCB'"));
        assert!(text.contains("NEXT_ASSEMBLY_USAGE_OCCURRENCE('NAUO0','PCB'"));
    }
}
