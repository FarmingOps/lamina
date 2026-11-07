//! Applies viewer edits through the engine on a worker thread, with undo.
//!
//! The viewer opens the project without its lock, so an agent can work on it
//! at the same time. Before each edit, the viewer reads the files again if they
//! changed on disk. Undo restores only the files that nobody changed after the edit.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use lamina_core::Project;
use lamina_model::{ProjectData, Severity};

pub type EditFn = Box<dyn FnOnce(&mut ProjectData) -> Result<String, String> + Send>;

enum Job {
    Edit(EditFn),
    Undo,
    Redo,
}

pub struct Reply {
    pub ok: bool,
    pub message: String,
    pub undo: Option<String>,
    pub redo: Option<String>,
}

struct Step {
    label: String,
    before: ProjectData,
    after: ProjectData,
}

pub struct Editor {
    tx: Sender<Job>,
    pub rx: Receiver<Reply>,
    pub undo: Option<String>,
    pub redo: Option<String>,
    pub busy: usize,
}

impl Editor {
    pub fn start(root: PathBuf, wake: impl Fn() + Send + 'static) -> Editor {
        let (tx, jobs) = channel::<Job>();
        let (out, rx) = channel();
        std::thread::Builder::new()
            .name("lamina-edit".into())
            .spawn(move || worker(root, jobs, out, wake))
            .expect("spawn editor thread");
        Editor {
            tx,
            rx,
            undo: None,
            redo: None,
            busy: 0,
        }
    }

    pub fn edit(
        &mut self,
        f: impl FnOnce(&mut ProjectData) -> Result<String, String> + Send + 'static,
    ) {
        self.busy += 1;
        let _ = self.tx.send(Job::Edit(Box::new(f)));
    }

    pub fn undo(&mut self) {
        self.busy += 1;
        let _ = self.tx.send(Job::Undo);
    }

    pub fn redo(&mut self) {
        self.busy += 1;
        let _ = self.tx.send(Job::Redo);
    }

    /// Get the next reply and update the undo labels.
    pub fn poll(&mut self) -> Option<Reply> {
        let r = self.rx.try_recv().ok()?;
        self.busy = self.busy.saturating_sub(1);
        self.undo = r.undo.clone();
        self.redo = r.redo.clone();
        Some(r)
    }
}

fn worker(root: PathBuf, jobs: Receiver<Job>, out: Sender<Reply>, wake: impl Fn()) {
    let mut project: Option<Project> = None;
    let mut undo: Vec<Step> = Vec::new();
    let mut redo: Vec<Step> = Vec::new();
    while let Ok(job) = jobs.recv() {
        let res = run(&root, &mut project, &mut undo, &mut redo, job);
        let (ok, message) = match res {
            Ok(m) => (true, m),
            Err(e) => (false, e),
        };
        let _ = out.send(Reply {
            ok,
            message,
            undo: undo.last().map(|s| s.label.clone()),
            redo: redo.last().map(|s| s.label.clone()),
        });
        wake();
    }
}

fn run(
    root: &std::path::Path,
    project: &mut Option<Project>,
    undo: &mut Vec<Step>,
    redo: &mut Vec<Step>,
    job: Job,
) -> Result<String, String> {
    if project.is_none() {
        *project = Some(lamina_wiring::open_shared(root).map_err(|e| e.to_string())?);
    }
    let p = project.as_mut().unwrap();
    p.sync()
        .map_err(|e| format!("cannot read the project: {e}"))?;
    match job {
        Job::Edit(f) => {
            let before = p.data.clone();
            let mut d = before.clone();
            let label = f(&mut d)?;
            if d == before {
                return Err("Nothing changed".into());
            }
            let r = p.apply(d, label.clone()).map_err(|e| e.to_string())?;
            undo.push(Step {
                label: label.clone(),
                before,
                after: p.data.clone(),
            });
            redo.clear();
            Ok(describe(&label, &r))
        }
        Job::Undo => {
            let s = undo.pop().ok_or("There is nothing to undo")?;
            let target = restore(&p.data, &s.after, &s.before)
                .map_err(|e| format!("Cannot undo “{}”: {e}", s.label))?;
            let r = p
                .apply(target, format!("undo {}", s.label))
                .map_err(|e| e.to_string())?;
            let msg = describe(&format!("Undid {}", lower_first(&s.label)), &r);
            redo.push(Step {
                label: s.label,
                before: s.before,
                after: p.data.clone(),
            });
            Ok(msg)
        }
        Job::Redo => {
            let s = redo.pop().ok_or("There is nothing to redo")?;
            // Undo restored the `before` state of the original edit.
            let target = restore(&p.data, &s.before, &s.after)
                .map_err(|e| format!("Cannot redo “{}”: {e}", s.label))?;
            let r = p
                .apply(target, format!("redo {}", s.label))
                .map_err(|e| e.to_string())?;
            let msg = describe(&format!("Redid {}", lower_first(&s.label)), &r);
            undo.push(Step {
                label: s.label,
                before: s.before,
                after: p.data.clone(),
            });
            Ok(msg)
        }
    }
}

/// Change the files that an edit changed from `from` back to `to`, if they
/// still match `from`. Files that the edit did not change keep their current content.
fn restore(cur: &ProjectData, from: &ProjectData, to: &ProjectData) -> Result<ProjectData, String> {
    let mut out = cur.clone();
    macro_rules! part {
        ($f:ident, $name:expr) => {
            if from.$f != to.$f {
                if cur.$f != from.$f {
                    return Err(format!(
                        "{} changed after this edit. The agent probably changed it.",
                        $name
                    ));
                }
                out.$f = to.$f.clone();
            }
        };
    }
    part!(board, "board.json");
    part!(parts, "parts.json");
    part!(nets, "nets.json");
    part!(placement, "placement.json");
    part!(routing, "routing.json");
    part!(footprints, "a footprint");
    Ok(out)
}

fn describe(label: &str, r: &lamina_core::OpResult) -> String {
    let errs: Vec<&lamina_model::Violation> = r
        .new_violations
        .iter()
        .filter(|v| v.severity == Severity::Error)
        .collect();
    if errs.is_empty() {
        return label.to_string();
    }
    let mut rules: Vec<&str> = errs.iter().map(|v| v.rule.as_str()).collect();
    rules.sort();
    rules.dedup();
    format!(
        "{label} · {} new error{} ({})",
        errs.len(),
        if errs.len() == 1 { "" } else { "s" },
        rules.join(", ")
    )
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lamina_model::*;

    #[test]
    fn restore_refuses_after_foreign_change() {
        let a = ProjectData::new("t");
        let mut b = a.clone();
        b.placement
            .placements
            .insert("R1".into(), Placement::at(1.0, 1.0));
        // Undo when nobody else changed anything.
        let back = restore(&b, &b, &a).unwrap();
        assert_eq!(back.placement, a.placement);
        // Someone moved R1 again after our edit.
        let mut c = b.clone();
        c.placement.placements.get_mut("R1").unwrap().x = 5.0;
        assert!(restore(&c, &b, &a).is_err());
        // A change to another file does not block the undo.
        let mut e = b.clone();
        e.board.name = "renamed".into();
        let back = restore(&e, &b, &a).unwrap();
        assert_eq!(back.board.name, "renamed");
        assert!(back.placement.placements.is_empty());
    }
}
