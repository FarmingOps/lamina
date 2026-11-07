//! Watches the project folder and rebuilds scenes off the UI thread.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use lamina_drc::Analysis;
use lamina_model::{ProjectData, Rect};
use lamina_scene::{Scene2D, Scene3D};
use notify::{RecursiveMode, Watcher};

pub struct Loaded {
    pub data: ProjectData,
    pub analysis: Analysis,
    pub s2: Scene2D,
    pub s3: Scene3D,
    pub took: Duration,
    /// Board areas that differ from the previous load.
    pub changed: Vec<Rect>,
    /// Model files that changed on disk.
    pub models_changed: bool,
    /// Files that the design source generates. The editor does not change them.
    pub generated: Vec<String>,
}

pub enum Msg {
    Loaded(Box<Loaded>),
    Error(String),
}

/// Paths whose changes do not start a rebuild.
fn ignored(root: &Path, p: &Path) -> bool {
    let rel = p.strip_prefix(root).unwrap_or(p);
    let s = rel.to_string_lossy().replace('\\', "/");
    s.starts_with("cache/render")
        || s.starts_with(".git")
        || s.ends_with(".tmp")
        || s.ends_with(".lamina.lock")
        || s.ends_with(".mesh")
        || s.starts_with("cache/parts")
}

pub struct Live {
    pub rx: Receiver<Msg>,
    _watcher: notify::RecommendedWatcher,
}

impl Live {
    pub fn start(root: PathBuf, wake: impl Fn() + Send + 'static) -> Result<Live, String> {
        let (ev_tx, ev_rx) = channel::<(bool, bool)>();
        let r2 = root.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                let relevant: Vec<&PathBuf> =
                    ev.paths.iter().filter(|p| !ignored(&r2, p)).collect();
                if relevant.is_empty() {
                    return;
                }
                let models = relevant
                    .iter()
                    .any(|p| p.components().any(|c| c.as_os_str() == "models"));
                let _ = ev_tx.send((true, models));
            }
        })
        .map_err(|e| format!("cannot watch {}: {e}", root.display()))?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|e| format!("cannot watch {}: {e}", root.display()))?;
        // Shared libraries: the blocks, footprints and models that the board imports.
        if let Some(ws) = lamina_model::Workspace::find(&root) {
            for lib in ws.lib_dirs() {
                if lib.is_dir() && !lib.starts_with(&root) {
                    let _ = watcher.watch(&lib, RecursiveMode::Recursive);
                }
            }
        }
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("lamina-live".into())
            .spawn(move || worker(root, ev_rx, tx, wake))
            .map_err(|e| e.to_string())?;
        Ok(Live {
            rx,
            _watcher: watcher,
        })
    }
}

fn worker(root: PathBuf, ev: Receiver<(bool, bool)>, tx: Sender<Msg>, wake: impl Fn()) {
    let mut prev: Option<ProjectData> = None;
    let mut models = true;
    loop {
        let t0 = Instant::now();
        match build(&root, prev.as_ref(), models) {
            Ok(l) => {
                prev = Some(l.data.clone());
                let _ = tx.send(Msg::Loaded(Box::new(Loaded {
                    took: t0.elapsed(),
                    ..l
                })));
            }
            Err(e) => {
                let _ = tx.send(Msg::Error(e));
            }
        }
        wake();
        // Wait for a change. Then wait until a burst of writes stops.
        match ev.recv() {
            Ok((_, m)) => models = m,
            Err(_) => return,
        }
        loop {
            match ev.recv_timeout(Duration::from_millis(45)) {
                Ok((_, m)) => models |= m,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

fn build(root: &Path, prev: Option<&ProjectData>, models_changed: bool) -> Result<Loaded, String> {
    // Build a changed design source first. Its errors look like compile errors.
    let ports = lamina_wiring::standard(Some(root));
    let mut generated: Vec<String> = Vec::new();
    if let Some(src) = &ports.source {
        src.ensure_built(root).map_err(|e| e.to_string())?;
        generated = src
            .owned(root)
            .iter()
            .map(|k| k.file_name().to_string())
            .collect();
    }
    let mut last = String::new();
    for _ in 0..5 {
        match ProjectData::load(root) {
            Ok(data) => {
                let (mut analysis, s2, s3) = lamina_scene::build(&data, root);
                // Add the structure checks (unknown nets, bad layers) to the DRC results.
                analysis
                    .violations
                    .extend(lamina_core::validate::validate(&data));
                let changed = prev.map(|p| diff(p, &data)).unwrap_or_default();
                return Ok(Loaded {
                    data,
                    analysis,
                    s2,
                    s3,
                    took: Duration::ZERO,
                    changed,
                    models_changed,
                    generated,
                });
            }
            Err(e) => {
                last = e.to_string();
                std::thread::sleep(Duration::from_millis(60));
            }
        }
    }
    Err(last)
}

fn grow(r: Rect, m: f64) -> Rect {
    Rect {
        min: lamina_model::Point::new(r.min.x - m, r.min.y - m),
        max: lamina_model::Point::new(r.max.x + m, r.max.y + m),
    }
}

/// Areas that changed between two versions: moved parts, and new or changed
/// tracks, vias, zones and keep-outs.
pub fn diff(a: &ProjectData, b: &ProjectData) -> Vec<Rect> {
    let mut out = Vec::new();
    let part_box = |d: &ProjectData, r: &str| -> Option<Rect> {
        let pl = d.placement.placements.get(r)?;
        let fp = d.footprints.get(&d.parts.parts.get(r)?.footprint)?;
        lamina_geom::courtyard(fp, pl).and_then(Rect::from_points)
    };
    for (r, pl) in &b.placement.placements {
        let fp_changed = b
            .parts
            .parts
            .get(r)
            .map(|p| a.footprints.get(&p.footprint) != b.footprints.get(&p.footprint))
            .unwrap_or(false);
        if a.placement.placements.get(r) != Some(pl) || fp_changed {
            if let Some(x) = part_box(b, r) {
                out.push(x);
            }
        }
    }
    let tracks: BTreeMap<&str, &lamina_model::Track> = a
        .routing
        .tracks
        .iter()
        .map(|t| (t.id.as_str(), t))
        .collect();
    for t in &b.routing.tracks {
        if tracks.get(t.id.as_str()) != Some(&t) {
            if let Some(r) = Rect::from_points(t.points.iter().copied()) {
                out.push(grow(r, t.width / 2.0 + 0.2));
            }
        }
    }
    let vias: BTreeMap<&str, &lamina_model::Via> =
        a.routing.vias.iter().map(|v| (v.id.as_str(), v)).collect();
    for v in &b.routing.vias {
        if vias.get(v.id.as_str()) != Some(&v) {
            out.push(grow(
                Rect {
                    min: v.at,
                    max: v.at,
                },
                v.diameter / 2.0 + 0.2,
            ));
        }
    }
    for z in &b.routing.zones {
        if !a.routing.zones.contains(z) {
            if let Some(r) = z.outline.bbox() {
                out.push(r);
            }
        }
    }
    for k in &b.routing.keepouts {
        if !a.routing.keepouts.contains(k) {
            if let Some(r) = k.outline.bbox() {
                out.push(r);
            }
        }
    }
    if a.board.outline != b.board.outline {
        if let Some(r) = b.board.outline.as_ref().and_then(|o| o.bbox()) {
            out.push(r);
        }
    }
    out
}
