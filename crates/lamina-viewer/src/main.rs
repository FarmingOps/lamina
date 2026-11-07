// In release builds, the viewer has no console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod editor;
mod edits;
mod live;
mod style;
mod tools;
mod widgets;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Pos2, Rect as ERect, Sense, Stroke, StrokeKind, Vec2,
};
use eframe::egui_wgpu;
use lamina_geom::{Owner, Prim};
use lamina_model::{
    Layer, NotesFile, Point, ProjectData, Rect, Severity, Side, json, write_atomic,
};
use lamina_render::{Camera, Gpu, Gpu2D, Gpu3D, MeshCache, Target};
use lamina_scene::SceneLayer;

use editor::Editor;
use edits::Sel;
use live::{Live, Loaded, Msg};
use style::*;
use tools::{Gesture, Placing, Settling, Tool};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    TwoD,
    ThreeD,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Pick {
    Pad {
        part: String,
        pad: String,
        net: Option<String>,
        layer: u8,
    },
    Track {
        id: String,
        net: String,
        seg: usize,
    },
    Via {
        id: String,
        net: String,
    },
    Fill {
        zone: String,
        net: Option<String>,
    },
    Part {
        refdes: String,
    },
    Keepout {
        id: String,
    },
    Hole {
        id: String,
    },
    Outline,
}

impl Pick {
    fn net(&self) -> Option<&str> {
        match self {
            Pick::Pad { net, .. } => net.as_deref(),
            Pick::Track { net, .. } | Pick::Via { net, .. } => Some(net),
            Pick::Fill { net, .. } => net.as_deref(),
            Pick::Part { .. } | Pick::Keepout { .. } | Pick::Hole { .. } | Pick::Outline => None,
        }
    }

    fn part(&self) -> Option<&str> {
        match self {
            Pick::Pad { part, .. } | Pick::Part { refdes: part } => Some(part),
            _ => None,
        }
    }
}

pub(crate) struct App {
    root: Option<PathBuf>,
    live: Option<Live>,
    loaded: Option<Box<Loaded>>,
    error: Option<String>,
    gpu: Option<Gpu>,
    target: Option<Target>,
    tex: Option<egui::TextureId>,
    g2d: Option<Gpu2D>,
    g3d: Option<Gpu3D>,
    cache: MeshCache,
    mode: Mode,
    center: [f64; 2],
    ppm: f64,
    fitted: bool,
    cam: Camera,
    cam_fitted: bool,
    visible: BTreeMap<SceneLayer, bool>,
    hover: Option<Pick>,
    sel: Vec<Sel>,
    editor: Option<Editor>,
    tool: Tool,
    gesture: Gesture,
    route: Option<edits::Route>,
    poly: Vec<Point>,
    placing: Option<Placing>,
    settling: Option<Settling>,
    grid_i: usize,
    active: u8,
    diagonal_first: bool,
    toast: Option<(String, bool, Instant)>,
    own_edit: Option<Instant>,
    /// The groups that can take the layout of a group: (group, load time, groups).
    group_matches: Option<(String, Option<Instant>, Vec<String>)>,
    mm_per_px: f64,
    last_cursor: Option<Point>,
    fit_pending: bool,
    flashes: Vec<(Rect, Instant)>,
    last_reload: Option<Instant>,
    cursor_mm: Option<Point>,
    note_text: String,
    note_at: Option<Point>,
    note_status: Option<(String, Instant)>,
    dirty: bool,
    open_path: String,
    /// This board gets the keyboard shortcuts. If several boards are open, it is
    /// the board that the user clicked last.
    pub(crate) focused: bool,
    /// A request from the empty state. The shell does it.
    pub(crate) request: Option<OpenRequest>,
    /// The canvas size in pixels at the last frame. The panels use it to zoom.
    last_size_px: Vec2,
    /// Exports and other background jobs. Their results show as toasts.
    bg: Vec<std::sync::mpsc::Receiver<actions::BgDone>>,
    find: Option<actions::Find>,
    help: bool,
    measure: actions::Measure,
    /// The view and the canvas size of the last automatic fit. If the view did
    /// not change, a resized canvas gets a new fit.
    fit_2d_at: Option<([f64; 2], f64, Vec2)>,
    fit_3d_at: Option<(Camera, Vec2)>,
}

fn default_visible(l: SceneLayer) -> bool {
    !matches!(
        l,
        SceneLayer::Mask(_)
            | SceneLayer::Paste(_)
            | SceneLayer::Fab(_)
            | SceneLayer::Courtyard(Side::Bottom)
    )
}

fn config_file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("lamina").join("viewer-last-project.txt"))
}

impl App {
    fn new(gpu: Option<Gpu>, root: Option<PathBuf>, three_d: bool, ctx: &egui::Context) -> App {
        let mut app = App {
            root: None,
            live: None,
            loaded: None,
            error: None,
            gpu,
            target: None,
            tex: None,
            g2d: None,
            g3d: None,
            cache: MeshCache::default(),
            mode: if three_d { Mode::ThreeD } else { Mode::TwoD },
            center: [0.0, 0.0],
            ppm: 20.0,
            fitted: false,
            cam: Camera::frame([0.0; 3], [50.0, 50.0, 2.0]),
            cam_fitted: false,
            visible: BTreeMap::new(),
            hover: None,
            sel: Vec::new(),
            editor: None,
            tool: Tool::Select,
            gesture: Gesture::None,
            route: None,
            poly: Vec::new(),
            placing: None,
            settling: None,
            grid_i: 2,
            active: 0,
            diagonal_first: false,
            toast: None,
            own_edit: None,
            group_matches: None,
            mm_per_px: 0.05,
            last_cursor: None,
            fit_pending: false,
            flashes: Vec::new(),
            last_reload: None,
            cursor_mm: None,
            note_text: String::new(),
            note_at: None,
            note_status: None,
            dirty: true,
            open_path: String::new(),
            focused: true,
            request: None,
            last_size_px: Vec2::new(900.0, 700.0),
            bg: Vec::new(),
            find: None,
            help: false,
            measure: actions::Measure::default(),
            fit_2d_at: None,
            fit_3d_at: None,
        };
        if let Some(r) = root {
            app.open(r, ctx);
        }
        app
    }

    fn open(&mut self, root: PathBuf, ctx: &egui::Context) {
        if !ProjectData::is_project(&root) {
            self.error = Some(format!(
                "{} is not a Lamina project. It has no board.json.",
                root.display()
            ));
            return;
        }
        let ctx2 = ctx.clone();
        match Live::start(root.clone(), move || ctx2.request_repaint()) {
            Ok(l) => {
                self.live = Some(l);
                let ctx3 = ctx.clone();
                self.editor = Some(Editor::start(root.clone(), move || ctx3.request_repaint()));
                self.loaded = None;
                self.fitted = false;
                self.cam_fitted = false;
                self.sel.clear();
                self.set_tool(Tool::Select);
                self.error = None;
                self.root = Some(root);
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn poll(&mut self) {
        self.poll_bg();
        while let Some(r) = self.editor.as_mut().and_then(|e| e.poll()) {
            if let Some(s) = self.settling.as_mut() {
                s.replied = true;
                if !r.ok {
                    self.settling = None;
                }
            }
            self.toast = Some((r.message, r.ok, Instant::now()));
        }
        let Some(live) = &self.live else { return };
        while let Ok(msg) = live.rx.try_recv() {
            match msg {
                Msg::Loaded(l) => {
                    let now = Instant::now();
                    if self.settling.as_ref().is_some_and(|s| s.replied) {
                        self.settling = None;
                    }
                    // Flash only the changes that someone else made.
                    let own = self
                        .own_edit
                        .is_some_and(|t| t.elapsed() < Duration::from_millis(2500));
                    if self.loaded.is_some() && !own {
                        for r in &l.changed {
                            self.flashes.push((*r, now));
                        }
                    }
                    if l.models_changed {
                        self.cache = MeshCache::default();
                    }
                    for layer in l.s2.layers.iter().map(|d| d.layer) {
                        self.visible
                            .entry(layer)
                            .or_insert_with(|| default_visible(layer));
                    }
                    for i in 0..l.s2.copper_layers {
                        self.visible.entry(SceneLayer::Copper(i)).or_insert(true);
                    }
                    if let Some(gpu) = &self.gpu {
                        self.g2d = Some(gpu.upload_2d(&l.s2, 40.0, 0));
                        self.g3d = Some(gpu.upload_3d(&l.s3, &l.s2, &mut self.cache));
                    }
                    self.loaded = Some(l);
                    self.error = None;
                    self.last_reload = Some(now);
                    self.dirty = true;
                }
                Msg::Error(e) => self.error = Some(e),
            }
        }
    }

    fn is_visible(&self, l: SceneLayer) -> bool {
        self.visible.get(&l).copied().unwrap_or(true)
    }

    fn fit_2d(&mut self, size_px: Vec2) {
        let Some(l) = &self.loaded else { return };
        let b = l.s2.bbox;
        self.center = [(b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0];
        let sx = (size_px.x as f64 * 0.86) / b.width().max(1.0);
        let sy = (size_px.y as f64 * 0.86) / b.height().max(1.0);
        self.ppm = sx.min(sy).clamp(0.5, 4000.0);
        self.fit_2d_at = Some((self.center, self.ppm, size_px));
        self.fitted = true;
        self.dirty = true;
    }

    fn fit_3d(&mut self, size_px: Vec2) {
        if let Some(g) = &self.g3d {
            let aspect = size_px.x / size_px.y.max(1.0);
            self.cam = Camera::frame_aspect(g.bounds.0, g.bounds.1, aspect);
            self.fit_3d_at = Some((self.cam, size_px));
            self.cam_fitted = true;
            self.dirty = true;
        }
    }

    fn zoom_to(&mut self, at: Point, span_mm: f64, size_px: Vec2) {
        self.mode = Mode::TwoD;
        self.center = [at.x, at.y];
        self.ppm = (size_px.x.min(size_px.y) as f64 / span_mm).clamp(0.5, 4000.0);
        self.dirty = true;
    }

    /// The item at a board point. The top layer comes first.
    fn pick(&self, p: Point) -> Option<Pick> {
        let l = self.loaded.as_ref()?;
        let n = l.s2.copper_layers;
        let mut best: Option<(u8, u8, Pick)> = None;
        for it in &l.analysis.geometry.copper {
            if !self.is_visible(SceneLayer::Copper(it.layer)) || !it.prim.contains(p) {
                continue;
            }
            let (rank, pick) = match &it.owner {
                Owner::Pad { part, pad, .. } => (
                    0,
                    Pick::Pad {
                        part: part.clone(),
                        pad: pad.clone(),
                        net: it.net.clone(),
                        layer: it.layer,
                    },
                ),
                Owner::Via { id } => (
                    1,
                    Pick::Via {
                        id: id.clone(),
                        net: it.net.clone().unwrap_or_default(),
                    },
                ),
                Owner::Hole { id } => (1, Pick::Hole { id: id.clone() }),
                Owner::Track { id, seg } => (
                    2,
                    Pick::Track {
                        id: id.clone(),
                        net: it.net.clone().unwrap_or_default(),
                        seg: *seg,
                    },
                ),
                _ => continue,
            };
            let key = (it.layer, rank);
            if best.as_ref().is_none_or(|b| (b.0, b.1) > key) {
                best = Some((key.0, key.1, pick));
            }
        }
        if let Some((_, _, p)) = best {
            return Some(p);
        }
        let d = &l.data;
        for side in [Side::Top, Side::Bottom] {
            for (r, pl) in &d.placement.placements {
                if pl.side != side {
                    continue;
                }
                let Some(fp) = d
                    .parts
                    .parts
                    .get(r)
                    .and_then(|x| d.footprints.get(&x.footprint))
                else {
                    continue;
                };
                if let Some(c) = lamina_geom::courtyard(fp, pl) {
                    if lamina_geom::prim::ring_contains(&c, p) {
                        return Some(Pick::Part { refdes: r.clone() });
                    }
                }
            }
        }
        // Pours come after parts. A click in a courtyard selects the part.
        for f in &l.analysis.fills {
            let Some(c) = f.layer.copper() else { continue };
            if !self.is_visible(SceneLayer::Copper(c.index(n))) {
                continue;
            }
            if f.polygons.iter().any(|poly| poly.prim().contains(p)) {
                return Some(Pick::Fill {
                    zone: f.zone.clone(),
                    net: f.net.clone(),
                });
            }
        }
        for h in &d.board.holes {
            if h.at.dist(p) <= h.drill.min_size() / 2.0 {
                return Some(Pick::Hole { id: h.id.clone() });
            }
        }
        let edge = 5.0 * self.mm_per_px;
        let near_edge = |c: &lamina_model::Contour| {
            let pts = c.flatten(0.02);
            (0..pts.len())
                .any(|i| lamina_geom::prim::seg_point(pts[i], pts[(i + 1) % pts.len()], p) <= edge)
        };
        if self.is_visible(SceneLayer::Keepout) {
            for k in &d.routing.keepouts {
                let pts = k.outline.flatten(0.02);
                if near_edge(&k.outline) || lamina_geom::prim::ring_contains(&pts, p) {
                    return Some(Pick::Keepout { id: k.id.clone() });
                }
            }
        }
        // Do the outline before the pours, because the edge of a pour is usually on it.
        if let Some(o) = &d.board.outline {
            if near_edge(o) {
                return Some(Pick::Outline);
            }
        }
        for z in &d.routing.zones {
            let visible = z.layers.iter().any(|l| {
                l.copper()
                    .is_some_and(|c| self.is_visible(SceneLayer::Copper(c.index(n))))
            });
            if visible && near_edge(&z.outline) {
                return Some(Pick::Fill {
                    zone: z.id.clone(),
                    net: z.net.clone(),
                });
            }
        }
        None
    }

    fn add_note(&mut self) {
        let text = self.note_text.trim().to_string();
        let Some(root) = &self.root else { return };
        if text.is_empty() {
            return;
        }
        let path = root.join("notes.json");
        let mut notes: NotesFile = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        notes.schema = lamina_model::SCHEMA;
        let next = notes
            .notes
            .iter()
            .filter_map(|n| n.id.strip_prefix('n')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let refdes = self.sel.iter().find_map(|s| s.part()).map(str::to_string);
        notes.notes.push(lamina_model::Note {
            id: format!("n{next}"),
            text,
            at: self
                .note_at
                .map(|p| Point::new(json::round6(p.x), json::round6(p.y))),
            refdes,
            author: "user".into(),
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            resolved: false,
        });
        let res = json::to_string(&notes)
            .map_err(|e| e.to_string())
            .and_then(|s| write_atomic(&path, s.as_bytes()).map_err(|e| e.to_string()));
        match res {
            Ok(()) => {
                self.note_text.clear();
                self.note_status = Some((
                    format!("Sent as n{next}. The agent sees it with the notes tool."),
                    Instant::now(),
                ));
            }
            Err(e) => {
                self.note_status = Some((format!("Cannot save the note: {e}"), Instant::now()))
            }
        }
    }
}

// ---------------------------------------------------------------- drawing helpers

struct Xf {
    rect: ERect,
    center: [f64; 2],
    ppm_pt: f64,
}

impl Xf {
    fn to_screen(&self, p: Point) -> Pos2 {
        Pos2::new(
            self.rect.center().x + ((p.x - self.center[0]) * self.ppm_pt) as f32,
            self.rect.center().y - ((p.y - self.center[1]) * self.ppm_pt) as f32,
        )
    }

    fn to_board(&self, s: Pos2) -> Point {
        Point::new(
            self.center[0] + (s.x - self.rect.center().x) as f64 / self.ppm_pt,
            self.center[1] - (s.y - self.rect.center().y) as f64 / self.ppm_pt,
        )
    }

    fn rect(&self, r: &Rect) -> ERect {
        ERect::from_two_pos(self.to_screen(r.min), self.to_screen(r.max))
    }
}

fn draw_prim(painter: &egui::Painter, xf: &Xf, prim: &Prim, color: Color32, glow: bool) {
    match prim {
        Prim::Capsule { a, b, r } if glow => {
            let w = ((2.0 * r * xf.ppm_pt) as f32).max(2.0);
            painter.line_segment(
                [xf.to_screen(*a), xf.to_screen(*b)],
                Stroke::new(w + 3.0, color.gamma_multiply(0.35)),
            );
            painter.line_segment(
                [xf.to_screen(*a), xf.to_screen(*b)],
                Stroke::new(1.2, color),
            );
        }
        _ => {
            let pts: Vec<Pos2> = prim
                .outline(0.02)
                .into_iter()
                .map(|p| xf.to_screen(p))
                .collect();
            painter.add(egui::Shape::closed_line(pts, Stroke::new(1.5, color)));
        }
    }
}

// ---------------------------------------------------------------- app

impl App {
    /// Upkeep for each frame after the drawing. Returns true while something animates.
    fn animating(&mut self) -> bool {
        let animating = self
            .flashes
            .iter()
            .any(|(_, t)| t.elapsed() < Duration::from_millis(900))
            || self
                .note_status
                .as_ref()
                .is_some_and(|(_, t)| t.elapsed() < Duration::from_secs(4))
            || self
                .toast
                .as_ref()
                .is_some_and(|(_, _, t)| t.elapsed() < Duration::from_secs(4))
            || self.editor.as_ref().is_some_and(|e| e.busy > 0);
        self.flashes
            .retain(|(_, t)| t.elapsed() < Duration::from_millis(900));
        animating
    }

    fn recently_reloaded(&self) -> bool {
        self.last_reload
            .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
    }

    /// Name shown on the board's tab.
    fn tab_name(&self) -> String {
        if let Some(l) = &self.loaded {
            return l.data.board.name.clone();
        }
        self.root
            .as_ref()
            .and_then(|r| std::path::absolute(r).ok())
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Open a board".into())
    }

    /// The health colour: error, warning, clean, or not loaded.
    fn health(&self) -> Color32 {
        let Some(l) = &self.loaded else {
            return TEXT_3;
        };
        let v = &l.analysis.violations;
        if v.iter().any(|v| v.severity == Severity::Error) {
            ERROR
        } else if !v.is_empty() {
            WARN
        } else {
            OK
        }
    }

    /// The DRC, routing and live chips, from right to left.
    fn chips(&self, ui: &mut egui::Ui) {
        let Some(l) = self.loaded.as_ref() else {
            return;
        };
        let errors = l
            .analysis
            .violations
            .iter()
            .filter(|v| v.severity == Severity::Error)
            .count();
        let warnings = l.analysis.violations.len() - errors;
        let unrouted = l.analysis.ratsnest.len();
        widgets::live_dot(ui, self.last_reload);
        ui.add_space(6.0);
        if !l.generated.is_empty() {
            widgets::chip(ui, TEXT_2, "design.jsonnet");
        }
        widgets::chip(
            ui,
            if unrouted == 0 { OK } else { TEXT_2 },
            &if unrouted == 0 {
                "all routed".to_string()
            } else {
                format!("{unrouted} unrouted")
            },
        );
        let (c, text) = if errors > 0 {
            (
                ERROR,
                format!("{errors} error{}", if errors == 1 { "" } else { "s" }),
            )
        } else if warnings > 0 {
            (
                WARN,
                format!("{warnings} warning{}", if warnings == 1 { "" } else { "s" }),
            )
        } else {
            (OK, "DRC clean".to_string())
        };
        widgets::chip(ui, c, &text);
    }

    /// The strip above the canvas of a board: name and path, 2D/3D switch and
    /// close button. Returns true if the user clicked the close button.
    fn pane_header(&mut self, ui: &mut egui::Ui, closable: bool) -> bool {
        let mut close = false;
        // Add the controls on the right first. Then the name and path get the
        // remaining space. They are cut short and do not go under the controls.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            {
                ui.add_space(8.0);
                if closable
                    && ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("×").font(body(15.0)).color(TEXT_2),
                            )
                            .frame(false),
                        )
                        .on_hover_text("Close this board")
                        .clicked()
                {
                    close = true;
                }
                if self.loaded.is_some() {
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(108.0, 24.0), Sense::hover());
                    let mut sel = if self.mode == Mode::TwoD { 0 } else { 1 };
                    if widgets::segmented(ui, rect, &["2D", "3D"], &mut sel) {
                        self.mode = if sel == 0 { Mode::TwoD } else { Mode::ThreeD };
                        self.dirty = true;
                    }
                }
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 3.5, self.health());
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(self.tab_name())
                            .font(semibold(13.0))
                            .color(TEXT),
                    )
                    .truncate(),
                );
                if let Some(root) = &self.root {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(root.display().to_string())
                                .font(body(11.5))
                                .color(TEXT_3),
                        )
                        .truncate(),
                    )
                    .on_hover_text(root.display().to_string());
                }
            });
        });
        close
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            let mono_c = |s: String| egui::RichText::new(s).font(mono(11.5)).color(TEXT_2);
            match (self.mode, self.cursor_mm) {
                (Mode::TwoD, Some(p)) => {
                    ui.label(mono_c(format!("x {:>8.3}   y {:>8.3} mm", p.x, p.y)));
                }
                _ => {
                    ui.label(mono_c("x        –   y        – mm".into()));
                }
            }
            ui.add_space(16.0);
            if self.mode == Mode::TwoD {
                ui.label(mono_c(format!("{:.1} px/mm", self.ppm)));
            } else {
                ui.label(mono_c(format!(
                    "yaw {:.0}°  pitch {:.0}°",
                    self.cam.yaw, self.cam.pitch
                )));
            }
            if let Some(err) = &self.error {
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new(format!("Cannot reload: {err}"))
                        .font(body(12.0))
                        .color(ERROR),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(l) = &self.loaded {
                    let d = &l.data;
                    ui.label(mono_c(format!("rebuilt in {} ms", l.took.as_millis())));
                    ui.add_space(14.0);
                    ui.label(mono_c(format!(
                        "{} parts · {} nets · {} tracks · {} vias",
                        d.parts.parts.len(),
                        d.nets.nets.len(),
                        d.routing.tracks.len(),
                        d.routing.vias.len()
                    )));
                    if let Some(b) = d.board.outline.as_ref().and_then(|o| o.bbox()) {
                        ui.add_space(14.0);
                        ui.label(mono_c(format!(
                            "{:.1} × {:.1} mm · {} layers",
                            b.width(),
                            b.height(),
                            d.board.stackup.copper_layers
                        )));
                    }
                }
            });
        });
    }

    fn layer_panel(&mut self, ui: &mut egui::Ui) {
        let Some(l) = &self.loaded else { return };
        let n = l.s2.copper_layers;
        let groups: [(&str, Vec<SceneLayer>); 3] = [
            ("Copper", (0..n).map(SceneLayer::Copper).collect()),
            (
                "Technical",
                vec![
                    SceneLayer::Silk(Side::Top),
                    SceneLayer::Silk(Side::Bottom),
                    SceneLayer::Courtyard(Side::Top),
                    SceneLayer::Courtyard(Side::Bottom),
                    SceneLayer::Fab(Side::Top),
                    SceneLayer::Fab(Side::Bottom),
                    SceneLayer::Mask(Side::Top),
                    SceneLayer::Mask(Side::Bottom),
                    SceneLayer::Paste(Side::Top),
                    SceneLayer::Paste(Side::Bottom),
                ],
            ),
            (
                "Overlays",
                vec![
                    SceneLayer::Outline,
                    SceneLayer::Drills,
                    SceneLayer::Keepout,
                    SceneLayer::Ratsnest,
                    SceneLayer::Drc,
                ],
            ),
        ];
        let present: Vec<SceneLayer> = l.s2.layers.iter().map(|d| d.layer).collect();
        let count = |layer: SceneLayer| l.s2.layer(layer).map(|d| d.items.len()).unwrap_or(0);
        let mut changed = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    widgets::section_title(ui, "Layers");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (label, f) in [("Bottom", 2u8), ("Top", 1u8), ("All", 0u8)] {
                            if widgets::text_button(ui, label) {
                                for (k, v) in self.visible.iter_mut() {
                                    *v = match f {
                                        0 => default_visible(*k),
                                        1 => k.side(n) != Some(Side::Bottom) && default_visible(*k),
                                        _ => k.side(n) != Some(Side::Top) && default_visible(*k),
                                    };
                                }
                                changed = true;
                            }
                        }
                    });
                });
                ui.add_space(6.0);
                for (gname, layers) in &groups {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(*gname).font(body(11.5)).color(TEXT_3));
                    ui.add_space(2.0);
                    for layer in layers {
                        if !matches!(layer, SceneLayer::Copper(_)) && !present.contains(layer) {
                            continue;
                        }
                        let mut on = self.visible.get(layer).copied().unwrap_or(true);
                        let color = rgba(lamina_scene::theme::layer_color(*layer, n));
                        if widgets::layer_row(ui, &layer.name(n), color, count(*layer), &mut on) {
                            self.visible.insert(*layer, on);
                            changed = true;
                        }
                    }
                }
            });
        if changed {
            self.dirty = true;
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let size_hint = self.last_size_px;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                widgets::section_title(ui, "Inspector");
                ui.add_space(6.0);
                if !self.sel.is_empty() {
                    self.selection_panel(ui);
                }
                if self.sel.is_empty() {
                    match (&self.hover, &self.loaded) {
                        (Some(p), Some(l)) => self.pick_details(ui, p, &l.data),
                        _ => {
                            ui.label(
                                egui::RichText::new(
                                    "Hover over a pad, track or part to see its data. Click it to select and edit it.",
                                )
                                .font(body(12.5))
                                .color(TEXT_3),
                            );
                        }
                    }
                }
                self.unplaced_panel(ui);

                ui.add_space(18.0);
                widgets::hairline(ui);
                ui.add_space(12.0);
                let Some(l) = &self.loaded else { return };
                let mut violations: Vec<&lamina_model::Violation> =
                    l.analysis.violations.iter().collect();
                violations.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.rule.cmp(&b.rule)));
                ui.horizontal(|ui| {
                    widgets::section_title(ui, "Checks");
                    ui.label(
                        egui::RichText::new(format!("{}", violations.len()))
                            .font(mono(11.5))
                            .color(TEXT_3),
                    );
                });
                ui.add_space(6.0);
                if violations.is_empty() {
                    ui.label(
                        egui::RichText::new("There are no rule violations. All checks pass.")
                            .font(body(12.5))
                            .color(OK),
                    );
                }
                let mut goto = None;
                for v in violations.iter().take(200) {
                    let color = if v.severity == Severity::Error {
                        ERROR
                    } else {
                        WARN
                    };
                    if widgets::violation_row(ui, color, &v.rule, &v.message, v.at.is_some()) {
                        goto = v.at;
                    }
                }
                if let Some(at) = goto {
                    self.zoom_to(at, 8.0, size_hint);
                }

                ui.add_space(18.0);
                widgets::hairline(ui);
                ui.add_space(12.0);
                self.notes_section(ui);
            });
    }

    fn pick_details(&self, ui: &mut egui::Ui, p: &Pick, d: &ProjectData) {
        let mut rows: Vec<(&str, String)> = Vec::new();
        let title;
        match p {
            Pick::Pad {
                part,
                pad,
                net,
                layer,
            } => {
                title = format!("{part}.{pad}");
                rows.push(("net", net.clone().unwrap_or_else(|| "unconnected".into())));
                rows.push((
                    "layer",
                    Layer::Copper(lamina_model::Copper::from_index(
                        *layer,
                        d.board.stackup.copper_layers,
                    ))
                    .to_string(),
                ));
                if let Some(fp) = d
                    .parts
                    .parts
                    .get(part)
                    .and_then(|x| d.footprints.get(&x.footprint))
                {
                    if let Some(pd) = fp.pad(pad) {
                        let (w, h) = pd.shape.size();
                        rows.push(("size", format!("{w:.3} × {h:.3} mm")));
                        if let lamina_model::PadKind::Through { drill, plated } = &pd.kind {
                            rows.push((
                                "drill",
                                format!(
                                    "{:.3} mm {}",
                                    drill.min_size(),
                                    if *plated { "plated" } else { "NPTH" }
                                ),
                            ));
                        }
                    }
                }
                part_rows(d, part, &mut rows);
            }
            Pick::Part { refdes } => {
                title = refdes.clone();
                part_rows(d, refdes, &mut rows);
            }
            Pick::Track { id, net, seg } => {
                title = format!("Track {id}");
                rows.push(("net", net.clone()));
                if let Some(t) = d.routing.tracks.iter().find(|t| &t.id == id) {
                    let len: f64 = t.points.windows(2).map(|w| w[0].dist(w[1])).sum();
                    rows.push(("layer", t.layer.to_string()));
                    rows.push(("width", format!("{:.3} mm", t.width)));
                    rows.push(("length", format!("{len:.2} mm")));
                    rows.push((
                        "segment",
                        format!("{} of {}", seg + 1, t.points.len().saturating_sub(1)),
                    ));
                }
            }
            Pick::Via { id, net } => {
                title = format!("Via {id}");
                rows.push(("net", net.clone()));
                if let Some(v) = d.routing.vias.iter().find(|v| &v.id == id) {
                    rows.push(("at", format!("{:.3}, {:.3}", v.at.x, v.at.y)));
                    rows.push(("size", format!("{:.2} / {:.2} mm", v.diameter, v.drill)));
                }
            }
            Pick::Fill { zone, net } => {
                title = format!("Pour {zone}");
                rows.push(("net", net.clone().unwrap_or_else(|| "none".into())));
            }
            Pick::Keepout { id } => {
                title = format!("Keep-out {id}");
                if let Some(k) = d.routing.keepouts.iter().find(|k| &k.id == id) {
                    let ls: Vec<String> = k.layers.iter().map(|l| l.to_string()).collect();
                    rows.push(("layers", ls.join(", ")));
                }
            }
            Pick::Hole { id } => {
                title = format!("Hole {id}");
                if let Some(h) = d.board.holes.iter().find(|h| &h.id == id) {
                    rows.push(("at", format!("{:.3}, {:.3}", h.at.x, h.at.y)));
                    rows.push(("drill", format!("{:.2} mm", h.drill.min_size())));
                }
            }
            Pick::Outline => {
                title = "Board outline".to_string();
                if let Some(b) = d.board.outline.as_ref().and_then(|o| o.bbox()) {
                    rows.push(("size", format!("{:.2} × {:.2} mm", b.width(), b.height())));
                }
            }
        }
        ui.label(egui::RichText::new(title).font(semibold(16.0)).color(TEXT));
        ui.add_space(6.0);
        egui::Grid::new("kv")
            .num_columns(2)
            .spacing([14.0, 5.0])
            .show(ui, |ui| {
                for (k, v) in rows {
                    ui.label(egui::RichText::new(k).font(body(12.0)).color(TEXT_3));
                    let mono_like = v
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_digit() || c == '-');
                    let rt = egui::RichText::new(v.clone()).color(TEXT);
                    let rt = if mono_like {
                        rt.font(mono(12.0))
                    } else {
                        rt.font(body(12.5))
                    };
                    // Cut long values short. Do not make the panel wider.
                    ui.add(egui::Label::new(rt).truncate()).on_hover_text(v);
                    ui.end_row();
                }
            });
    }

    fn notes_section(&mut self, ui: &mut egui::Ui) {
        widgets::section_title(ui, "Notes for the agent");
        ui.add_space(6.0);
        if let Some(l) = &self.loaded {
            let open: Vec<&lamina_model::Note> = l.data.notes.notes.iter().rev().take(12).collect();
            for n in open {
                widgets::note_row(ui, n);
            }
        }
        ui.add_space(6.0);
        let anchor = match (self.sel.iter().find_map(|s| s.part()), self.note_at) {
            (Some(r), _) => format!("Attached to {r}"),
            (None, Some(p)) => format!("Attached to ({:.2}, {:.2}) mm", p.x, p.y),
            _ => "Click the board to attach a location".to_string(),
        };
        ui.label(egui::RichText::new(anchor).font(body(11.5)).color(TEXT_3));
        ui.add(
            egui::TextEdit::multiline(&mut self.note_text)
                .hint_text("Example: move C3 nearer to U1 pin 1")
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .font(body(13.0)),
        );
        ui.add_space(4.0);
        let enabled = !self.note_text.trim().is_empty();
        if widgets::primary_button(ui, "Send to agent", enabled) {
            self.add_note();
        }
        if let Some((msg, t)) = &self.note_status {
            if t.elapsed() < Duration::from_secs(4) {
                ui.label(egui::RichText::new(msg).font(body(11.5)).color(TEXT_2));
            }
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, CANVAS_EDGE);
        let ctx = ui.ctx().clone();
        let mut open = None;
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(ERect::from_center_size(rect.center(), Vec2::new(460.0, 300.0))),
            |ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Open a board").font(semibold(22.0)).color(TEXT));
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(
                            "A Lamina project folder, or a KiCad, EasyEDA or Gerber file (the viewer imports it). \
                             The viewer draws the board again when the agent changes it. Ctrl+O also opens a file. \
                             You can also drop folders or files on the window.",
                        )
                        .font(body(13.5))
                        .color(TEXT_2),
                    );
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if widgets::primary_button(ui, "Open board file…", true) {
                            self.request = Some(OpenRequest::PickFile);
                        }
                        ui.add_space(6.0);
                        if ui.button("Open project folder…").clicked() {
                            self.request = Some(OpenRequest::PickFolder);
                        }
                    });
                    ui.add_space(10.0);
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.open_path)
                            .hint_text("or type a path and press Enter")
                            .desired_width(f32::INFINITY)
                            .font(mono(13.0)),
                    );
                    if edit.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && !self.open_path.trim().is_empty()
                    {
                        open = Some(PathBuf::from(self.open_path.trim()));
                    }
                    if let Some(e) = &self.error {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(e).font(body(12.5)).color(ERROR));
                    }
                });
            },
        );
        let _ = ctx;
        if let Some(p) = open {
            self.request = Some(OpenRequest::Path(p));
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui, rs: &egui_wgpu::RenderState) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let w = (rect.width() * ppp).round().max(1.0) as u32;
        let h = (rect.height() * ppp).round().max(1.0) as u32;
        let size_px = Vec2::new(w as f32, h as f32);
        self.last_size_px = size_px;
        if self.gpu.is_none() {
            return;
        }

        let resized = self
            .target
            .as_ref()
            .is_none_or(|t| t.width != w || t.height != h);
        if resized {
            let t = self.gpu.as_ref().unwrap().target(w, h);
            let mut r = rs.renderer.write();
            match self.tex {
                None => {
                    self.tex =
                        Some(r.register_native_texture(&rs.device, &t.resolve_view, wgpu_filter()))
                }
                Some(id) => r.update_egui_texture_from_wgpu_texture(
                    &rs.device,
                    &t.resolve_view,
                    wgpu_filter(),
                    id,
                ),
            }
            self.target = Some(t);
            self.dirty = true;
        }
        // If the window or pane changes size and the view is as fitted, fit again.
        if let Some((c, p, s)) = self.fit_2d_at {
            if s != size_px && c == self.center && p == self.ppm {
                self.fitted = false;
            }
        }
        if let Some((cam, s)) = self.fit_3d_at {
            let same = cam.target == self.cam.target
                && cam.yaw == self.cam.yaw
                && cam.pitch == self.cam.pitch
                && cam.distance == self.cam.distance;
            if s != size_px && same {
                self.cam_fitted = false;
            }
        }
        if !self.fitted {
            self.fit_2d(size_px);
        }
        if !self.cam_fitted {
            self.fit_3d(size_px);
        }

        // Input.
        let (scroll, zoom_gesture, hover_pos, keys) = ui.input(|i| {
            (
                i.smooth_scroll_delta.y,
                i.zoom_delta(),
                i.pointer.hover_pos(),
                (
                    i.key_pressed(egui::Key::F),
                    i.key_pressed(egui::Key::Num1),
                    i.key_pressed(egui::Key::Num2),
                ),
            )
        });
        let focus_free = self.focused && !ui.ctx().egui_wants_keyboard_input();
        self.global_keys(ui);
        if focus_free {
            if keys.1 {
                self.mode = Mode::TwoD;
                self.dirty = true;
            }
            if keys.2 {
                self.mode = Mode::ThreeD;
                self.dirty = true;
            }
            if keys.0 && self.mode == Mode::ThreeD {
                self.fit_3d(size_px);
            }
        }
        let inside = hover_pos.is_some_and(|p| rect.contains(p));
        let xf = Xf {
            rect,
            center: self.center,
            ppm_pt: self.ppm / ppp as f64,
        };
        match self.mode {
            Mode::TwoD => {
                if inside && (scroll != 0.0 || zoom_gesture != 1.0) {
                    let pos = hover_pos.unwrap();
                    let before = xf.to_board(pos);
                    let factor = (scroll as f64 * 0.0018).exp() * zoom_gesture as f64;
                    self.ppm = (self.ppm * factor).clamp(0.5, 4000.0);
                    let xf2 = Xf {
                        rect,
                        center: self.center,
                        ppm_pt: self.ppm / ppp as f64,
                    };
                    let after = xf2.to_board(pos);
                    self.center[0] += before.x - after.x;
                    self.center[1] += before.y - after.y;
                    self.dirty = true;
                }
                // Pan with the right or middle button, or Space + left drag.
                let space = ui.input(|i| i.key_down(egui::Key::Space))
                    && !ui.ctx().egui_wants_keyboard_input();
                let panning = resp.dragged_by(egui::PointerButton::Secondary)
                    || resp.dragged_by(egui::PointerButton::Middle)
                    || (space && resp.dragged_by(egui::PointerButton::Primary));
                if panning {
                    let d = resp.drag_delta();
                    self.center[0] -= d.x as f64 / xf.ppm_pt;
                    self.center[1] += d.y as f64 / xf.ppm_pt;
                    self.dirty = true;
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                let xf = Xf {
                    rect,
                    center: self.center,
                    ppm_pt: self.ppm / ppp as f64,
                };
                self.mm_per_px = 1.0 / xf.ppm_pt;
                self.cursor_mm = hover_pos
                    .filter(|p| rect.contains(*p))
                    .map(|p| xf.to_board(p));
                let busy = !matches!(self.gesture, Gesture::None) || panning;
                let hover = if busy {
                    None
                } else {
                    self.cursor_mm.and_then(|p| self.pick(p))
                };
                if hover != self.hover {
                    self.hover = hover;
                }
                if !space {
                    self.edit_input(ui, &resp, &xf, size_px);
                }
                if std::mem::take(&mut self.fit_pending) {
                    self.fit_2d(size_px);
                }
                if self.tool != Tool::Select || self.placing.is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                } else if matches!(self.gesture, Gesture::Drag { .. } | Gesture::Grab { .. }) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
                }
            }
            Mode::ThreeD => {
                self.cursor_mm = None;
                self.hover = None;
                if inside && (scroll != 0.0 || zoom_gesture != 1.0) {
                    self.cam.distance = (self.cam.distance * (-(scroll * 0.0018)).exp()
                        / zoom_gesture)
                        .clamp(2.0, 5000.0);
                    self.dirty = true;
                }
                if resp.dragged_by(egui::PointerButton::Primary) {
                    let d = resp.drag_delta();
                    self.cam.yaw -= d.x * 0.35;
                    self.cam.pitch = (self.cam.pitch + d.y * 0.35).clamp(-89.0, 89.9);
                    self.dirty = true;
                }
                if resp.dragged_by(egui::PointerButton::Secondary)
                    || resp.dragged_by(egui::PointerButton::Middle)
                {
                    let d = resp.drag_delta();
                    let k = self.cam.distance * 0.0016;
                    let (sy, cy) = self.cam.yaw.to_radians().sin_cos();
                    let (sp, cp) = self.cam.pitch.to_radians().sin_cos();
                    // The right and up vectors of the camera. A drag moves the target in
                    // the view plane.
                    let fwd = [-cp * cy, -cp * sy, -sp];
                    let right = {
                        let r = [fwd[1], -fwd[0], 0.0];
                        let l = (r[0] * r[0] + r[1] * r[1]).sqrt().max(1e-6);
                        [r[0] / l, r[1] / l, 0.0]
                    };
                    let up = [
                        right[1] * fwd[2] - right[2] * fwd[1],
                        right[2] * fwd[0] - right[0] * fwd[2],
                        right[0] * fwd[1] - right[1] * fwd[0],
                    ];
                    for i in 0..3 {
                        self.cam.target[i] += (-right[i] * d.x + up[i] * d.y) * k;
                    }
                    self.dirty = true;
                }
                if resp.double_clicked() {
                    self.fit_3d(size_px);
                }
            }
        }

        if self.dirty {
            let gpu = self.gpu.as_ref().unwrap();
            if let Some(t) = &self.target {
                match self.mode {
                    Mode::TwoD => {
                        if let Some(g) = &self.g2d {
                            let vis = |l: SceneLayer| self.visible.get(&l).copied().unwrap_or(true);
                            gpu.draw_2d(t, g, self.center, self.ppm, &vis);
                        }
                    }
                    Mode::ThreeD => {
                        if let Some(g) = &self.g3d {
                            gpu.draw_3d(t, g, &self.cam);
                        }
                    }
                }
            }
            self.dirty = false;
        }
        let painter = ui.painter_at(rect);
        if let Some(id) = self.tex {
            painter.image(
                id,
                rect,
                ERect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        if self.mode == Mode::TwoD {
            let xf = Xf {
                rect,
                center: self.center,
                ppm_pt: self.ppm / ppp as f64,
            };
            self.overlays(&painter, &xf);
            self.edit_overlays(&painter, &xf);
            self.measure_overlay(&painter, &xf);
            widgets::hint(&painter, rect, self.tool_hint());
            self.toolbar(ui, rect);
        } else {
            widgets::hint(
                &painter,
                rect,
                "Drag to orbit · right-drag to pan · scroll to zoom · F to fit",
            );
            // The Board menu works from 3D too.
            let bar = ERect::from_min_size(
                rect.left_top() + Vec2::new(12.0, 12.0),
                Vec2::new(200.0, 36.0),
            );
            let mut b = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(bar)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            egui::Frame::new()
                .fill(RAISED.gamma_multiply(0.97))
                .stroke(Stroke::new(1.0, HAIRLINE))
                .corner_radius(CornerRadius::same(9))
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(&mut b, |ui| self.board_menu(ui));
        }
        // Toasts show in both views. A "…" message stays while its work runs.
        let working = self.editor.as_ref().is_some_and(|e| e.busy > 0) || !self.bg.is_empty();
        if let Some((msg, ok, t)) = &self.toast {
            let held = working && msg.ends_with('…');
            let age = if held { 0.0 } else { t.elapsed().as_secs_f32() };
            if age < 4.0 {
                let a = if age > 3.4 { (4.0 - age) / 0.6 } else { 1.0 };
                widgets::toast(
                    &painter,
                    Pos2::new(rect.center().x, rect.top() + 72.0),
                    msg,
                    if *ok { OK } else { ERROR },
                    a,
                );
            }
        }
    }

    fn overlays(&self, painter: &egui::Painter, xf: &Xf) {
        let Some(l) = &self.loaded else { return };
        // Outline the net under the cursor in the accent colour.
        let focus = self.hover.as_ref();
        if let Some(net) = focus.and_then(|p| p.net()) {
            for it in &l.analysis.geometry.copper {
                if it.net.as_deref() == Some(net) && self.is_visible(SceneLayer::Copper(it.layer)) {
                    draw_prim(painter, xf, &it.prim, ACCENT, true);
                }
            }
        }
        if let Some(part) = focus.and_then(|p| p.part()) {
            let d = &l.data;
            if let (Some(pl), Some(fp)) = (
                d.placement.placements.get(part),
                d.parts
                    .parts
                    .get(part)
                    .and_then(|x| d.footprints.get(&x.footprint)),
            ) {
                if let Some(c) = lamina_geom::courtyard(fp, pl) {
                    let pts: Vec<Pos2> = c.into_iter().map(|p| xf.to_screen(p)).collect();
                    painter.add(egui::Shape::closed_line(
                        pts,
                        Stroke::new(1.2, ACCENT.gamma_multiply(0.55)),
                    ));
                }
            }
        }
        // Flash the items that the agent changed.
        for (r, t) in &self.flashes {
            let k = (t.elapsed().as_secs_f32() / 0.9).clamp(0.0, 1.0);
            let a = (1.0 - k).powi(2);
            let grow = 2.0 + 6.0 * k;
            let sr = xf.rect(r).expand(grow);
            painter.rect_filled(sr, CornerRadius::same(4), ACCENT.gamma_multiply(0.10 * a));
            painter.rect_stroke(
                sr,
                CornerRadius::same(4),
                Stroke::new(1.5, ACCENT.gamma_multiply(a)),
                StrokeKind::Outside,
            );
        }
        if let Some(p) = self.note_at {
            let s = xf.to_screen(p);
            painter.circle_stroke(s, 5.0, Stroke::new(1.5, ACCENT));
            painter.circle_filled(s, 1.8, ACCENT);
        }
        // Scale bar.
        let mm_per_100 = 100.0 / xf.ppm_pt;
        let nice = [0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0]
            .into_iter()
            .find(|v| *v >= mm_per_100 * 0.6)
            .unwrap_or(100.0);
        let len = (nice * xf.ppm_pt) as f32;
        let base = xf.rect.left_bottom() + Vec2::new(18.0, -18.0);
        painter.line_segment([base, base + Vec2::new(len, 0.0)], Stroke::new(1.5, TEXT_2));
        for x in [0.0, len] {
            painter.line_segment(
                [base + Vec2::new(x, -4.0), base + Vec2::new(x, 0.0)],
                Stroke::new(1.5, TEXT_2),
            );
        }
        painter.text(
            base + Vec2::new(len + 8.0, -1.0),
            Align2::LEFT_CENTER,
            format!("{nice} mm"),
            mono(11.0),
            TEXT_2,
        );
    }
}

fn wgpu_filter() -> egui_wgpu::wgpu::FilterMode {
    egui_wgpu::wgpu::FilterMode::Linear
}

fn part_rows(d: &ProjectData, refdes: &str, rows: &mut Vec<(&str, String)>) {
    if let Some(p) = d.parts.parts.get(refdes) {
        if let Some(v) = &p.value {
            rows.push(("value", v.clone()));
        }
        if let Some(m) = &p.mpn {
            rows.push(("mpn", m.clone()));
        }
        if let Some(m) = &p.manufacturer {
            rows.push(("maker", m.clone()));
        }
        if let Some(c) = &p.lcsc {
            rows.push(("lcsc", c.clone()));
        }
        rows.push(("footprint", p.footprint.clone()));
        if let Some(g) = &p.group {
            rows.push(("group", g.clone()));
        }
    }
    if let Some(pl) = d.placement.placements.get(refdes) {
        rows.push(("at", format!("{:.3}, {:.3}", pl.x, pl.y)));
        rows.push(("rotation", format!("{}°", pl.rotation)));
        rows.push(("side", pl.side.as_str().to_string()));
    }
}

/// The end of a long path. It counts characters, so it never cuts a non-ASCII
/// folder name (`Masaüstü`) in a character.
fn short_path(p: &Path) -> String {
    let s = p.display().to_string();
    let n = s.chars().count();
    if n > 48 {
        format!("…{}", s.chars().skip(n - 46).collect::<String>())
    } else {
        s
    }
}

/// The window: several boards side by side. Side panels, the status bar
/// and keyboard shortcuts belong to the focused board.
struct Shell {
    boards: Vec<App>,
    focus: usize,
    three_d: bool,
    add_path: String,
    add_error: Option<(String, Instant)>,
    /// File dialogs and imports that run in the background.
    jobs: Vec<Job>,
    /// Project folders to open on the next frame.
    pending_open: Vec<PathBuf>,
    title: String,
    remembered: Vec<PathBuf>,
    panes_seen: usize,
}

fn remembered_boards() -> Vec<PathBuf> {
    config_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|s| {
            s.lines()
                .map(|l| PathBuf::from(l.trim()))
                .filter(|p| ProjectData::is_project(p))
                .collect()
        })
        .unwrap_or_default()
}

/// True if two paths name the same folder, also if they are written differently.
fn same_path(a: &Path, b: &Path) -> bool {
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    abs(a) == abs(b)
}

/// The project root of a dropped folder, or of a file in a project folder.
fn project_root(p: &Path) -> Option<PathBuf> {
    if ProjectData::is_project(p) {
        return Some(p.to_path_buf());
    }
    p.parent()
        .filter(|d| ProjectData::is_project(d))
        .map(Path::to_path_buf)
}

fn gpu_of(rs: Option<&egui_wgpu::RenderState>) -> Option<Gpu> {
    rs.map(|rs| Gpu::new(rs.device.clone(), rs.queue.clone()))
}

impl Shell {
    fn new(cc: &eframe::CreationContext<'_>, roots: Vec<PathBuf>, three_d: bool) -> Shell {
        style::install(&cc.egui_ctx);
        let roots = if roots.is_empty() {
            remembered_boards()
        } else {
            roots
        };
        let mut s = Shell {
            boards: Vec::new(),
            focus: 0,
            three_d,
            add_path: String::new(),
            add_error: None,
            jobs: Vec::new(),
            pending_open: Vec::new(),
            title: String::new(),
            remembered: Vec::new(),
            panes_seen: 0,
        };
        let rs = cc.wgpu_render_state.as_ref();
        for r in roots {
            if ProjectData::is_project(&r) {
                s.add(r, &cc.egui_ctx, rs);
            } else {
                // A board file or a file in a project. Import or resolve it in
                // the background, as with files that open later.
                s.request(OpenRequest::Path(r), &cc.egui_ctx);
            }
        }
        if s.boards.is_empty() {
            s.boards
                .push(App::new(gpu_of(rs), None, three_d, &cc.egui_ctx));
        }
        s.focus = 0;
        s
    }

    fn add(&mut self, root: PathBuf, ctx: &egui::Context, rs: Option<&egui_wgpu::RenderState>) {
        if let Some(i) = self
            .boards
            .iter()
            .position(|b| b.root.as_deref().is_some_and(|x| same_path(x, &root)))
        {
            self.focus = i;
            return;
        }
        let b = App::new(gpu_of(rs), Some(root), self.three_d, ctx);
        // The first real board replaces an empty placeholder pane.
        if self.boards.len() == 1 && self.boards[0].root.is_none() {
            self.boards.clear();
        }
        self.boards.push(b);
        self.focus = self.boards.len() - 1;
    }

    fn close(&mut self, i: usize, rs: Option<&egui_wgpu::RenderState>, ctx: &egui::Context) {
        let b = self.boards.remove(i);
        if let (Some(id), Some(rs)) = (b.tex, rs) {
            rs.renderer.write().free_texture(&id);
        }
        if self.boards.is_empty() {
            self.boards
                .push(App::new(gpu_of(rs), None, self.three_d, ctx));
        }
        self.focus = self.focus.min(self.boards.len() - 1);
    }

    /// Make the window title and the list of remembered boards match the open boards.
    fn sync_title_and_memory(&mut self, ctx: &egui::Context) {
        let names: Vec<String> = self
            .boards
            .iter()
            .filter(|b| b.root.is_some())
            .map(App::tab_name)
            .collect();
        let title = if names.is_empty() {
            "Lamina".to_string()
        } else {
            format!("{} - Lamina", names.join(" · "))
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        let roots: Vec<PathBuf> = self.boards.iter().filter_map(|b| b.root.clone()).collect();
        if roots != self.remembered && !roots.is_empty() {
            if let Some(f) = config_file() {
                let _ = std::fs::create_dir_all(f.parent().unwrap());
                let text: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
                let _ = std::fs::write(f, text.join("\n"));
            }
            self.remembered = roots;
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, rs: Option<&egui_wgpu::RenderState>) {
        let ctx = ui.ctx().clone();
        let mut close = None;
        ui.horizontal_centered(|ui| {
            widgets::wordmark(ui);
            ui.add_space(14.0);
            let many = self.boards.len() > 1;
            for (i, b) in self.boards.iter().enumerate() {
                if b.root.is_none() {
                    continue;
                }
                let text = egui::RichText::new(b.tab_name())
                    .font(semibold(13.0))
                    .color(if i == self.focus { TEXT } else { TEXT_2 });
                let r = ui
                    .add(egui::Button::selectable(i == self.focus && many, text))
                    .on_hover_text(
                        b.root
                            .as_ref()
                            .map(|r| r.display().to_string())
                            .unwrap_or_default(),
                    );
                ui.painter().circle_filled(
                    Pos2::new(r.rect.right() - 3.0, r.rect.top() + 4.0),
                    2.5,
                    b.health(),
                );
                if r.clicked() {
                    self.focus = i;
                }
                if r.middle_clicked() {
                    close = Some(i);
                }
                ui.add_space(2.0);
            }
            let mut req = None;
            ui.menu_button(
                egui::RichText::new("+").font(semibold(15.0)).color(TEXT_2),
                |ui| {
                    if ui.button("Open board file…    Ctrl+O").clicked() {
                        req = Some(OpenRequest::PickFile);
                        ui.close();
                    }
                    if ui.button("Open project folder…").clicked() {
                        req = Some(OpenRequest::PickFolder);
                        ui.close();
                    }
                    ui.separator();
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.add_path)
                            .hint_text("path, then Enter")
                            .desired_width(300.0)
                            .font(mono(12.0)),
                    );
                    if edit.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && !self.add_path.trim().is_empty()
                    {
                        req = Some(OpenRequest::Path(PathBuf::from(self.add_path.trim())));
                        self.add_path.clear();
                        ui.close();
                    }
                    let recent: Vec<PathBuf> = remembered_boards()
                        .into_iter()
                        .filter(|r| {
                            !self
                                .boards
                                .iter()
                                .any(|b| b.root.as_deref().is_some_and(|x| same_path(x, r)))
                        })
                        .collect();
                    if !recent.is_empty() {
                        ui.separator();
                        ui.label(egui::RichText::new("Recent").font(body(11.5)).color(TEXT_3));
                        for r in recent {
                            if ui.button(short_path(&r)).clicked() {
                                req = Some(OpenRequest::Path(r));
                                ui.close();
                            }
                        }
                    }
                },
            )
            .response
            .on_hover_text("Open another board next to this one. You can also drop files and folders on the window.");
            if let Some(r) = req {
                self.request(r, &ctx);
            }
            for j in &self.jobs {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(egui::RichText::new(&j.label).font(body(12.0)).color(TEXT_2));
            }
            if let Some((e, t)) = &self.add_error {
                if t.elapsed() < Duration::from_secs(8) {
                    // A long import error stays on one line. Hover over it to see the full text.
                    let short: String = if e.chars().count() > 90 {
                        format!("{}…", e.chars().take(89).collect::<String>())
                    } else {
                        e.clone()
                    };
                    ui.label(egui::RichText::new(short).font(body(12.0)).color(ERROR))
                        .on_hover_text(e);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.boards[self.focus].chips(ui);
            });
        });
        if let Some(i) = close {
            self.close(i, rs, &ctx);
        }
    }
}

/// Something the user asked to open.
pub(crate) enum OpenRequest {
    PickFile,
    PickFolder,
    Path(PathBuf),
}

/// A file dialog or an import that runs off the UI thread.
struct Job {
    label: String,
    /// The file to import. `None` for a dialog.
    path: Option<PathBuf>,
    rx: std::sync::mpsc::Receiver<JobDone>,
}

enum JobDone {
    /// The dialog closed. `None` if the user cancelled it.
    Picked(Option<PathBuf>),
    Opened(Result<PathBuf, String>),
}

impl Shell {
    fn spawn(
        &mut self,
        label: String,
        path: Option<PathBuf>,
        ctx: &egui::Context,
        f: impl FnOnce() -> JobDone + Send + 'static,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
        self.jobs.push(Job { label, path, rx });
    }

    fn request(&mut self, r: OpenRequest, ctx: &egui::Context) {
        // One dialog at a time, and one import per file.
        let dialog_open = self.jobs.iter().any(|j| j.path.is_none());
        if dialog_open && matches!(r, OpenRequest::PickFile | OpenRequest::PickFolder) {
            return;
        }
        match r {
            OpenRequest::PickFile => self.spawn(String::new(), None, ctx, || {
                JobDone::Picked(
                    rfd::FileDialog::new()
                        .set_title("Open a board")
                        .add_filter(
                            "Boards",
                            &["json", "kicad_pcb", "zip", "gbr", "gtl", "gbl", "drl"],
                        )
                        .add_filter("Lamina project (board.json)", &["json"])
                        .add_filter("KiCad board", &["kicad_pcb"])
                        .add_filter("EasyEDA board", &["json"])
                        .add_filter("Gerbers", &["zip", "gbr", "gtl", "gbl", "drl"])
                        .add_filter("All files", &["*"])
                        .pick_file(),
                )
            }),
            OpenRequest::PickFolder => self.spawn(String::new(), None, ctx, || {
                JobDone::Picked(
                    rfd::FileDialog::new()
                        .set_title("Open a Lamina project or Gerber folder")
                        .pick_folder(),
                )
            }),
            OpenRequest::Path(p) => {
                if let Some(r) = project_root(&p) {
                    // A project, or a file in a project. An import is not necessary.
                    if lamina_wiring::board_file_kind(&p).is_none()
                        || ProjectData::is_project(&p)
                        || p.file_name().is_some_and(|n| n == "board.json")
                    {
                        self.open_root(r, ctx);
                        return;
                    }
                }
                if lamina_wiring::board_file_kind(&p).is_none() {
                    self.add_error = Some((
                        format!("{} is not a Lamina project or a board file", p.display()),
                        Instant::now(),
                    ));
                    return;
                }
                if self
                    .jobs
                    .iter()
                    .any(|j| j.path.as_deref() == Some(p.as_path()))
                {
                    return;
                }
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let key = Some(p.clone());
                self.spawn(format!("Import {name}…"), key, ctx, move || {
                    JobDone::Opened(
                        lamina_wiring::open_board_path(&p)
                            .map(|(dir, _)| dir)
                            .map_err(|e| e.to_string()),
                    )
                });
            }
        }
    }

    fn open_root(&mut self, r: PathBuf, ctx: &egui::Context) {
        self.pending_open.push(r);
        ctx.request_repaint();
    }

    fn finish_jobs(&mut self, ctx: &egui::Context, rs: Option<&egui_wgpu::RenderState>) {
        let mut done = Vec::new();
        self.jobs.retain(|j| match j.rx.try_recv() {
            Ok(d) => {
                done.push(d);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        });
        for d in done {
            match d {
                JobDone::Picked(Some(p)) => self.request(OpenRequest::Path(p), ctx),
                JobDone::Picked(None) => {}
                JobDone::Opened(Ok(dir)) => self.add(dir, ctx, rs),
                JobDone::Opened(Err(e)) => self.add_error = Some((e, Instant::now())),
            }
        }
        for r in std::mem::take(&mut self.pending_open) {
            self.add(r, ctx, rs);
        }
    }
}

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let rs = frame.wgpu_render_state().cloned();
        for b in &mut self.boards {
            b.poll();
        }
        // Folders dropped on the window open next to the others.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        for p in dropped {
            self.request(OpenRequest::Path(p), &ctx);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O)) {
            self.request(OpenRequest::PickFile, &ctx);
        }
        self.finish_jobs(&ctx, rs.as_ref());
        let asked: Vec<OpenRequest> = self
            .boards
            .iter_mut()
            .filter_map(|b| b.request.take())
            .collect();
        for r in asked {
            self.request(r, &ctx);
        }
        // A new pane layout changes the size of each canvas. Fit them all again.
        if self.boards.len() != self.panes_seen {
            self.panes_seen = self.boards.len();
            for b in &mut self.boards {
                b.fitted = false;
                b.cam_fitted = false;
            }
        }
        for (i, b) in self.boards.iter_mut().enumerate() {
            b.focused = i == self.focus;
        }

        egui::Panel::top("top")
            .exact_size(44.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(14, 0)),
            )
            .show(ui, |ui| self.top_bar(ui, rs.as_ref()));
        let f = self.focus;
        egui::Panel::bottom("status")
            .exact_size(26.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(14, 0)),
            )
            .show(ui, |ui| self.boards[f].status_bar(ui));
        if self.boards[f].loaded.is_some() {
            egui::Panel::left("layers")
                .exact_size(236.0)
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .fill(PANEL)
                        .inner_margin(egui::Margin::same(12)),
                )
                .show(ui, |ui| self.boards[f].layer_panel(ui));
            egui::Panel::right("inspector")
                .default_size(312.0)
                .size_range(260.0..=520.0)
                .resizable(true)
                .frame(
                    egui::Frame::new()
                        .fill(PANEL)
                        .inner_margin(egui::Margin::same(12)),
                )
                .show(ui, |ui| self.boards[f].inspector(ui));
        }
        let mut close = None;
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let area = ui.max_rect();
            let n = self.boards.len();
            let gap = if n > 1 { 2.0 } else { 0.0 };
            let w = (area.width() - gap * (n as f32 - 1.0)) / n as f32;
            let header = 34.0;
            let (pressed, press_at) = ctx.input(|i| {
                (
                    i.pointer.any_pressed(),
                    i.pointer.interact_pos().or(i.pointer.latest_pos()),
                )
            });
            for (i, b) in self.boards.iter_mut().enumerate() {
                let x = area.left() + i as f32 * (w + gap);
                let pane =
                    ERect::from_min_size(Pos2::new(x, area.top()), Vec2::new(w, area.height()));
                if pressed && press_at.is_some_and(|p| pane.contains(p)) {
                    self.focus = i;
                }
                let head = ERect::from_min_size(pane.min, Vec2::new(w, header));
                let body_r = ERect::from_min_max(Pos2::new(pane.left(), head.bottom()), pane.max);
                ui.painter().rect_filled(head, 0.0, PANEL);
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(head)
                        .id_salt(("pane-head", i)),
                    |ui| {
                        if b.pane_header(ui, n > 1) {
                            close = Some(i);
                        }
                    },
                );
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(body_r).id_salt(("pane", i)),
                    |ui| {
                        if b.loaded.is_some() {
                            if let Some(rs) = &rs {
                                b.canvas(ui, rs);
                                let ppp = ui.ctx().pixels_per_point();
                                b.canvas_panels(ui, body_r, body_r.size() * ppp);
                            }
                        } else {
                            b.empty_state(ui);
                        }
                    },
                );
                if n > 1 && i == self.focus {
                    ui.painter().rect_stroke(
                        pane.shrink(0.5),
                        0.0,
                        Stroke::new(1.0, ACCENT.gamma_multiply(0.7)),
                        StrokeKind::Inside,
                    );
                }
            }
        });
        if let Some(i) = close {
            self.close(i, rs.as_ref(), &ctx);
        }
        self.sync_title_and_memory(&ctx);

        let mut animating = false;
        let mut recent = false;
        for b in &mut self.boards {
            animating |= b.animating();
            recent |= b.recently_reloaded();
        }
        if animating || !self.jobs.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if recent {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}

/// A GUI-subsystem exe has no console. Print to the console that started it.
fn attach_console() {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn AttachConsole(pid: u32) -> i32;
        }
        // ATTACH_PARENT_PROCESS
        unsafe {
            AttachConsole(u32::MAX);
        }
    }
}

const USAGE: &str = "Usage: lamina-viewer [--3d] [BOARD_DIR | BOARD_FILE]...

Opens Lamina project folders, or KiCad, EasyEDA and Gerber files, side by
side in one window. The viewer imports board files when it opens them.
Without arguments, it opens the boards that were open last time. To open
more boards, click + in the top bar or drop a folder on the window.

  --3d  start in the 3D view";

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a.starts_with('-')) {
        attach_console();
    }
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("lamina-viewer {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let three_d = args.iter().any(|a| a == "--3d");
    let args: Vec<String> = args.into_iter().filter(|a| a != "--3d").collect();
    if let Some(bad) = args.iter().find(|a| a.starts_with('-')) {
        eprintln!(
            "{bad} is not a known option.

{USAGE}"
        );
        std::process::exit(2);
    }
    let roots: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lamina")
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon.png"))
                    .expect("icon.png is a valid PNG"),
            )
            .with_inner_size([1480.0, 920.0])
            .with_min_inner_size([760.0, 480.0])
            .with_maximized(true),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Lamina",
        options,
        Box::new(move |cc| Ok(Box::new(Shell::new(cc, roots, three_d)))),
    )
}

#[cfg(test)]
mod path_tests {
    #[test]
    fn short_path_keeps_whole_characters() {
        let p = std::path::PathBuf::from(format!("C:/Users/oğuz/Masaüstü/{}", "ü".repeat(60)));
        let s = super::short_path(&p);
        assert_eq!(s.chars().count(), 47);
        assert!(s.starts_with('…'));
        assert_eq!(super::short_path(std::path::Path::new("C:/a")), "C:/a");
    }
}
