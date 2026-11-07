//! CPU rendering with tiny-skia: PNG snapshots and board surface textures.

use lamina_model::{Point, Rect, Side};
use lamina_scene::theme::{self, Rgba};
use lamina_scene::{Geom, Scene2D, SceneLayer};
use tiny_skia::{FillRule, LineCap, LineJoin, Mask, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// Board millimetres to pixels: fit a box into an image, Y up.
#[derive(Debug, Clone, Copy)]
pub struct View {
    /// Pixels per mm.
    pub scale: f32,
    /// Mirror horizontally (bottom side seen from below).
    pub mirror: bool,
    pub tx: f32,
    pub ty: f32,
    pub width: u32,
    pub height: u32,
}

impl View {
    /// Fit `r` into `width` x `height` with `margin` pixels around it.
    pub fn fit(r: Rect, width: u32, height: u32, margin: f32) -> View {
        let w = (r.width() as f32).max(1e-3);
        let h = (r.height() as f32).max(1e-3);
        let s = ((width as f32 - 2.0 * margin) / w).min((height as f32 - 2.0 * margin) / h);
        let cx = (r.min.x + r.max.x) as f32 / 2.0;
        let cy = (r.min.y + r.max.y) as f32 / 2.0;
        View {
            scale: s,
            mirror: false,
            tx: width as f32 / 2.0 - cx * s,
            ty: height as f32 / 2.0 + cy * s,
            width,
            height,
        }
    }

    /// Exact mapping of `r` onto the whole image (for textures).
    pub fn exact(r: Rect, width: u32, height: u32) -> View {
        let s = width as f32 / r.width() as f32;
        View {
            scale: s,
            mirror: false,
            tx: -(r.min.x as f32) * s,
            ty: r.max.y as f32 * s,
            width,
            height: height.max(1),
        }
    }

    pub fn transform(&self) -> Transform {
        if self.mirror {
            Transform::from_row(
                -self.scale,
                0.0,
                0.0,
                -self.scale,
                self.width as f32 - self.tx,
                self.ty,
            )
        } else {
            Transform::from_row(self.scale, 0.0, 0.0, -self.scale, self.tx, self.ty)
        }
    }

    pub fn to_px(&self, p: Point) -> (f32, f32) {
        let x = p.x as f32 * self.scale + self.tx;
        let x = if self.mirror {
            self.width as f32 - x
        } else {
            x
        };
        (x, self.ty - p.y as f32 * self.scale)
    }
}

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], c[3]);
    p.anti_alias = true;
    p
}

fn ring(pb: &mut PathBuilder, pts: &[Point]) {
    if let Some(first) = pts.first() {
        pb.move_to(first.x as f32, first.y as f32);
        for p in &pts[1..] {
            pb.line_to(p.x as f32, p.y as f32);
        }
        pb.close();
    }
}

/// Draw one shape in board coordinates.
pub fn draw_geom(pm: &mut Pixmap, g: &Geom, color: Rgba, ts: Transform, mask: Option<&Mask>) {
    let pt = paint(color);
    match g {
        Geom::Poly { outer, holes } => {
            let mut pb = PathBuilder::new();
            ring(&mut pb, outer);
            for h in holes {
                ring(&mut pb, h);
            }
            if let Some(path) = pb.finish() {
                pm.fill_path(&path, &pt, FillRule::EvenOdd, ts, mask);
            }
        }
        Geom::Stroke { pts, width, closed } => {
            let mut pb = PathBuilder::new();
            if let Some(first) = pts.first() {
                pb.move_to(first.x as f32, first.y as f32);
                for p in &pts[1..] {
                    pb.line_to(p.x as f32, p.y as f32);
                }
                if pts.len() == 1 {
                    pb.line_to(first.x as f32 + 1e-4, first.y as f32);
                }
                if *closed {
                    pb.close();
                }
            }
            if let Some(path) = pb.finish() {
                let st = Stroke {
                    width: *width as f32,
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Stroke::default()
                };
                pm.stroke_path(&path, &pt, &st, ts, mask);
            }
        }
        Geom::Disc { c, r } => {
            if let Some(path) =
                PathBuilder::from_circle(c.x as f32, c.y as f32, (*r as f32).max(1e-4))
            {
                pm.fill_path(&path, &pt, FillRule::Winding, ts, mask);
            }
        }
    }
}

fn geoms_mask(items: &[Geom], w: u32, h: u32, ts: Transform) -> Option<Mask> {
    let mut pm = Pixmap::new(w, h)?;
    for g in items {
        draw_geom(&mut pm, g, [255, 255, 255, 255], ts, None);
    }
    Some(Mask::from_pixmap(pm.as_ref(), tiny_skia::MaskType::Alpha))
}

pub use lamina_ports::render::Options2D;

/// Technical view: each layer in its own colour.
pub fn render_layers(s: &Scene2D, opt: &Options2D) -> Option<Pixmap> {
    let mut pm = Pixmap::new(opt.width, opt.height)?;
    let bg = theme::BACKGROUND;
    pm.fill(tiny_skia::Color::from_rgba8(bg[0], bg[1], bg[2], 255));
    let view = View::fit(
        opt.region.unwrap_or(pad_rect(s.bbox, 1.0)),
        opt.width,
        opt.height,
        12.0,
    );
    let ts = view.transform();
    // Board body.
    if let Some(o) = s.layer(SceneLayer::Outline) {
        for g in &o.items {
            if let Geom::Stroke { pts, .. } = g {
                draw_geom(
                    &mut pm,
                    &Geom::Poly {
                        outer: pts.clone(),
                        holes: vec![],
                    },
                    theme::BOARD_2D,
                    ts,
                    None,
                );
            }
        }
    }
    for l in &s.layers {
        if let Some(sel) = &opt.layers {
            if !sel.contains(&l.layer) {
                continue;
            }
        }
        // Mask openings and paste make the technical view unclear.
        if opt.layers.is_none() && matches!(l.layer, SceneLayer::Mask(_) | SceneLayer::Paste(_)) {
            continue;
        }
        let c = theme::layer_color(l.layer, s.copper_layers);
        // Draw each layer into its own pixmap, so that overlaps are not drawn two times.
        let mut layer_pm = Pixmap::new(opt.width, opt.height)?;
        let opaque = [c[0], c[1], c[2], 255];
        for g in &l.items {
            draw_geom(&mut layer_pm, g, opaque, ts, None);
        }
        let paint = tiny_skia::PixmapPaint {
            opacity: c[3] as f32 / 255.0,
            ..Default::default()
        };
        pm.draw_pixmap(0, 0, layer_pm.as_ref(), &paint, Transform::identity(), None);
    }
    Some(pm)
}

fn pad_rect(r: Rect, m: f64) -> Rect {
    Rect {
        min: Point::new(r.min.x - m, r.min.y - m),
        max: Point::new(r.max.x + m, r.max.y + m),
    }
}

fn layer_items(s: &Scene2D, l: SceneLayer) -> &[Geom] {
    s.layer(l).map(|d| d.items.as_slice()).unwrap_or(&[])
}

/// The appearance of one side of the finished board: mask, exposed finish, silk.
/// Pixels outside the board are transparent.
pub fn appearance(s: &Scene2D, side: Side, view: View, background: Option<Rgba>) -> Option<Pixmap> {
    let (w, h) = (view.width, view.height);
    let ts = view.transform();
    let mut pm = Pixmap::new(w, h)?;
    if let Some(bg) = background {
        pm.fill(tiny_skia::Color::from_rgba8(bg[0], bg[1], bg[2], bg[3]));
    }
    let outline: Vec<Geom> = layer_items(s, SceneLayer::Outline)
        .iter()
        .filter_map(|g| match g {
            Geom::Stroke { pts, .. } => Some(Geom::Poly {
                outer: pts.clone(),
                holes: vec![],
            }),
            _ => None,
        })
        .collect();
    let mask_c = theme::mask(&s.mask_color);
    let copper_i = match side {
        Side::Top => 0,
        Side::Bottom => s.copper_layers.saturating_sub(1),
    };
    // Laminate under the mask.
    for g in outline.iter().take(1) {
        draw_geom(&mut pm, g, mask_c, ts, None);
    }
    for g in outline.iter().skip(1) {
        draw_geom(&mut pm, g, [0, 0, 0, 0], ts, None);
    }
    // Copper shows through the mask as a lighter tint.
    let lighter = [
        (mask_c[0] as f32 * 1.0 + 40.0).min(255.0) as u8,
        (mask_c[1] as f32 * 1.0 + 46.0).min(255.0) as u8,
        (mask_c[2] as f32 * 1.0 + 30.0).min(255.0) as u8,
        255,
    ];
    let copper = layer_items(s, SceneLayer::Copper(copper_i));
    let mut cu = Pixmap::new(w, h)?;
    for g in copper {
        draw_geom(&mut cu, g, lighter, ts, None);
    }
    pm.draw_pixmap(
        0,
        0,
        cu.as_ref(),
        &tiny_skia::PixmapPaint {
            opacity: 0.55,
            ..Default::default()
        },
        Transform::identity(),
        None,
    );
    // Mask openings: bare laminate, then exposed copper in the finish colour.
    let openings = layer_items(s, SceneLayer::Mask(side));
    if let Some(open_mask) = geoms_mask(openings, w, h, ts) {
        for g in openings {
            draw_geom(&mut pm, g, theme::FR4, ts, None);
        }
        let fin = theme::finish(&s.finish);
        for g in copper {
            draw_geom(&mut pm, g, fin, ts, Some(&open_mask));
        }
        // Silk is not printed on openings.
        let mut no_open = open_mask.clone();
        no_open.invert();
        let silk = theme::silk(&s.silk_color);
        for g in layer_items(s, SceneLayer::Silk(side)) {
            draw_geom(&mut pm, g, silk, ts, Some(&no_open));
        }
    }
    for g in layer_items(s, SceneLayer::Drills) {
        draw_geom(&mut pm, g, [18, 18, 18, 255], ts, None);
    }
    Some(pm)
}

/// Photo-like view of one side, as a PNG-ready pixmap.
pub fn render_side(s: &Scene2D, side: Side, width: u32, height: u32) -> Option<Pixmap> {
    let mut view = View::fit(pad_rect(s.bbox, 1.0), width, height, 12.0);
    // Seen from below, the bottom side is mirrored.
    view.mirror = side == Side::Bottom;
    appearance(s, side, view, Some(theme::BACKGROUND))
}

pub fn png(pm: &Pixmap) -> Result<Vec<u8>, String> {
    pm.encode_png().map_err(|e| e.to_string())
}
