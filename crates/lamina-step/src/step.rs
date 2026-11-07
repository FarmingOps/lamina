//! Converts a STEP B-rep to triangles. It applies assembly transforms and
//! colours, and tessellates surfaces in parameter space.

use std::collections::HashMap;

use crate::p21::{Arg, Entity, parse};
use lamina_ports::mesh::{Mesh, MeshPart};

type V3 = [f64; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}
fn norm(a: V3) -> V3 {
    let l = len(a);
    if l < 1e-300 {
        [0.0, 0.0, 1.0]
    } else {
        mul(a, 1.0 / l)
    }
}
fn dist(a: V3, b: V3) -> f64 {
    len(sub(a, b))
}

const TAU: f64 = std::f64::consts::TAU;

/// Right-handed frame: origin and axes.
#[derive(Clone, Copy, Debug)]
struct Frame {
    o: V3,
    x: V3,
    y: V3,
    z: V3,
}

impl Frame {
    fn identity() -> Frame {
        Frame {
            o: [0.0; 3],
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
            z: [0.0, 0.0, 1.0],
        }
    }
    fn local(&self, p: V3) -> V3 {
        let d = sub(p, self.o);
        [dot(d, self.x), dot(d, self.y), dot(d, self.z)]
    }
    fn world(&self, l: V3) -> V3 {
        add(
            self.o,
            add(mul(self.x, l[0]), add(mul(self.y, l[1]), mul(self.z, l[2]))),
        )
    }
}

/// Affine transform as a frame (rotation + translation) with uniform scale.
#[derive(Clone, Copy, Debug)]
struct Xf {
    f: Frame,
}

impl Xf {
    fn id() -> Xf {
        Xf {
            f: Frame::identity(),
        }
    }
    fn apply(&self, p: V3) -> V3 {
        self.f.world(p)
    }
    fn apply_dir(&self, d: V3) -> V3 {
        add(
            mul(self.f.x, d[0]),
            add(mul(self.f.y, d[1]), mul(self.f.z, d[2])),
        )
    }
    /// self ∘ other
    fn then(&self, other: &Xf) -> Xf {
        Xf {
            f: Frame {
                o: self.apply(other.f.o),
                x: self.apply_dir(other.f.x),
                y: self.apply_dir(other.f.y),
                z: self.apply_dir(other.f.z),
            },
        }
    }
    fn inverse(&self) -> Xf {
        let f = &self.f;
        let x = [f.x[0], f.y[0], f.z[0]];
        let y = [f.x[1], f.y[1], f.z[1]];
        let z = [f.x[2], f.y[2], f.z[2]];
        let o = [-dot(f.o, f.x), -dot(f.o, f.y), -dot(f.o, f.z)];
        Xf {
            f: Frame { o, x, y, z },
        }
    }
}

struct Model<'a> {
    e: &'a HashMap<u32, Entity>,
    scale: f64,
}

#[derive(Clone)]
enum Curve {
    Line { p: V3, d: V3 },
    Circle { f: Frame, r: f64 },
    Ellipse { f: Frame, a: f64, b: f64 },
    Spline(BSplineCurve),
    Poly(Vec<V3>),
    Other,
}

#[derive(Clone)]
struct BSplineCurve {
    deg: usize,
    pts: Vec<V3>,
    w: Vec<f64>,
    knots: Vec<f64>,
}

#[derive(Clone)]
struct BSplineSurface {
    du: usize,
    dv: usize,
    pts: Vec<Vec<V3>>,
    w: Vec<Vec<f64>>,
    ku: Vec<f64>,
    kv: Vec<f64>,
}

#[derive(Clone)]
enum Surface {
    Plane(Frame),
    Cylinder(Frame, f64),
    Cone(Frame, f64, f64),
    Sphere(Frame, f64),
    Torus(Frame, f64, f64),
    Spline(BSplineSurface),
    Revolution(Box<Curve>, V3, V3),
    Extrusion(Box<Curve>, V3),
    Offset(Box<Surface>, f64),
}

fn find_span(k: &[f64], deg: usize, n: usize, t: f64) -> usize {
    if t >= k[n + 1] {
        return n;
    }
    if t <= k[deg] {
        return deg;
    }
    let (mut lo, mut hi) = (deg, n + 1);
    let mut mid = (lo + hi) / 2;
    while t < k[mid] || t >= k[mid + 1] {
        if t < k[mid] {
            hi = mid;
        } else {
            lo = mid;
        }
        mid = (lo + hi) / 2;
    }
    mid
}

fn basis(k: &[f64], deg: usize, span: usize, t: f64) -> Vec<f64> {
    let mut n = vec![0.0; deg + 1];
    let mut left = vec![0.0; deg + 1];
    let mut right = vec![0.0; deg + 1];
    n[0] = 1.0;
    for j in 1..=deg {
        left[j] = t - k[span + 1 - j];
        right[j] = k[span + j] - t;
        let mut saved = 0.0;
        for r in 0..j {
            let den = right[r + 1] + left[j - r];
            let tmp = if den.abs() < 1e-300 { 0.0 } else { n[r] / den };
            n[r] = saved + right[r + 1] * tmp;
            saved = left[j - r] * tmp;
        }
        n[j] = saved;
    }
    n
}

impl BSplineCurve {
    fn range(&self) -> (f64, f64) {
        (self.knots[self.deg], self.knots[self.pts.len()])
    }
    fn eval(&self, t: f64) -> V3 {
        let n = self.pts.len() - 1;
        let s = find_span(&self.knots, self.deg, n, t);
        let b = basis(&self.knots, self.deg, s, t);
        let mut p = [0.0; 3];
        let mut ws = 0.0;
        for (i, bi) in b.iter().enumerate() {
            let idx = s - self.deg + i;
            let w = self.w[idx] * bi;
            p = add(p, mul(self.pts[idx], w));
            ws += w;
        }
        if ws.abs() < 1e-300 {
            p
        } else {
            mul(p, 1.0 / ws)
        }
    }
}

impl BSplineSurface {
    fn range(&self) -> (f64, f64, f64, f64) {
        (
            self.ku[self.du],
            self.ku[self.pts.len()],
            self.kv[self.dv],
            self.kv[self.pts[0].len()],
        )
    }
    fn eval(&self, u: f64, v: f64) -> V3 {
        let nu = self.pts.len() - 1;
        let nv = self.pts[0].len() - 1;
        let su = find_span(&self.ku, self.du, nu, u);
        let sv = find_span(&self.kv, self.dv, nv, v);
        let bu = basis(&self.ku, self.du, su, u);
        let bv = basis(&self.kv, self.dv, sv, v);
        let mut p = [0.0; 3];
        let mut ws = 0.0;
        for (i, a) in bu.iter().enumerate() {
            for (j, b) in bv.iter().enumerate() {
                let ii = su - self.du + i;
                let jj = sv - self.dv + j;
                let w = self.w[ii][jj] * a * b;
                p = add(p, mul(self.pts[ii][jj], w));
                ws += w;
            }
        }
        if ws.abs() < 1e-300 {
            p
        } else {
            mul(p, 1.0 / ws)
        }
    }
}

fn expand_knots(mults: &[Arg], knots: &[Arg]) -> Vec<f64> {
    let mut out = Vec::new();
    for (m, k) in mults.iter().zip(knots) {
        let m = m.num().unwrap_or(1.0) as usize;
        let k = k.num().unwrap_or(0.0);
        for _ in 0..m {
            out.push(k);
        }
    }
    out
}

impl Curve {
    fn eval(&self, t: f64) -> V3 {
        match self {
            Curve::Line { p, d } => add(*p, mul(*d, t)),
            Curve::Circle { f, r } => f.world([r * t.cos(), r * t.sin(), 0.0]),
            Curve::Ellipse { f, a, b } => f.world([a * t.cos(), b * t.sin(), 0.0]),
            Curve::Spline(s) => s.eval(t),
            Curve::Poly(p) => {
                let i = (t.floor().max(0.0) as usize).min(p.len().saturating_sub(2));
                let k = t - i as f64;
                add(mul(p[i], 1.0 - k), mul(p[(i + 1).min(p.len() - 1)], k))
            }
            Curve::Other => [0.0; 3],
        }
    }

    fn param(&self, p: V3) -> f64 {
        match self {
            Curve::Line { p: o, d } => dot(sub(p, *o), *d) / dot(*d, *d).max(1e-300),
            Curve::Circle { f, .. } => {
                let l = f.local(p);
                l[1].atan2(l[0])
            }
            Curve::Ellipse { f, a, b } => {
                let l = f.local(p);
                (l[1] / b).atan2(l[0] / a)
            }
            Curve::Spline(s) => {
                let (t0, t1) = s.range();
                closest_param(|t| s.eval(t), t0, t1, p)
            }
            Curve::Poly(pts) => closest_param(|t| self.eval(t), 0.0, (pts.len() - 1) as f64, p),
            Curve::Other => 0.0,
        }
    }
}

fn closest_param(f: impl Fn(f64) -> V3, t0: f64, t1: f64, p: V3) -> f64 {
    let n = 64;
    let mut best = (f64::INFINITY, t0);
    for i in 0..=n {
        let t = t0 + (t1 - t0) * i as f64 / n as f64;
        let d = dist(f(t), p);
        if d < best.0 {
            best = (d, t);
        }
    }
    let mut step = (t1 - t0) / n as f64;
    let mut t = best.1;
    for _ in 0..40 {
        step *= 0.5;
        for c in [t - step, t + step] {
            if c >= t0 && c <= t1 {
                let d = dist(f(c), p);
                if d < best.0 {
                    best = (d, c);
                }
            }
        }
        t = best.1;
    }
    t
}

impl Surface {
    fn eval(&self, u: f64, v: f64) -> V3 {
        match self {
            Surface::Plane(f) => f.world([u, v, 0.0]),
            Surface::Cylinder(f, r) => f.world([r * u.cos(), r * u.sin(), v]),
            Surface::Cone(f, r, a) => {
                let rr = r + v * a.tan();
                f.world([rr * u.cos(), rr * u.sin(), v])
            }
            Surface::Sphere(f, r) => {
                f.world([r * v.cos() * u.cos(), r * v.cos() * u.sin(), r * v.sin()])
            }
            Surface::Torus(f, big, small) => {
                let rr = big + small * v.cos();
                f.world([rr * u.cos(), rr * u.sin(), small * v.sin()])
            }
            Surface::Spline(s) => s.eval(u, v),
            Surface::Revolution(c, o, ax) => {
                let p = c.eval(v);
                rotate_about(p, *o, *ax, u)
            }
            Surface::Extrusion(c, d) => add(c.eval(u), mul(*d, v)),
            Surface::Offset(s, dd) => {
                let p = s.eval(u, v);
                add(p, mul(s.normal(u, v), *dd))
            }
        }
    }

    fn normal(&self, u: f64, v: f64) -> V3 {
        let h = 1e-5 * self.param_scale();
        let du = sub(self.eval(u + h, v), self.eval(u - h, v));
        let dv = sub(self.eval(u, v + h), self.eval(u, v - h));
        let n = cross(du, dv);
        if len(n) < 1e-14 {
            // At a degenerate point (a pole), move a small distance inward.
            let du = sub(
                self.eval(u + 10.0 * h, v + 10.0 * h),
                self.eval(u - 10.0 * h, v + 10.0 * h),
            );
            let dv = sub(self.eval(u, v + 20.0 * h), self.eval(u, v));
            return norm(cross(du, dv));
        }
        norm(n)
    }

    fn param_scale(&self) -> f64 {
        match self {
            Surface::Spline(s) => {
                let (a, b, c, d) = s.range();
                (b - a).abs().max((d - c).abs()).max(1e-9)
            }
            _ => 1.0,
        }
    }

    /// The parameters of a point on the surface. The caller unwraps the `u`
    /// angles. If `hint` is given, the search starts from it.
    fn uv_near(&self, p: V3, hint: Option<(f64, f64)>) -> (f64, f64) {
        if let (Surface::Spline(s), Some(h)) = (self, hint) {
            let (u0, u1, v0, v1) = s.range();
            let (u, v, d) = newton(|u, v| s.eval(u, v), h.0, h.1, u0, u1, v0, v1, p);
            if d < 1e-4 {
                return (u, v);
            }
        }
        self.uv(p)
    }

    fn uv(&self, p: V3) -> (f64, f64) {
        match self {
            Surface::Plane(f) => {
                let l = f.local(p);
                (l[0], l[1])
            }
            Surface::Cylinder(f, _) | Surface::Cone(f, _, _) => {
                let l = f.local(p);
                (l[1].atan2(l[0]), l[2])
            }
            Surface::Sphere(f, r) => {
                let l = f.local(p);
                let rr = (l[0] * l[0] + l[1] * l[1]).sqrt();
                let _ = r;
                (l[1].atan2(l[0]), l[2].atan2(rr))
            }
            Surface::Torus(f, big, _) => {
                let l = f.local(p);
                let u = l[1].atan2(l[0]);
                let rr = (l[0] * l[0] + l[1] * l[1]).sqrt() - big;
                (u, l[2].atan2(rr))
            }
            Surface::Spline(s) => {
                let (u0, u1, v0, v1) = s.range();
                invert_grid(|u, v| s.eval(u, v), u0, u1, v0, v1, p)
            }
            Surface::Revolution(c, o, ax) => {
                // Get the profile parameter from the radial distance and the height.
                let d = sub(p, *o);
                let h = dot(d, *ax);
                let rho = len(sub(d, mul(*ax, h)));
                let (t0, t1) = curve_range(c);
                let rh = |q: V3| {
                    let dq = sub(q, *o);
                    let hq = dot(dq, *ax);
                    (len(sub(dq, mul(*ax, hq))), hq)
                };
                let t = closest_param(
                    |t| {
                        let (r, hh) = rh(c.eval(t));
                        [r, hh, 0.0]
                    },
                    t0,
                    t1,
                    [rho, h, 0.0],
                );
                // The angle around the axis between the profile point and p.
                let q = c.eval(t);
                let a = radial(sub(q, *o), *ax);
                let b = radial(d, *ax);
                let ang = dot(cross(a, b), *ax).atan2(dot(a, b));
                (ang, t)
            }
            Surface::Extrusion(c, d) => {
                let dn = norm(*d);
                let (t0, t1) = curve_range(c);
                let t = closest_param(
                    |t| {
                        let q = c.eval(t);
                        sub(q, mul(dn, dot(q, dn)))
                    },
                    t0,
                    t1,
                    sub(p, mul(dn, dot(p, dn))),
                );
                let v = dot(sub(p, c.eval(t)), *d) / dot(*d, *d).max(1e-300);
                (t, v)
            }
            Surface::Offset(s, _) => s.uv(p),
        }
    }

    /// Shows which parameters are angles (periodic with 2π).
    fn periodic(&self) -> (bool, bool) {
        match self {
            Surface::Cylinder(..)
            | Surface::Cone(..)
            | Surface::Sphere(..)
            | Surface::Revolution(..) => (true, false),
            Surface::Torus(..) => (true, true),
            Surface::Offset(s, _) => s.periodic(),
            _ => (false, false),
        }
    }

    /// Ruled or flat surfaces do not need interior refinement.
    fn flat_in_v(&self) -> bool {
        matches!(self, Surface::Plane(_))
    }
}

fn radial(d: V3, ax: V3) -> V3 {
    sub(d, mul(ax, dot(d, ax)))
}

fn rotate_about(p: V3, o: V3, ax: V3, ang: f64) -> V3 {
    let d = sub(p, o);
    let (s, c) = ang.sin_cos();
    let r = add(
        add(mul(d, c), mul(cross(ax, d), s)),
        mul(ax, dot(ax, d) * (1.0 - c)),
    );
    add(o, r)
}

fn curve_range(c: &Curve) -> (f64, f64) {
    match c {
        Curve::Spline(s) => s.range(),
        Curve::Poly(p) => (0.0, (p.len().max(2) - 1) as f64),
        Curve::Circle { .. } | Curve::Ellipse { .. } => (0.0, TAU),
        _ => (-1000.0, 1000.0),
    }
}

/// Gauss-Newton projection onto a parametric surface from a start point.
fn newton(
    f: impl Fn(f64, f64) -> V3,
    mut u: f64,
    mut v: f64,
    u0: f64,
    u1: f64,
    v0: f64,
    v1: f64,
    p: V3,
) -> (f64, f64, f64) {
    let hu = (u1 - u0).abs().max(1e-9) * 1e-6;
    let hv = (v1 - v0).abs().max(1e-9) * 1e-6;
    let mut d = dist(f(u, v), p);
    for _ in 0..12 {
        let s = f(u, v);
        let su = mul(sub(f(u + hu, v), f(u - hu, v)), 0.5 / hu);
        let sv = mul(sub(f(u, v + hv), f(u, v - hv)), 0.5 / hv);
        let r = sub(p, s);
        let (a, b, c) = (dot(su, su), dot(su, sv), dot(sv, sv));
        let (g1, g2) = (dot(su, r), dot(sv, r));
        let det = a * c - b * b;
        if det.abs() < 1e-30 {
            break;
        }
        let du = (c * g1 - b * g2) / det;
        let dv = (a * g2 - b * g1) / det;
        let nu = (u + du).clamp(u0.min(u1), u0.max(u1));
        let nv = (v + dv).clamp(v0.min(v1), v0.max(v1));
        let nd = dist(f(nu, nv), p);
        if nd > d {
            break;
        }
        u = nu;
        v = nv;
        d = nd;
        if d < 1e-7 {
            break;
        }
    }
    (u, v, d)
}

fn invert_grid(
    f: impl Fn(f64, f64) -> V3,
    u0: f64,
    u1: f64,
    v0: f64,
    v1: f64,
    p: V3,
) -> (f64, f64) {
    let n = 16;
    let mut best = (f64::INFINITY, u0, v0);
    for i in 0..=n {
        for j in 0..=n {
            let u = u0 + (u1 - u0) * i as f64 / n as f64;
            let v = v0 + (v1 - v0) * j as f64 / n as f64;
            let d = dist(f(u, v), p);
            if d < best.0 {
                best = (d, u, v);
            }
        }
    }
    let (mut u, mut v) = (best.1, best.2);
    let mut su = (u1 - u0) / n as f64;
    let mut sv = (v1 - v0) / n as f64;
    let mut bd = best.0;
    for _ in 0..48 {
        su *= 0.6;
        sv *= 0.6;
        let mut improved = true;
        while improved {
            improved = false;
            for (du, dv) in [
                (su, 0.0),
                (-su, 0.0),
                (0.0, sv),
                (0.0, -sv),
                (su, sv),
                (-su, -sv),
                (su, -sv),
                (-su, sv),
            ] {
                let (cu, cv) = ((u + du).clamp(u0, u1), (v + dv).clamp(v0, v1));
                let d = dist(f(cu, cv), p);
                if d < bd {
                    bd = d;
                    u = cu;
                    v = cv;
                    improved = true;
                }
            }
        }
    }
    (u, v)
}

impl<'a> Model<'a> {
    fn get(&self, id: u32) -> Option<&'a Entity> {
        self.e.get(&id)
    }

    fn point(&self, id: u32) -> Option<V3> {
        let e = self.get(id)?;
        match e.kind() {
            "CARTESIAN_POINT" => {
                let c = e.args().get(1)?.list();
                let g = |i: usize| c.get(i).and_then(|x| x.num()).unwrap_or(0.0) * self.scale;
                Some([g(0), g(1), g(2)])
            }
            "VERTEX_POINT" => self.point(e.args().get(1)?.id()?),
            _ => None,
        }
    }

    fn dir(&self, id: u32) -> Option<V3> {
        let e = self.get(id)?;
        match e.kind() {
            "DIRECTION" => {
                let c = e.args().get(1)?.list();
                let g = |i: usize| c.get(i).and_then(|x| x.num()).unwrap_or(0.0);
                Some(norm([g(0), g(1), g(2)]))
            }
            "VECTOR" => {
                let d = self.dir(e.args().get(1)?.id()?)?;
                let m = e.args().get(2)?.num()? * self.scale;
                Some(mul(d, m))
            }
            _ => None,
        }
    }

    fn frame(&self, id: u32) -> Option<Frame> {
        let e = self.get(id)?;
        let a = e.args();
        let o = self.point(a.get(1)?.id()?)?;
        match e.kind() {
            "AXIS2_PLACEMENT_3D" => {
                let z = a
                    .get(2)
                    .and_then(|x| x.id())
                    .and_then(|i| self.dir(i))
                    .unwrap_or([0.0, 0.0, 1.0]);
                let xr = a
                    .get(3)
                    .and_then(|x| x.id())
                    .and_then(|i| self.dir(i))
                    .unwrap_or_else(|| {
                        if z[0].abs() < 0.9 {
                            [1.0, 0.0, 0.0]
                        } else {
                            [0.0, 1.0, 0.0]
                        }
                    });
                let x = norm(sub(xr, mul(z, dot(xr, z))));
                Some(Frame {
                    o,
                    x,
                    y: cross(z, x),
                    z,
                })
            }
            "AXIS1_PLACEMENT" => {
                let z = a
                    .get(2)
                    .and_then(|x| x.id())
                    .and_then(|i| self.dir(i))
                    .unwrap_or([0.0, 0.0, 1.0]);
                let xr = if z[0].abs() < 0.9 {
                    [1.0, 0.0, 0.0]
                } else {
                    [0.0, 1.0, 0.0]
                };
                let x = norm(sub(xr, mul(z, dot(xr, z))));
                Some(Frame {
                    o,
                    x,
                    y: cross(z, x),
                    z,
                })
            }
            _ => None,
        }
    }

    fn curve(&self, id: u32) -> Curve {
        self.curve_opt(id).unwrap_or(Curve::Other)
    }

    fn curve_opt(&self, id: u32) -> Option<Curve> {
        let e = self.get(id)?;
        let a = e.args();
        Some(match e.kind() {
            "LINE" => Curve::Line {
                p: self.point(a.get(1)?.id()?)?,
                d: self.dir(a.get(2)?.id()?)?,
            },
            "CIRCLE" => Curve::Circle {
                f: self.frame(a.get(1)?.id()?)?,
                r: a.get(2)?.num()? * self.scale,
            },
            "ELLIPSE" => Curve::Ellipse {
                f: self.frame(a.get(1)?.id()?)?,
                a: a.get(2)?.num()? * self.scale,
                b: a.get(3)?.num()? * self.scale,
            },
            "SURFACE_CURVE" | "SEAM_CURVE" | "INTERSECTION_CURVE" => {
                self.curve_opt(a.get(1)?.id()?)?
            }
            "TRIMMED_CURVE" => self.curve_opt(a.get(1)?.id()?)?,
            "POLYLINE" => Curve::Poly(
                a.get(1)?
                    .list()
                    .iter()
                    .filter_map(|x| x.id())
                    .filter_map(|i| self.point(i))
                    .collect(),
            ),
            "B_SPLINE_CURVE_WITH_KNOTS" => {
                let deg = a.get(1)?.num()? as usize;
                let pts: Vec<V3> = a
                    .get(2)?
                    .list()
                    .iter()
                    .filter_map(|x| x.id())
                    .filter_map(|i| self.point(i))
                    .collect();
                let knots = expand_knots(a.get(6)?.list(), a.get(7)?.list());
                let n = pts.len();
                Curve::Spline(BSplineCurve {
                    deg,
                    pts,
                    w: vec![1.0; n],
                    knots,
                })
            }
            _ if e.is("B_SPLINE_CURVE") => {
                let bc = e.part("B_SPLINE_CURVE")?;
                let kn = e.part("B_SPLINE_CURVE_WITH_KNOTS")?;
                let deg = bc.first()?.num()? as usize;
                let pts: Vec<V3> = bc
                    .get(1)?
                    .list()
                    .iter()
                    .filter_map(|x| x.id())
                    .filter_map(|i| self.point(i))
                    .collect();
                let knots = expand_knots(kn.first()?.list(), kn.get(1)?.list());
                let w = e
                    .part("RATIONAL_B_SPLINE_CURVE")
                    .and_then(|r| r.first())
                    .map(|l| l.list().iter().map(|x| x.num().unwrap_or(1.0)).collect())
                    .unwrap_or_else(|| vec![1.0; pts.len()]);
                Curve::Spline(BSplineCurve { deg, pts, w, knots })
            }
            _ => return None,
        })
    }

    fn surface(&self, id: u32) -> Option<Surface> {
        let e = self.get(id)?;
        let a = e.args();
        Some(match e.kind() {
            "PLANE" => Surface::Plane(self.frame(a.get(1)?.id()?)?),
            "CYLINDRICAL_SURFACE" => {
                Surface::Cylinder(self.frame(a.get(1)?.id()?)?, a.get(2)?.num()? * self.scale)
            }
            "CONICAL_SURFACE" => Surface::Cone(
                self.frame(a.get(1)?.id()?)?,
                a.get(2)?.num()? * self.scale,
                a.get(3)?.num()?.to_radians_if_degrees(),
            ),
            "SPHERICAL_SURFACE" => {
                Surface::Sphere(self.frame(a.get(1)?.id()?)?, a.get(2)?.num()? * self.scale)
            }
            "TOROIDAL_SURFACE" | "DEGENERATE_TOROIDAL_SURFACE" => Surface::Torus(
                self.frame(a.get(1)?.id()?)?,
                a.get(2)?.num()? * self.scale,
                a.get(3)?.num()? * self.scale,
            ),
            "SURFACE_OF_REVOLUTION" => {
                let c = self.curve_opt(a.get(1)?.id()?)?;
                let f = self.frame(a.get(2)?.id()?)?;
                Surface::Revolution(Box::new(c), f.o, f.z)
            }
            "SURFACE_OF_LINEAR_EXTRUSION" => {
                let c = self.curve_opt(a.get(1)?.id()?)?;
                Surface::Extrusion(Box::new(c), self.dir(a.get(2)?.id()?)?)
            }
            "OFFSET_SURFACE" => Surface::Offset(
                Box::new(self.surface(a.get(1)?.id()?)?),
                a.get(2)?.num()? * self.scale,
            ),
            "B_SPLINE_SURFACE_WITH_KNOTS" => {
                let pts = self.grid(a.get(3)?)?;
                let w = pts.iter().map(|r| vec![1.0; r.len()]).collect();
                Surface::Spline(BSplineSurface {
                    du: a.get(1)?.num()? as usize,
                    dv: a.get(2)?.num()? as usize,
                    w,
                    ku: expand_knots(a.get(8)?.list(), a.get(10)?.list()),
                    kv: expand_knots(a.get(9)?.list(), a.get(11)?.list()),
                    pts,
                })
            }
            _ if e.is("B_SPLINE_SURFACE") => {
                let bs = e.part("B_SPLINE_SURFACE")?;
                let kn = e.part("B_SPLINE_SURFACE_WITH_KNOTS")?;
                let pts = self.grid(bs.get(2)?)?;
                let w: Vec<Vec<f64>> = e
                    .part("RATIONAL_B_SPLINE_SURFACE")
                    .and_then(|r| r.first())
                    .map(|l| {
                        l.list()
                            .iter()
                            .map(|row| row.list().iter().map(|x| x.num().unwrap_or(1.0)).collect())
                            .collect()
                    })
                    .unwrap_or_else(|| pts.iter().map(|r| vec![1.0; r.len()]).collect());
                Surface::Spline(BSplineSurface {
                    du: bs.first()?.num()? as usize,
                    dv: bs.get(1)?.num()? as usize,
                    w,
                    ku: expand_knots(kn.first()?.list(), kn.get(2)?.list()),
                    kv: expand_knots(kn.get(1)?.list(), kn.get(3)?.list()),
                    pts,
                })
            }
            _ => return None,
        })
    }

    fn grid(&self, a: &Arg) -> Option<Vec<Vec<V3>>> {
        let rows: Vec<Vec<V3>> = a
            .list()
            .iter()
            .map(|r| {
                r.list()
                    .iter()
                    .filter_map(|x| x.id())
                    .filter_map(|i| self.point(i))
                    .collect()
            })
            .collect();
        (!rows.is_empty()
            && rows
                .iter()
                .all(|r| r.len() == rows[0].len() && !r.is_empty()))
        .then_some(rows)
    }
}

trait Angle {
    fn to_radians_if_degrees(self) -> f64;
}
impl Angle for f64 {
    fn to_radians_if_degrees(self) -> f64 {
        // Files store plane angles in the unit of the context. Use radians,
        // but if the value is clearly in degrees, convert it.
        if self.abs() > TAU {
            self.to_radians()
        } else {
            self
        }
    }
}

/// Divide one edge into points, from its start vertex to its end vertex.
fn edge_points(m: &Model, curve: &Curve, a: V3, b: V3, same_sense: bool, tol: f64) -> Vec<V3> {
    let closed = dist(a, b) < 1e-7;
    match curve {
        Curve::Line { .. } | Curve::Other => vec![a, b],
        Curve::Circle { r, .. } | Curve::Ellipse { a: r, .. } => {
            let ta = curve.param(a);
            let tb = curve.param(b);
            let mut sweep = if same_sense {
                (tb - ta).rem_euclid(TAU)
            } else {
                -((ta - tb).rem_euclid(TAU))
            };
            if closed || sweep.abs() < 1e-9 {
                sweep = if same_sense { TAU } else { -TAU };
            }
            let step =
                (2.0 * (1.0 - (tol / r.max(tol * 2.0))).clamp(-1.0, 1.0).acos()).clamp(0.02, 0.35);
            let n = ((sweep.abs() / step).ceil() as usize).clamp(2, 512);
            let mut out = Vec::with_capacity(n + 1);
            out.push(a);
            for i in 1..n {
                out.push(curve.eval(ta + sweep * i as f64 / n as f64));
            }
            out.push(b);
            out
        }
        Curve::Spline(s) => {
            let (t0, t1) = s.range();
            let (ta, tb) = if closed {
                if same_sense { (t0, t1) } else { (t1, t0) }
            } else {
                (curve.param(a), curve.param(b))
            };
            let spans = s.knots.windows(2).filter(|w| w[1] > w[0]).count().max(1);
            let n = (spans * 8 * s.deg.max(1)).clamp(4, 400);
            let mut out = Vec::with_capacity(n + 1);
            out.push(a);
            for i in 1..n {
                out.push(s.eval(ta + (tb - ta) * i as f64 / n as f64));
            }
            out.push(b);
            let _ = m;
            out
        }
        Curve::Poly(p) => {
            let mut v = p.clone();
            if !same_sense {
                v.reverse();
            }
            v
        }
    }
}

struct Ctx<'a> {
    m: Model<'a>,
    edges: HashMap<u32, Vec<V3>>,
    colors: HashMap<u32, [f32; 3]>,
    tol: f64,
    skipped: usize,
}

impl<'a> Ctx<'a> {
    /// The points of an EDGE_CURVE in its own direction. The result is cached.
    fn edge(&mut self, id: u32) -> Option<Vec<V3>> {
        if let Some(v) = self.edges.get(&id) {
            return Some(v.clone());
        }
        let e = self.m.get(id)?;
        let a = e.args();
        let v1 = self.m.point(a.get(1)?.id()?)?;
        let v2 = self.m.point(a.get(2)?.id()?)?;
        let curve = self.m.curve(a.get(3)?.id()?);
        let same = a.get(4).and_then(|x| x.boolean()).unwrap_or(true);
        let pts = edge_points(&self.m, &curve, v1, v2, same, self.tol);
        self.edges.insert(id, pts.clone());
        Some(pts)
    }

    fn loop_points(&mut self, id: u32) -> Option<Vec<V3>> {
        let e = self.m.get(id)?;
        match e.kind() {
            "EDGE_LOOP" => {
                let mut out: Vec<V3> = Vec::new();
                for oe in e.args().get(1)?.list() {
                    let oe = self.m.get(oe.id()?)?;
                    let a = oe.args();
                    let ec = a.get(3)?.id()?;
                    let orient = a.get(4).and_then(|x| x.boolean()).unwrap_or(true);
                    let mut pts = self.edge(ec)?;
                    if !orient {
                        pts.reverse();
                    }
                    if let Some(last) = out.last() {
                        if pts.first().is_some_and(|p| dist(*p, *last) < 1e-6) {
                            pts.remove(0);
                        }
                    }
                    out.extend(pts);
                }
                if out.len() > 1 && dist(out[0], *out.last().unwrap()) < 1e-6 {
                    out.pop();
                }
                Some(out)
            }
            "POLY_LOOP" => Some(
                e.args()
                    .get(1)?
                    .list()
                    .iter()
                    .filter_map(|x| x.id())
                    .filter_map(|i| self.m.point(i))
                    .collect(),
            ),
            _ => None,
        }
    }

    fn face(&mut self, id: u32, xf: &Xf, color: [f32; 3], out: &mut MeshPart) {
        if self.try_face(id, xf, color, out).is_none() {
            self.skipped += 1;
        }
    }

    fn try_face(&mut self, id: u32, xf: &Xf, color: [f32; 3], out: &mut MeshPart) -> Option<()> {
        let e = self.m.get(id)?;
        let a = e.args();
        let same_sense = a.get(3).and_then(|x| x.boolean()).unwrap_or(true);
        let surface = a
            .get(2)
            .and_then(|x| x.id())
            .and_then(|s| self.m.surface(s));
        let mut loops: Vec<Vec<V3>> = Vec::new();
        let mut outer_idx = 0;
        for (k, b) in a.get(1)?.list().iter().enumerate() {
            let be = self.m.get(b.id()?)?;
            if be.kind() == "FACE_OUTER_BOUND" {
                outer_idx = k;
            }
            let orient = be.args().get(2).and_then(|x| x.boolean()).unwrap_or(true);
            let mut pts = self.loop_points(be.args().get(1)?.id()?)?;
            if !orient {
                pts.reverse();
            }
            if pts.len() >= 2 {
                loops.push(pts);
            }
        }
        if loops.is_empty() {
            return None;
        }
        if outer_idx < loops.len() {
            let o = loops.remove(outer_idx);
            loops.insert(0, o);
        }
        let _ = color;
        let tris = match &surface {
            Some(s) => tessellate(s, &loops, self.tol).or_else(|| planar_fallback(&loops))?,
            None => planar_fallback(&loops)?,
        };
        for (p, n) in tris {
            let mut tri_p = [[0f32; 3]; 3];
            let mut tri_n = [[0f32; 3]; 3];
            for k in 0..3 {
                let w = xf.apply(p[k]);
                let mut nn = norm(xf.apply_dir(n[k]));
                if !same_sense {
                    nn = mul(nn, -1.0);
                }
                tri_p[k] = [w[0] as f32, w[1] as f32, w[2] as f32];
                tri_n[k] = [nn[0] as f32, nn[1] as f32, nn[2] as f32];
            }
            out.push_smooth(tri_p, tri_n);
        }
        Some(())
    }
}

type Tri = ([V3; 3], [V3; 3]);

/// Triangulate a 2D polygon with holes. Returns index triples.
fn earcut2(rings: &[Vec<[f64; 2]>]) -> Option<(Vec<[f64; 2]>, Vec<usize>)> {
    let mut coords = Vec::new();
    let mut holes = Vec::new();
    for (k, r) in rings.iter().enumerate() {
        if k > 0 {
            holes.push(coords.len() / 2);
        }
        for p in r {
            coords.push(p[0]);
            coords.push(p[1]);
        }
    }
    let idx = earcutr::earcut(&coords, &holes, 2).ok()?;
    if idx.is_empty() {
        return None;
    }
    let pts = coords.chunks(2).map(|c| [c[0], c[1]]).collect();
    Some((pts, idx))
}

fn tessellate(s: &Surface, loops: &[Vec<V3>], tol: f64) -> Option<Vec<Tri>> {
    let (pu, pv) = s.periodic();
    // Map the loops to parameter space, and unwrap periodic coordinates.
    let mut uv_loops: Vec<Vec<[f64; 2]>> = Vec::new();
    for l in loops {
        let mut out: Vec<[f64; 2]> = Vec::with_capacity(l.len());
        for p in l {
            let hint = out.last().map(|q| (q[0], q[1]));
            let (mut u, mut v) = s.uv_near(*p, hint);
            if let Some(prev) = out.last() {
                if pu {
                    while u - prev[0] > std::f64::consts::PI {
                        u -= TAU;
                    }
                    while prev[0] - u > std::f64::consts::PI {
                        u += TAU;
                    }
                }
                if pv {
                    while v - prev[1] > std::f64::consts::PI {
                        v -= TAU;
                    }
                    while prev[1] - v > std::f64::consts::PI {
                        v += TAU;
                    }
                }
            }
            out.push([u, v]);
        }
        uv_loops.push(out);
    }
    // If a loop goes around a periodic surface and does not close in u, it
    // cannot be triangulated in uv. An example is a full cylinder band with no
    // seam edge.
    for l in &uv_loops {
        if pu && (l[0][0] - l[l.len() - 1][0]).abs() > 1.0 {
            return None;
        }
    }
    // Move the inner loops by whole periods into the outer loop.
    if pu && uv_loops.len() > 1 {
        let (lo, hi) = uv_loops[0]
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |a, p| {
                (a.0.min(p[0]), a.1.max(p[0]))
            });
        for l in uv_loops.iter_mut().skip(1) {
            let c = l.iter().map(|p| p[0]).sum::<f64>() / l.len() as f64;
            let mut shift = 0.0;
            while c + shift < lo {
                shift += TAU;
            }
            while c + shift > hi {
                shift -= TAU;
            }
            for p in l.iter_mut() {
                p[0] += shift;
            }
        }
    }
    // Multiply u by a radius, so that earcut gets correct proportions.
    let ku = match s {
        Surface::Cylinder(_, r) | Surface::Sphere(_, r) | Surface::Cone(_, r, _) => r.max(0.1),
        Surface::Torus(_, big, _) => big.max(0.1),
        _ => 1.0,
    };
    let scaled: Vec<Vec<[f64; 2]>> = uv_loops
        .iter()
        .map(|l| l.iter().map(|p| [p[0] * ku, p[1]]).collect())
        .collect();
    let (pts, idx) = earcut2(&scaled)?;
    let uv: Vec<[f64; 2]> = pts.iter().map(|p| [p[0] / ku, p[1]]).collect();
    // Do not split boundary edges. Then adjacent faces stay joined.
    let mut boundary = std::collections::HashSet::new();
    let mut base = 0;
    for l in &uv_loops {
        let n = l.len();
        for i in 0..n {
            let (a, b) = (base + i, base + (i + 1) % n);
            boundary.insert((a.min(b), a.max(b)));
        }
        base += n;
    }
    let mut verts: Vec<[f64; 2]> = uv;
    let mut tris: Vec<[usize; 3]> = idx.chunks(3).map(|t| [t[0], t[1], t[2]]).collect();
    if !s.flat_in_v() {
        let mut mids: HashMap<(usize, usize), Option<usize>> = HashMap::new();
        for _ in 0..16 {
            let mut next = Vec::with_capacity(tris.len() * 2);
            let mut changed = false;
            for t in tris {
                // Split the edge with the largest error, if it is more than tol from the surface.
                let mut worst = None;
                let mut worst_err = tol;
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    let key = (a.min(b), a.max(b));
                    if boundary.contains(&key) {
                        continue;
                    }
                    let m = [
                        (verts[a][0] + verts[b][0]) / 2.0,
                        (verts[a][1] + verts[b][1]) / 2.0,
                    ];
                    let pa = s.eval(verts[a][0], verts[a][1]);
                    let pb = s.eval(verts[b][0], verts[b][1]);
                    let pm = s.eval(m[0], m[1]);
                    let err = dist(pm, mul(add(pa, pb), 0.5));
                    if err > worst_err {
                        worst_err = err;
                        worst = Some(k);
                    }
                }
                match worst {
                    None => next.push(t),
                    Some(k) => {
                        let (a, b) = (t[k], t[(k + 1) % 3]);
                        let c = t[(k + 2) % 3];
                        let key = (a.min(b), a.max(b));
                        let mi = *mids.entry(key).or_insert_with(|| {
                            verts.push([
                                (verts[a][0] + verts[b][0]) / 2.0,
                                (verts[a][1] + verts[b][1]) / 2.0,
                            ]);
                            Some(verts.len() - 1)
                        });
                        let mi = mi.unwrap();
                        next.push([a, mi, c]);
                        next.push([mi, b, c]);
                        changed = true;
                    }
                }
            }
            tris = next;
            if !changed || tris.len() > 60_000 {
                break;
            }
        }
    }
    let p3: Vec<V3> = verts.iter().map(|p| s.eval(p[0], p[1])).collect();
    let n3: Vec<V3> = verts.iter().map(|p| s.normal(p[0], p[1])).collect();
    // Check the parameter mapping. Boundary points must go back to their initial positions.
    let mut k = 0;
    for l in loops {
        for p in l {
            if dist(p3[k], *p) > (tol * 50.0).max(0.05) {
                return None;
            }
            k += 1;
        }
    }
    let mut out = Vec::with_capacity(tris.len());
    for t in tris {
        let (a, b, c) = (p3[t[0]], p3[t[1]], p3[t[2]]);
        let fn_ = cross(sub(b, a), sub(c, a));
        let mut tri = ([a, b, c], [n3[t[0]], n3[t[1]], n3[t[2]]]);
        // Wind triangles to agree with the surface normal.
        let avg = add(add(n3[t[0]], n3[t[1]]), n3[t[2]]);
        if dot(fn_, avg) < 0.0 {
            tri = ([a, c, b], [n3[t[0]], n3[t[2]], n3[t[1]]]);
        }
        out.push(tri);
    }
    Some(out)
}

/// Project the loops on their best-fit plane and triangulate them there.
fn planar_fallback(loops: &[Vec<V3>]) -> Option<Vec<Tri>> {
    let outer = &loops[0];
    let mut n = [0.0; 3];
    for i in 0..outer.len() {
        let (p, q) = (outer[i], outer[(i + 1) % outer.len()]);
        n[0] += (p[1] - q[1]) * (p[2] + q[2]);
        n[1] += (p[2] - q[2]) * (p[0] + q[0]);
        n[2] += (p[0] - q[0]) * (p[1] + q[1]);
    }
    let z = norm(n);
    let xr = if z[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let x = norm(sub(xr, mul(z, dot(xr, z))));
    let y = cross(z, x);
    let rings: Vec<Vec<[f64; 2]>> = loops
        .iter()
        .map(|l| l.iter().map(|p| [dot(*p, x), dot(*p, y)]).collect())
        .collect();
    let (_, idx) = earcut2(&rings)?;
    let all: Vec<V3> = loops.iter().flatten().copied().collect();
    Some(
        idx.chunks(3)
            .map(|t| {
                let (a, b, c) = (all[t[0]], all[t[1]], all[t[2]]);
                let fnn = norm(cross(sub(b, a), sub(c, a)));
                let fnn = if dot(fnn, z) < 0.0 {
                    mul(fnn, -1.0)
                } else {
                    fnn
                };
                ([a, b, c], [fnn, fnn, fnn])
            })
            .collect(),
    )
}

fn length_scale(e: &HashMap<u32, Entity>) -> f64 {
    for ent in e.values() {
        if ent.is("LENGTH_UNIT") {
            if let Some(si) = ent.part("SI_UNIT") {
                let prefix = si.first().and_then(|a| a.enum_name()).unwrap_or("");
                return match prefix {
                    "MILLI" => 1.0,
                    "CENTI" => 10.0,
                    "MICRO" => 0.001,
                    "KILO" => 1e6,
                    _ => 1000.0,
                };
            }
            if let Some(cb) = ent.part("CONVERSION_BASED_UNIT") {
                let name = cb
                    .first()
                    .and_then(|a| a.text())
                    .unwrap_or("")
                    .to_ascii_uppercase();
                if name.contains("INCH") {
                    return 25.4;
                }
                if name.contains("FOOT") {
                    return 304.8;
                }
            }
        }
    }
    1.0
}

/// The colours that STYLED_ITEM chains give: item id -> linear RGB.
fn colors(e: &HashMap<u32, Entity>) -> HashMap<u32, [f32; 3]> {
    fn find_rgb(e: &HashMap<u32, Entity>, a: &Arg, depth: usize) -> Option<[f32; 3]> {
        if depth > 12 {
            return None;
        }
        match a {
            Arg::Ref(r) => {
                let ent = e.get(r)?;
                if ent.kind() == "COLOUR_RGB" {
                    let g =
                        |i: usize| ent.args().get(i).and_then(|x| x.num()).unwrap_or(0.5) as f32;
                    return Some([g(1), g(2), g(3)]);
                }
                if ent.kind() == "DRAUGHTING_PRE_DEFINED_COLOUR" {
                    return match ent.args().first().and_then(|x| x.text()).unwrap_or("") {
                        "red" => Some([0.8, 0.1, 0.1]),
                        "green" => Some([0.1, 0.6, 0.2]),
                        "blue" => Some([0.1, 0.2, 0.8]),
                        "yellow" => Some([0.9, 0.8, 0.1]),
                        "black" => Some([0.05, 0.05, 0.05]),
                        "white" => Some([0.95, 0.95, 0.95]),
                        _ => None,
                    };
                }
                ent.parts
                    .iter()
                    .flat_map(|p| p.1.iter())
                    .find_map(|x| find_rgb(e, x, depth + 1))
            }
            Arg::List(v) | Arg::Typed(_, v) => v.iter().find_map(|x| find_rgb(e, x, depth + 1)),
            _ => None,
        }
    }
    let srgb = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let mut out = HashMap::new();
    for ent in e.values() {
        if ent.kind() == "STYLED_ITEM" || ent.kind() == "OVER_RIDING_STYLED_ITEM" {
            let a = ent.args();
            let (Some(styles), Some(item)) = (a.get(1), a.get(2).and_then(|x| x.id())) else {
                continue;
            };
            if let Some(c) = find_rgb(e, styles, 0) {
                out.insert(item, [srgb(c[0]), srgb(c[1]), srgb(c[2])]);
            }
        }
    }
    out
}

const DEFAULT_COLOR: [f32; 3] = [0.42, 0.42, 0.45];

fn placement_xf(m: &Model, id: u32) -> Xf {
    m.frame(id).map(|f| Xf { f }).unwrap_or_else(Xf::id)
}

/// Load a STEP file into a mesh.
pub fn load(bytes: &[u8]) -> Result<Mesh, String> {
    if !bytes.starts_with(b"ISO-10303-21") {
        return Err("this is not a STEP file".into());
    }
    let e = parse(bytes)?;
    let scale = length_scale(&e);
    let colors = colors(&e);
    let mut ctx = Ctx {
        m: Model { e: &e, scale },
        edges: HashMap::new(),
        colors,
        tol: 0.01,
        skipped: 0,
    };

    // The representation graph: groups of reps that are linked without a
    // transform, and child -> parent links with a transform.
    let mut ids: Vec<u32> = e.keys().copied().collect();
    ids.sort_unstable();
    // Only shape representations. Not style or relationship records.
    let is_rep = |k: &str| {
        k.ends_with("REPRESENTATION")
            && !k.contains("RELATIONSHIP")
            && !k.contains("PRESENTATION_REPRESENTATION")
            && !k.starts_with("DRAUGHTING")
            && k != "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION"
    };
    // Find which side of each transform relationship is the child. The
    // product structure gives this: CDSR -> NAUO (parent, child product
    // definitions) and SHAPE_DEFINITION_REPRESENTATION (product definition -> rep).
    let mut pd_rep: HashMap<u32, u32> = HashMap::new();
    for ent in e.values() {
        if ent.kind() == "SHAPE_DEFINITION_REPRESENTATION" {
            let a = ent.args();
            let (Some(pds), Some(rep)) = (
                a.first().and_then(|x| x.id()),
                a.get(1).and_then(|x| x.id()),
            ) else {
                continue;
            };
            if let Some(pd) = e
                .get(&pds)
                .and_then(|p| p.args().get(2))
                .and_then(|x| x.id())
            {
                pd_rep.insert(pd, rep);
            }
        }
    }
    let mut child_rep_of: HashMap<u32, u32> = HashMap::new();
    for ent in e.values() {
        if ent.kind() == "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION" {
            let a = ent.args();
            let (Some(rel), Some(pds)) = (
                a.first().and_then(|x| x.id()),
                a.get(1).and_then(|x| x.id()),
            ) else {
                continue;
            };
            let Some(nauo) = e
                .get(&pds)
                .and_then(|p| p.args().get(2))
                .and_then(|x| x.id())
                .and_then(|i| e.get(&i))
            else {
                continue;
            };
            if let Some(child) = nauo
                .args()
                .get(4)
                .and_then(|x| x.id())
                .and_then(|pd| pd_rep.get(&pd))
            {
                child_rep_of.insert(rel, *child);
            }
        }
    }
    let mut group: HashMap<u32, u32> = HashMap::new();
    fn root(g: &mut HashMap<u32, u32>, x: u32) -> u32 {
        let mut r = x;
        while let Some(&p) = g.get(&r) {
            if p == r {
                break;
            }
            r = p;
        }
        g.insert(x, r);
        r
    }
    let mut links: Vec<(u32, u32, Xf)> = Vec::new();
    for id in &ids {
        let ent = &e[id];
        if let Some(rr) = ent.part("REPRESENTATION_RELATIONSHIP") {
            let (Some(r1), Some(r2)) = (
                rr.get(2).and_then(|x| x.id()),
                rr.get(3).and_then(|x| x.id()),
            ) else {
                continue;
            };
            if let Some(t) = ent.part("REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION") {
                let Some(itd) = t.first().and_then(|x| x.id()).and_then(|i| e.get(&i)) else {
                    continue;
                };
                let a = itd.args();
                let t1 = a
                    .get(2)
                    .and_then(|x| x.id())
                    .map(|i| placement_xf(&ctx.m, i))
                    .unwrap_or_else(Xf::id);
                let t2 = a
                    .get(3)
                    .and_then(|x| x.id())
                    .map(|i| placement_xf(&ctx.m, i))
                    .unwrap_or_else(Xf::id);
                // By default, rep_1 goes into rep_2. But if the product
                // structure shows that rep_2 is the child, rep_2 goes into rep_1.
                if child_rep_of.get(id) == Some(&r2) {
                    links.push((r2, r1, t1.then(&t2.inverse())));
                } else {
                    links.push((r1, r2, t2.then(&t1.inverse())));
                }
            } else {
                let (a, b) = (root(&mut group, r1), root(&mut group, r2));
                group.insert(a, b);
            }
        } else if ent.kind() == "SHAPE_REPRESENTATION_RELATIONSHIP" {
            let a = ent.args();
            let (Some(r1), Some(r2)) =
                (a.get(2).and_then(|x| x.id()), a.get(3).and_then(|x| x.id()))
            else {
                continue;
            };
            let (x, y) = (root(&mut group, r1), root(&mut group, r2));
            group.insert(x, y);
        }
    }
    // A mapped item puts one representation in another.
    for id in &ids {
        let ent = &e[id];
        if !is_rep(ent.kind()) {
            continue;
        }
        for it in ent.args().get(1).map(|x| x.list()).unwrap_or(&[]) {
            let Some(mi) = it.id().and_then(|i| e.get(&i)) else {
                continue;
            };
            if mi.kind() != "MAPPED_ITEM" {
                continue;
            }
            let a = mi.args();
            let Some(map) = a.get(1).and_then(|x| x.id()).and_then(|i| e.get(&i)) else {
                continue;
            };
            let origin = map
                .args()
                .first()
                .and_then(|x| x.id())
                .map(|i| placement_xf(&ctx.m, i))
                .unwrap_or_else(Xf::id);
            let Some(child) = map.args().get(1).and_then(|x| x.id()) else {
                continue;
            };
            let target = a
                .get(2)
                .and_then(|x| x.id())
                .map(|i| placement_xf(&ctx.m, i))
                .unwrap_or_else(Xf::id);
            links.push((child, *id, target.then(&origin.inverse())));
        }
    }
    let reps: Vec<u32> = ids
        .iter()
        .copied()
        .filter(|i| is_rep(e[i].kind()))
        .collect();
    let mut members: HashMap<u32, Vec<u32>> = HashMap::new();
    for r in &reps {
        let g = root(&mut group, *r);
        members.entry(g).or_default().push(*r);
    }
    let mut children: HashMap<u32, Vec<(u32, Xf)>> = HashMap::new();
    let mut is_child = std::collections::HashSet::new();
    for (c, p, xf) in &links {
        let (gc, gp) = (root(&mut group, *c), root(&mut group, *p));
        children.entry(gp).or_default().push((gc, *xf));
        is_child.insert(gc);
    }

    let mut parts: HashMap<[u32; 3], MeshPart> = HashMap::new();
    let mut emitted = 0usize;
    let mut stack: Vec<(u32, Xf, usize)> = members
        .keys()
        .filter(|g| !is_child.contains(g))
        .map(|g| (*g, Xf::id(), 0))
        .collect();
    stack.sort_by_key(|s| s.0);
    while let Some((g, xf, depth)) = stack.pop() {
        if depth > 32 || emitted > 5000 {
            continue;
        }
        for rep in members.get(&g).cloned().unwrap_or_default() {
            for it in e[&rep].args().get(1).map(|x| x.list()).unwrap_or(&[]) {
                if let Some(i) = it.id() {
                    emitted += emit_solid(&mut ctx, i, &xf, &mut parts);
                }
            }
        }
        for (c, cx) in children.get(&g).cloned().unwrap_or_default() {
            stack.push((c, xf.then(&cx), depth + 1));
        }
    }
    if std::env::var("LAMINA_STEP_DEBUG").is_ok() {
        let roots: Vec<_> = members.keys().filter(|g| !is_child.contains(g)).collect();
        eprintln!(
            "reps {} groups {} links {} roots {:?} emitted {}",
            reps.len(),
            members.len(),
            links.len(),
            roots,
            emitted
        );
    }
    if emitted == 0 {
        // There is no usable representation graph. Use each solid as it is.
        for id in &ids {
            emit_solid(&mut ctx, *id, &Xf::id(), &mut parts);
        }
    }
    let mut mesh = Mesh {
        parts: parts
            .into_values()
            .filter(|p| !p.positions.is_empty())
            .collect(),
    };
    mesh.parts
        .sort_by_key(|p| std::cmp::Reverse(p.positions.len()));
    if mesh.parts.is_empty() {
        return Err(format!(
            "the file has no geometry that can be tessellated ({} faces failed)",
            ctx.skipped
        ));
    }
    Ok(mesh.compact())
}

fn key(c: [f32; 3]) -> [u32; 3] {
    [c[0].to_bits(), c[1].to_bits(), c[2].to_bits()]
}

/// Tessellate a solid or shell item. Returns 1 if it wrote one or more faces.
fn emit_solid(ctx: &mut Ctx, id: u32, xf: &Xf, parts: &mut HashMap<[u32; 3], MeshPart>) -> usize {
    let Some(ent) = ctx.m.get(id) else { return 0 };
    let shells: Vec<u32> = match ent.kind() {
        "MANIFOLD_SOLID_BREP" | "FACETED_BREP" => {
            ent.args().get(1).and_then(|x| x.id()).into_iter().collect()
        }
        "BREP_WITH_VOIDS" => {
            let mut v: Vec<u32> = ent.args().get(1).and_then(|x| x.id()).into_iter().collect();
            v.extend(
                ent.args()
                    .get(2)
                    .map(|x| x.list())
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(|x| x.id()),
            );
            v
        }
        "SHELL_BASED_SURFACE_MODEL" => ent
            .args()
            .get(1)
            .map(|x| x.list())
            .unwrap_or(&[])
            .iter()
            .filter_map(|x| x.id())
            .collect(),
        _ => return 0,
    };
    let solid_color = ctx.colors.get(&id).copied();
    let mut any = 0;
    for sh in shells {
        let Some(se) = ctx.m.get(sh) else { continue };
        let shell_color = ctx.colors.get(&sh).copied().or(solid_color);
        let faces: Vec<u32> = se
            .args()
            .get(1)
            .map(|x| x.list())
            .unwrap_or(&[])
            .iter()
            .filter_map(|x| x.id())
            .collect();
        for f in faces {
            let c = ctx
                .colors
                .get(&f)
                .copied()
                .or(shell_color)
                .unwrap_or(DEFAULT_COLOR);
            let part = parts.entry(key(c)).or_insert_with(|| MeshPart {
                color: c,
                ..Default::default()
            });
            ctx.face(f, xf, c, part);
            any = 1;
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE: &str = "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n\
#1=CARTESIAN_POINT('',(0.,0.,0.));\n#2=DIRECTION('',(0.,0.,1.));\n#3=DIRECTION('',(1.,0.,0.));\n\
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\n#5=CYLINDRICAL_SURFACE('',#4,2.);\n\
#10=CARTESIAN_POINT('',(2.,0.,0.));\n#11=CARTESIAN_POINT('',(2.,0.,5.));\n\
#12=VERTEX_POINT('',#10);\n#13=VERTEX_POINT('',#11);\n\
#14=CIRCLE('',#4,2.);\n#15=CARTESIAN_POINT('',(0.,0.,5.));\n#16=AXIS2_PLACEMENT_3D('',#15,#2,#3);\n#17=CIRCLE('',#16,2.);\n\
#18=VECTOR('',#2,1.);\n#19=LINE('',#10,#18);\n\
#20=EDGE_CURVE('',#12,#12,#14,.T.);\n#21=EDGE_CURVE('',#13,#13,#17,.T.);\n#22=EDGE_CURVE('',#12,#13,#19,.T.);\n\
#23=ORIENTED_EDGE('',*,*,#20,.T.);\n#24=ORIENTED_EDGE('',*,*,#22,.T.);\n#25=ORIENTED_EDGE('',*,*,#21,.F.);\n#26=ORIENTED_EDGE('',*,*,#22,.F.);\n\
#27=EDGE_LOOP('',(#23,#24,#25,#26));\n#28=FACE_OUTER_BOUND('',#27,.T.);\n#29=ADVANCED_FACE('',(#28),#5,.T.);\n\
#30=CLOSED_SHELL('',(#29));\n#31=MANIFOLD_SOLID_BREP('',#30);\n\
#40=COLOUR_RGB('',1.,0.,0.);\n#41=FILL_AREA_STYLE_COLOUR('',#40);\n#42=FILL_AREA_STYLE('',(#41));\n\
#43=SURFACE_STYLE_FILL_AREA(#42);\n#44=SURFACE_SIDE_STYLE('',(#43));\n#45=SURFACE_STYLE_USAGE(.BOTH.,#44);\n\
#46=PRESENTATION_STYLE_ASSIGNMENT((#45));\n#47=STYLED_ITEM('',(#46),#31);\n\
#50=( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );\nENDSEC;\nEND-ISO-10303-21;\n";

    #[test]
    fn assembly_placement_parent_first() {
        // The SolidWorks order: rep_1 is the parent and rep_2 is the child.
        // The product structure (NAUO) shows which is which.
        let asm = CUBE.replace(
            "ENDSEC;
END-ISO-10303-21;",
            "#60=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#31,#4),#99);
#61=CARTESIAN_POINT('',(10.,0.,0.));
#62=AXIS2_PLACEMENT_3D('',#61,#2,#3);
#63=SHAPE_REPRESENTATION('',(#62),#99);
#64=( REPRESENTATION_RELATIONSHIP('','',#63,#60) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#65) SHAPE_REPRESENTATION_RELATIONSHIP() );
#65=ITEM_DEFINED_TRANSFORMATION('','',#62,#4);
#70=PRODUCT_DEFINITION('p','',#98,#97);
#71=PRODUCT_DEFINITION('c','',#98,#97);
#72=PRODUCT_DEFINITION_SHAPE('','',#70);
#73=PRODUCT_DEFINITION_SHAPE('','',#71);
#74=SHAPE_DEFINITION_REPRESENTATION(#72,#63);
#75=SHAPE_DEFINITION_REPRESENTATION(#73,#60);
#76=NEXT_ASSEMBLY_USAGE_OCCURRENCE('','','',#70,#71,$);
#77=PRODUCT_DEFINITION_SHAPE('','',#76);
#78=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#64,#77);
ENDSEC;
END-ISO-10303-21;",
        );
        let m = load(asm.as_bytes()).unwrap();
        let (lo, hi) = m.bounds().unwrap();
        assert!(
            (lo[0] - 8.0).abs() < 0.05 && (hi[0] - 12.0).abs() < 0.05,
            "{lo:?} {hi:?}"
        );
    }

    #[test]
    fn cylinder_side_with_seam() {
        let m = load(CUBE.as_bytes()).unwrap();
        let (lo, hi) = m.bounds().unwrap();
        assert!(
            (lo[0] + 2.0).abs() < 0.02 && (hi[0] - 2.0).abs() < 0.02,
            "{lo:?} {hi:?}"
        );
        assert!((hi[2] - 5.0).abs() < 1e-6);
        assert_eq!(m.parts.len(), 1);
        assert!((m.parts[0].color[0] - 1.0).abs() < 1e-6);
        // The lateral area is about 2*pi*r*h.
        let mut area = 0.0;
        for t in m.parts[0].positions.chunks(3) {
            let a = [t[0][0] as f64, t[0][1] as f64, t[0][2] as f64];
            let b = [t[1][0] as f64, t[1][1] as f64, t[1][2] as f64];
            let c = [t[2][0] as f64, t[2][1] as f64, t[2][2] as f64];
            area += len(cross(sub(b, a), sub(c, a))) / 2.0;
        }
        assert!((area - TAU * 2.0 * 5.0).abs() < 0.5, "{area}");
    }
}
