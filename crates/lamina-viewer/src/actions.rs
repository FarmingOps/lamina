//! Board-wide actions (autoroute, rip-up, exports), find, measure and the
//! shortcut sheet.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, Align2, Color32, Pos2, Rect as ERect, Stroke, Vec2};
use lamina_model::{Point, ProjectData};

use crate::edits::{self, Sel};
use crate::style::*;
use crate::{App, Xf};

/// The result of a background job: the message and the success flag.
pub type BgDone = (String, bool);

/// Ctrl+F search over parts and nets.
#[derive(Default)]
pub struct Find {
    pub text: String,
    /// The result that the keyboard highlights.
    pub cursor: usize,
    pub just_opened: bool,
}

#[derive(Clone)]
pub enum Hit {
    Part { refdes: String, label: String },
    Net { name: String, label: String },
}

impl Hit {
    fn label(&self) -> &str {
        match self {
            Hit::Part { label, .. } | Hit::Net { label, .. } => label,
        }
    }
}

/// The state of the measure tool: the first point, and the second point after it is set.
#[derive(Default, Clone, Copy)]
pub struct Measure {
    pub a: Option<Point>,
    pub b: Option<Point>,
}

/// The nets that touch the pads of the given parts.
fn nets_of_parts(d: &ProjectData, refs: &[String]) -> Vec<String> {
    let mut v: Vec<String> = d
        .nets
        .nets
        .iter()
        .filter(|(_, n)| n.pads.iter().any(|p| refs.contains(&p.part)))
        .map(|(k, _)| k.clone())
        .collect();
    v.sort();
    v
}

pub fn search(d: &ProjectData, q: &str) -> Vec<Hit> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(u8, Hit)> = Vec::new();
    for (r, p) in &d.parts.parts {
        let value = p.value.clone().unwrap_or_default();
        let hay = format!("{} {} {}", r, value, p.footprint).to_lowercase();
        if hay.contains(&q) {
            let rank = if r.to_lowercase() == q {
                0
            } else if r.to_lowercase().starts_with(&q) {
                1
            } else {
                3
            };
            out.push((
                rank,
                Hit::Part {
                    refdes: r.clone(),
                    label: format!("{r}  {value}  {}", p.footprint),
                },
            ));
        }
    }
    for (name, n) in &d.nets.nets {
        if name.to_lowercase().contains(&q) {
            let rank = if name.to_lowercase() == q { 0 } else { 2 };
            out.push((
                rank,
                Hit::Net {
                    name: name.clone(),
                    label: format!("net {name}  ({} pads)", n.pads.len()),
                },
            ));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.label().cmp(b.1.label())));
    out.into_iter().map(|(_, h)| h).take(40).collect()
}

impl App {
    fn spawn_bg(&mut self, f: impl FnOnce() -> BgDone + Send + 'static) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        self.bg.push(rx);
        self.say("Busy…", true);
    }

    /// Show the results of finished background jobs as toasts.
    pub(crate) fn poll_bg(&mut self) {
        let mut done = Vec::new();
        self.bg.retain(|rx: &Receiver<BgDone>| match rx.try_recv() {
            Ok(d) => {
                done.push(d);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
        for (m, ok) in done {
            self.say(m, ok);
        }
    }

    /// Route the given nets. If `None`, route all unrouted nets. The ratsnest
    /// does not include the connections that a pour makes, so a net with a pour
    /// gets only the connections that the pour does not make. You can undo this
    /// as any other edit.
    pub(crate) fn autoroute(&mut self, nets: Option<Vec<String>>) {
        if self.loaded.is_none() {
            return;
        }
        self.say("Autoroute…", true);
        self.commit(move |d| {
            let opt = lamina_route::RouteOptions {
                nets,
                ..Default::default()
            };
            let out = lamina_route::autoroute(d, &opt)?;
            if out.tracks.is_empty() && out.vias.is_empty() {
                return Err(if out.failed.is_empty() {
                    "There is nothing to route. All connections are made.".into()
                } else {
                    format!("Cannot route {} connection(s)", out.failed.len())
                });
            }
            let n = out.routed.len();
            d.routing.tracks.extend(out.tracks);
            d.routing.vias.extend(out.vias);
            Ok(if out.failed.is_empty() {
                format!("Autorouted {n} connection(s)")
            } else {
                format!(
                    "Autorouted {n} connection(s). {} are not routed: {}",
                    out.failed.len(),
                    out.failed
                        .iter()
                        .take(3)
                        .map(|f| format!("{} {}–{}", f.net, f.from, f.to))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
        });
    }

    /// Remove tracks and vias of the given nets (all nets when `None`).
    pub(crate) fn rip_up(&mut self, nets: Option<Vec<String>>) {
        self.commit(move |d| {
            let hit = |n: &str| nets.as_ref().is_none_or(|v| v.iter().any(|x| x == n));
            let (t, v) = (d.routing.tracks.len(), d.routing.vias.len());
            d.routing.tracks.retain(|x| !hit(&x.net));
            d.routing.vias.retain(|x| !hit(&x.net));
            let removed = t - d.routing.tracks.len() + v - d.routing.vias.len();
            if removed == 0 {
                return Err("There are no tracks or vias to rip up".into());
            }
            Ok(format!("Ripped up {removed} track(s) and via(s)"))
        });
    }

    fn export_fab(&mut self, root: PathBuf, force: bool) {
        self.spawn_bg(move || {
            let r = lamina_core::Project::open_shared(&root)
                .and_then(|p| p.export(lamina_core::Export { dir: None, force }));
            match r {
                Ok(x) => (
                    format!(
                        "Wrote {} fab files to {}",
                        x.files.len(),
                        root.join("output").display()
                    ),
                    true,
                ),
                // The engine error is for agents. Tell the user what to click.
                Err(e) => (
                    format!("The export failed: {e}").replace(
                        "fix them or pass force: true",
                        "fix them or use Board › Export fab files anyway",
                    ),
                    false,
                ),
            }
        });
    }

    fn export_step(&mut self, root: PathBuf) {
        self.spawn_bg(move || {
            let r = lamina_core::Project::open_shared(&root).and_then(|p| {
                p.export_step(lamina_core::ExportStep {
                    path: None,
                    vias: false,
                    board_only: false,
                })
            });
            match r {
                Ok(x) => (format!("Wrote {}", x.path), true),
                Err(e) => (format!("The STEP export failed: {e}"), false),
            }
        });
    }

    fn export_kicad(&mut self, root: PathBuf) {
        self.spawn_bg(move || {
            let r = lamina_core::Project::open_shared(&root).and_then(|p| {
                p.export_board(lamina_core::ExportBoard {
                    format: "kicad".into(),
                    path: None,
                })
            });
            match r {
                Ok(x) => (format!("Wrote {}", x.path), true),
                Err(e) => (format!("The KiCad export failed: {e}"), false),
            }
        });
    }

    /// The Board menu in the toolbar.
    pub(crate) fn board_menu(&mut self, ui: &mut egui::Ui) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let refs: Vec<String> = self
            .sel
            .iter()
            .filter_map(|s| match s {
                Sel::Part(r) => Some(r.clone()),
                _ => None,
            })
            .collect();
        let sel_nets = self.selected_nets();
        let unrouted = self
            .loaded
            .as_ref()
            .map(|l| l.analysis.ratsnest.len())
            .unwrap_or(0);
        ui.menu_button(egui::RichText::new("Board").font(body(12.5)).color(TEXT), |ui| {
            ui.set_min_width(260.0);
            if ui
                .button(format!("Autoroute all   ({unrouted} unrouted)"))
                .on_hover_text("Routes all open connections. For a net with a pour, it routes only where the pour does not reach. Ctrl+Z undoes it.")
                .clicked()
            {
                self.autoroute(None);
                ui.close();
            }
            let nets = if refs.is_empty() {
                sel_nets.clone()
            } else {
                self.loaded
                    .as_ref()
                    .map(|l| nets_of_parts(&l.data, &refs))
                    .unwrap_or_default()
            };
            if ui
                .add_enabled(!nets.is_empty(), egui::Button::new("Autoroute selection"))
                .on_hover_text("Routes the nets of the selected parts or tracks.")
                .clicked()
            {
                self.autoroute(Some(nets.clone()));
                ui.close();
            }
            ui.separator();
            if ui
                .add_enabled(!nets.is_empty(), egui::Button::new("Rip up selection's nets"))
                .clicked()
            {
                self.rip_up(Some(nets));
                ui.close();
            }
            if ui.button("Rip up all tracks and vias").clicked() {
                self.rip_up(None);
                ui.close();
            }
            ui.separator();
            if ui
                .button("Export fab files (Gerbers, drill, BOM, CPL)")
                .on_hover_text("This is not possible while the board has DRC errors.")
                .clicked()
            {
                self.export_fab(root.clone(), false);
                ui.close();
            }
            let errors = self.loaded.as_ref().is_some_and(|l| {
                l.analysis
                    .violations
                    .iter()
                    .any(|v| v.severity == lamina_model::Severity::Error)
            });
            if errors
                && ui
                    .button("Export fab files anyway (ignore DRC errors)")
                    .clicked()
            {
                self.export_fab(root.clone(), true);
                ui.close();
            }
            if ui.button("Export STEP (board and parts)").clicked() {
                self.export_step(root.clone());
                ui.close();
            }
            if ui.button("Export KiCad board").clicked() {
                self.export_kicad(root.clone());
                ui.close();
            }
            ui.separator();
            if ui.button("Show project folder").clicked() {
                open_folder(&root);
                ui.close();
            }
            if ui.button("Keyboard shortcuts    F1").clicked() {
                self.help = true;
                ui.close();
            }
        });
    }

    /// Nets of the selected tracks and vias.
    fn selected_nets(&self) -> Vec<String> {
        let Some(l) = &self.loaded else {
            return Vec::new();
        };
        let r = &l.data.routing;
        let mut v: Vec<String> = self
            .sel
            .iter()
            .filter_map(|s| match s {
                Sel::Track { id, .. } => {
                    r.tracks.iter().find(|t| &t.id == id).map(|t| t.net.clone())
                }
                Sel::Via(id) => r.vias.iter().find(|x| &x.id == id).map(|x| x.net.clone()),
                _ => None,
            })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Select a search hit and bring it into view.
    fn go_to(&mut self, h: &Hit, size_px: Vec2) {
        let Some(l) = &self.loaded else { return };
        let d = &l.data;
        match h {
            Hit::Part { refdes, .. } => {
                if let Some(pl) = d.placement.placements.get(refdes) {
                    let at = pl.origin();
                    self.sel = vec![Sel::Part(refdes.clone())];
                    self.zoom_to(at, 12.0, size_px);
                } else {
                    self.say(format!("{refdes} is not placed yet"), false);
                }
            }
            Hit::Net { name, .. } => {
                let pts: Vec<Point> = d.nets.nets[name]
                    .pads
                    .iter()
                    .filter_map(|p| d.placement.placements.get(&p.part).map(|pl| pl.origin()))
                    .collect();
                self.sel = d
                    .routing
                    .tracks
                    .iter()
                    .filter(|t| &t.net == name)
                    .map(|t| Sel::Track {
                        id: t.id.clone(),
                        seg: 0,
                    })
                    .collect();
                if let Some(b) = lamina_model::Rect::from_points(pts) {
                    let c = Point::new((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0);
                    self.zoom_to(c, (b.width().max(b.height()) * 1.3).max(6.0), size_px);
                }
                self.say(format!("Net {name}"), true);
            }
        }
        self.dirty = true;
    }

    /// Floating panels over the canvas: find and the shortcut sheet.
    pub(crate) fn canvas_panels(&mut self, ui: &mut egui::Ui, rect: ERect, size_px: Vec2) {
        if self.find.is_some() {
            self.find_panel(ui, rect, size_px);
        }
        if self.help {
            self.help_panel(ui, rect);
        }
    }

    fn find_panel(&mut self, ui: &mut egui::Ui, rect: ERect, size_px: Vec2) {
        let Some(l) = &self.loaded else { return };
        let hits = search(&l.data, &self.find.as_ref().unwrap().text);
        let mut close = false;
        let mut go = None;
        let w = 380.0f32.min(rect.width() - 24.0);
        let area = ERect::from_min_size(
            Pos2::new(rect.center().x - w / 2.0, rect.top() + 56.0),
            Vec2::new(w, 34.0 + 22.0 * hits.len().min(10) as f32 + 12.0),
        );
        ui.painter().rect_filled(area, 9.0, RAISED);
        ui.painter().rect_stroke(
            area,
            9.0,
            Stroke::new(1.0, HAIRLINE),
            egui::StrokeKind::Inside,
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(area.shrink(8.0)), |ui| {
            let f = self.find.as_mut().unwrap();
            let edit = ui.add(
                egui::TextEdit::singleline(&mut f.text)
                    .hint_text("Find a part, value or net…   Enter to go · Esc to close")
                    .desired_width(f32::INFINITY)
                    .font(mono(13.0)),
            );
            if f.just_opened {
                edit.request_focus();
                f.just_opened = false;
            }
            if edit.changed() {
                f.cursor = 0;
            }
            let (down, up, enter, esc) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if down && !hits.is_empty() {
                f.cursor = (f.cursor + 1).min(hits.len() - 1);
            }
            if up {
                f.cursor = f.cursor.saturating_sub(1);
            }
            if esc {
                close = true;
            }
            if enter {
                if let Some(h) = hits.get(f.cursor) {
                    go = Some(h.clone());
                }
            }
            for (i, h) in hits.iter().take(10).enumerate() {
                let sel = i == f.cursor;
                let r = ui.add(egui::Button::selectable(
                    sel,
                    egui::RichText::new(h.label())
                        .font(mono(12.0))
                        .color(if sel { TEXT } else { TEXT_2 }),
                ));
                if r.clicked() {
                    go = Some(h.clone());
                }
            }
            if hits.is_empty() && !f.text.trim().is_empty() {
                ui.label(
                    egui::RichText::new("No match")
                        .font(body(12.0))
                        .color(TEXT_3),
                );
            }
        });
        if let Some(h) = go {
            self.go_to(&h, size_px);
            close = true;
        }
        if close {
            self.find = None;
        }
    }

    fn help_panel(&mut self, ui: &mut egui::Ui, rect: ERect) {
        const KEYS: &[(&str, &str)] = &[
            ("S", "Select"),
            ("X", "Route. V adds a via, / flips the bend"),
            ("V", "Via tool"),
            ("Z / K / O", "Pour / keep-out / outline"),
            ("D", "Measure (click two points)"),
            ("R / Shift+R", "Rotate selection"),
            ("F", "Flip selection, or fit the view"),
            ("M", "Move selection with the pointer"),
            ("L", "Lock or unlock"),
            ("Arrows", "Move by one grid step"),
            ("N / Shift+N", "Grid size"),
            ("PgUp / PgDn", "Active copper layer"),
            ("Del", "Delete selection"),
            ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
            ("Ctrl+A", "Select all parts"),
            ("Ctrl+F", "Find a part or net"),
            ("Ctrl+O", "Open a board or file"),
            ("1 / 2", "2D / 3D"),
            ("Home", "Fit the board"),
            ("Esc", "Cancel, then deselect"),
            ("F1", "This sheet"),
        ];
        let w = 420.0f32.min(rect.width() - 24.0);
        let h = 40.0 + KEYS.len() as f32 * 19.0;
        let area = ERect::from_center_size(rect.center(), Vec2::new(w, h));
        let p = ui.painter();
        p.rect_filled(area, 10.0, RAISED);
        p.rect_stroke(
            area,
            10.0,
            Stroke::new(1.0, HAIRLINE),
            egui::StrokeKind::Inside,
        );
        p.text(
            area.left_top() + Vec2::new(16.0, 14.0),
            Align2::LEFT_TOP,
            "Keyboard shortcuts",
            semibold(14.0),
            TEXT,
        );
        p.text(
            area.right_top() + Vec2::new(-16.0, 16.0),
            Align2::RIGHT_TOP,
            "F1 or Esc to close",
            body(11.5),
            TEXT_3,
        );
        for (i, (k, v)) in KEYS.iter().enumerate() {
            let y = area.top() + 40.0 + i as f32 * 19.0;
            p.text(
                Pos2::new(area.left() + 16.0, y),
                Align2::LEFT_TOP,
                *k,
                mono(12.0),
                ACCENT,
            );
            p.text(
                Pos2::new(area.left() + 150.0, y),
                Align2::LEFT_TOP,
                *v,
                body(12.5),
                TEXT_2,
            );
        }
        // The code for the other keys handles F1 (it toggles this sheet).
        if ui.input(|i| i.key_pressed(egui::Key::Escape) || i.pointer.any_pressed()) {
            self.help = false;
        }
    }

    /// Input for the measure tool: the first click starts, the second click sets
    /// the end, and a third click starts again.
    pub(crate) fn measure_input(&mut self, resp: &egui::Response, p: Point) {
        let at = self.measure_snap(p);
        if resp.clicked() {
            let m = &mut self.measure;
            match (m.a, m.b) {
                (Some(_), None) => m.b = Some(at),
                _ => {
                    m.a = Some(at);
                    m.b = None;
                }
            }
            self.dirty = true;
        }
    }

    /// Snap to a pad or via centre in a few pixels. If there is none, snap to the grid.
    fn measure_snap(&self, p: Point) -> Point {
        match self.magnet(p, None, None) {
            Some(m) => m.at,
            None => edits::snap_pt(p, self.grid()),
        }
    }

    pub(crate) fn measure_overlay(&self, painter: &egui::Painter, xf: &Xf) {
        if self.tool != crate::tools::Tool::Measure {
            return;
        }
        let Some(a) = self.measure.a else { return };
        let b = self
            .measure
            .b
            .or(self.cursor_mm.map(|p| self.measure_snap(p)))
            .unwrap_or(a);
        let (sa, sb) = (xf.to_screen(a), xf.to_screen(b));
        let c = Color32::from_rgb(255, 214, 102);
        painter.line_segment([sa, sb], Stroke::new(1.5, c));
        // The dx and dy legs, as thin lines.
        let corner = xf.to_screen(Point::new(b.x, a.y));
        painter.line_segment([sa, corner], Stroke::new(1.0, c.gamma_multiply(0.45)));
        painter.line_segment([corner, sb], Stroke::new(1.0, c.gamma_multiply(0.45)));
        for s in [sa, sb] {
            painter.circle_stroke(s, 4.0, Stroke::new(1.5, c));
        }
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len = (dx * dx + dy * dy).sqrt();
        let ang = dy.atan2(dx).to_degrees();
        let text = format!("{len:.3} mm   dx {dx:.3}   dy {dy:.3}   {ang:.1}°");
        let mid = Pos2::new((sa.x + sb.x) / 2.0, (sa.y + sb.y) / 2.0) + Vec2::new(10.0, -18.0);
        let galley = painter.layout_no_wrap(text, mono(12.0), TEXT);
        let r = ERect::from_min_size(mid, galley.size()).expand(5.0);
        painter.rect_filled(r, 5.0, RAISED);
        painter.rect_stroke(
            r,
            5.0,
            Stroke::new(1.0, c.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
        painter.galley(mid, galley, TEXT);
    }
}

/// Open a folder in the system file manager.
pub fn open_folder(p: &std::path::Path) {
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(p).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use lamina_model::*;

    #[test]
    fn search_ranks_exact_refdes_first() {
        let mut d = ProjectData::new("t");
        for (r, v) in [("R1", "10k"), ("R10", "1k"), ("U1", "R1-reg")] {
            let mut p = Part::new("fp");
            p.value = Some(v.into());
            d.parts.parts.insert(r.into(), p);
        }
        d.nets.nets.insert(
            "R1_OUT".into(),
            Net {
                pads: vec![],
                class: None,
            },
        );
        let h = search(&d, "r1");
        assert!(matches!(&h[0], Hit::Part { refdes, .. } if refdes == "R1"));
        assert!(
            h.iter()
                .any(|x| matches!(x, Hit::Net { name, .. } if name == "R1_OUT"))
        );
        assert_eq!(h.len(), 4);
        assert!(search(&d, "  ").is_empty());
    }

    #[test]
    fn poured_and_part_nets() {
        let mut d = ProjectData::new("t");
        d.nets.nets.insert(
            "A".into(),
            Net {
                pads: vec!["R1.1".parse().unwrap()],
                class: None,
            },
        );
        d.nets.nets.insert(
            "B".into(),
            Net {
                pads: vec!["R2.1".parse().unwrap()],
                class: None,
            },
        );
        assert_eq!(nets_of_parts(&d, &["R1".into()]), vec!["A".to_string()]);
    }
}
