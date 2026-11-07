//! An open project, and the commit step that all operations use.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lamina_model::error::Result;
use lamina_model::*;
use schemars::JsonSchema;
use serde::Serialize;

use crate::lock::Lock;
use lamina_ports::Ports;

/// What an operation did. The agent gets this as JSON.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct OpResult {
    pub summary: String,
    /// The files that the operation wrote.
    pub changed: Vec<String>,
    /// The violations that this change added.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub new_violations: Vec<Violation>,
    /// The violations that this change corrected (only the rule and the items).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fixed: Vec<String>,
    /// The total number of errors and warnings on the board now.
    pub errors: usize,
    pub warnings: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// The files that a change modifies.
#[derive(Debug, Default, Clone)]
pub struct Touched {
    pub files: BTreeSet<FileKind>,
    pub footprints: BTreeSet<String>,
    pub removed_footprints: BTreeSet<String>,
}

impl Touched {
    pub fn file(k: FileKind) -> Self {
        let mut t = Touched::default();
        t.files.insert(k);
        t
    }

    pub fn and(mut self, k: FileKind) -> Self {
        self.files.insert(k);
        self
    }

    pub fn footprint(mut self, id: impl Into<String>) -> Self {
        self.footprints.insert(id.into());
        self
    }
}

pub type Checker = fn(&ProjectData) -> Vec<Violation>;

pub struct Project {
    root: PathBuf,
    pub data: ProjectData,
    checkers: Vec<Checker>,
    source: std::sync::Arc<dyn lamina_parts::PartSource>,
    kicad: std::sync::Arc<dyn lamina_kicad::KicadSource>,
    /// The part library: downloads go there too, and imports look there
    /// before the network.
    library: Library,
    /// The part providers, in the search order.
    pub(crate) providers: crate::provider::Providers,
    /// Everything from outside the core: formats, fabricators, renderer...
    pub(crate) ports: Ports,
    cached: Option<Vec<Violation>>,
    /// The size and the modification time of each project file at the last
    /// load or save. They show changes that another person or process made.
    stamp: Vec<(String, u64, std::time::SystemTime)>,
    _lock: Option<Lock>,
}

impl std::fmt::Debug for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Project").field("root", &self.root).finish()
    }
}

const PROJECT_GITIGNORE: &str = "cache/render/\ncache/build.json\n.lamina.lock\n*.tmp\n*.mesh\n";

impl Project {
    /// Create a new project in `dir`, which must not contain a project.
    pub fn create(dir: &Path, name: &str) -> Result<Project> {
        Project::create_with(dir, name, crate::wiring::standard(Some(dir)))
    }

    /// Like [`Project::create`], with the given ports.
    pub fn create_with(dir: &Path, name: &str, ports: Ports) -> Result<Project> {
        if ProjectData::is_project(dir) {
            return Err(Error::invalid(format!(
                "{} already contains a project. Open it, or use a different folder.",
                dir.display()
            )));
        }
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        let lock = Lock::acquire(dir)?;
        let data = ProjectData::new(name);
        data.save_all(dir)?;
        for sub in [FOOTPRINT_DIR, MODEL_DIR, PARTS_DIR] {
            let p = dir.join(sub);
            std::fs::create_dir_all(&p).map_err(|e| Error::io(&p, e))?;
        }
        let gi = dir.join(".gitignore");
        if !gi.exists() {
            write_atomic(&gi, PROJECT_GITIGNORE.as_bytes())?;
        }
        Ok(Project {
            root: dir.to_path_buf(),
            data,
            checkers: Vec::new(),
            source: std::sync::Arc::new(lamina_parts::HttpSource::default()),
            kicad: std::sync::Arc::new(lamina_kicad::HttpSource),
            library: Library::for_board(dir),
            providers: crate::provider::Providers::new(ports.providers.clone()),
            ports,
            cached: None,
            stamp: disk_stamp(dir),
            _lock: Some(lock),
        })
    }

    /// Open a project and get its lock, so that no other engine can write to it.
    pub fn open(dir: &Path) -> Result<Project> {
        Project::open_with(dir, crate::wiring::standard(Some(dir)))
    }

    /// Like [`Project::open`], with the given ports.
    pub fn open_with(dir: &Path, ports: Ports) -> Result<Project> {
        let lock = Lock::acquire(dir)?;
        let mut p = Project::open_shared_with(dir, ports)?;
        p._lock = Some(lock);
        Ok(p)
    }

    /// Open the project, but do not get the lock. The viewer uses this to edit
    /// at the same time as an agent. Both call [`Project::sync`] before each change.
    pub fn open_shared(dir: &Path) -> Result<Project> {
        Project::open_shared_with(dir, crate::wiring::standard(Some(dir)))
    }

    /// Like [`Project::open_shared`], with the given ports.
    pub fn open_shared_with(dir: &Path, ports: Ports) -> Result<Project> {
        lamina_build::ensure_built(dir)?;
        let stamp = disk_stamp(dir);
        let data = ProjectData::load(dir)?;
        Ok(Project {
            root: dir.to_path_buf(),
            data,
            checkers: Vec::new(),
            source: std::sync::Arc::new(lamina_parts::HttpSource::default()),
            kicad: std::sync::Arc::new(lamina_kicad::HttpSource),
            library: Library::for_board(dir),
            providers: crate::provider::Providers::new(ports.providers.clone()),
            ports,
            cached: None,
            stamp,
            _lock: None,
        })
    }

    /// Reload if the files on disk changed after the last load or save.
    /// Returns true if something was reloaded.
    pub fn sync(&mut self) -> Result<bool> {
        let built = lamina_build::ensure_built(&self.root)?.is_some();
        let now = disk_stamp(&self.root);
        if now == self.stamp && !built {
            return Ok(false);
        }
        self.data = ProjectData::load(&self.root)?;
        self.cached = None;
        self.stamp = now;
        Ok(true)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The source for part searches and imports (HTTP by default).
    pub fn source(&self) -> &dyn lamina_parts::PartSource {
        self.source.as_ref()
    }

    pub fn set_source(&mut self, s: std::sync::Arc<dyn lamina_parts::PartSource>) {
        self.source = s;
    }

    /// The source of KiCad footprints and models (by default, the official
    /// online libraries).
    pub fn kicad_source(&self) -> &dyn lamina_kicad::KicadSource {
        self.kicad.as_ref()
    }

    pub fn set_kicad_source(&mut self, s: std::sync::Arc<dyn lamina_kicad::KicadSource>) {
        self.kicad = s;
    }

    /// Use different providers (tests, offline mirrors, new sources).
    pub fn set_providers(&mut self, p: crate::provider::Providers) {
        self.providers = p;
    }

    /// The ports of this project.
    pub fn ports(&self) -> &Ports {
        &self.ports
    }

    /// Use different ports (tests and other programs).
    pub fn set_ports(&mut self, ports: Ports) {
        self.providers = crate::provider::Providers::new(ports.providers.clone());
        self.ports = ports;
    }

    /// Read a 3D model file through the mesh loader port.
    pub(crate) fn load_mesh(&self, path: &Path) -> Result<lamina_ports::mesh::Mesh> {
        let loader = self
            .ports
            .meshes
            .as_ref()
            .ok_or_else(|| Error::invalid(Ports::missing("mesh loader")))?;
        loader.load(path).map_err(Error::Invalid)
    }

    /// The part library of this board.
    pub fn library(&self) -> &Library {
        &self.library
    }

    /// Use a different library (tests and offline mirrors).
    pub fn set_library(&mut self, l: Library) {
        self.library = l;
    }

    /// Add a checker that runs after each change (DRC adds its checker here).
    pub fn add_checker(&mut self, c: Checker) {
        self.cached = None;
        self.checkers.push(c);
    }

    /// All the violations that are on the board now.
    pub fn check(&self) -> Vec<Violation> {
        self.check_data(&self.data)
    }

    fn check_data(&self, d: &ProjectData) -> Vec<Violation> {
        let mut v = crate::validate::validate(d);
        v.extend(lamina_drc::check(d));
        for c in &self.checkers {
            v.extend(c(d));
        }
        v
    }

    /// The violations of the state now, kept in the cache between changes.
    fn current_violations(&mut self) -> Vec<Violation> {
        if let Some(v) = &self.cached {
            return v.clone();
        }
        let v = self.check_data(&self.data);
        self.cached = Some(v.clone());
        v
    }

    /// Read all data from disk again (for example, after the user edited files).
    pub fn reload(&mut self) -> Result<()> {
        lamina_build::ensure_built(&self.root)?;
        self.stamp = disk_stamp(&self.root);
        self.data = ProjectData::load(&self.root)?;
        self.cached = None;
        Ok(())
    }

    /// Make `new` the project state, write the changed files and return a
    /// report. If `strict` is true, refuse the change if it adds an error.
    pub fn commit(
        &mut self,
        new: ProjectData,
        touched: Touched,
        strict: bool,
        summary: impl Into<String>,
    ) -> Result<OpResult> {
        let before = self.current_violations();
        let after = self.check_data(&new);
        let before_keys: BTreeSet<String> = before.iter().map(|v| v.key()).collect();
        let after_keys: BTreeSet<String> = after.iter().map(|v| v.key()).collect();
        let new_violations: Vec<Violation> = after
            .iter()
            .filter(|v| !before_keys.contains(&v.key()))
            .cloned()
            .collect();
        if strict {
            let errs: Vec<&Violation> = new_violations
                .iter()
                .filter(|v| v.severity == Severity::Error)
                .collect();
            if !errs.is_empty() {
                let list: Vec<String> = errs
                    .iter()
                    .take(10)
                    .map(|v| format!("{}: {}", v.rule, v.message))
                    .collect();
                return Err(Error::Refused(format!(
                    "strict: The change adds {} error(s), so it was not applied: {}",
                    errs.len(),
                    list.join("; ")
                )));
            }
        }
        let owned = lamina_build::owned(&self.root);
        let clash: Vec<&str> = touched
            .files
            .iter()
            .filter(|k| owned.contains(k))
            .map(|k| k.file_name())
            .collect();
        if !clash.is_empty() {
            return Err(Error::Refused(format!(
                "{} {} generated from {}. Change the source instead. \
                 Lamina rebuilds the files automatically.",
                clash.join(" and "),
                if clash.len() == 1 { "is" } else { "are" },
                lamina_build::SOURCE_FILE
            )));
        }
        let fixed: Vec<String> = before
            .iter()
            .filter(|v| !after_keys.contains(&v.key()))
            .map(|v| format!("{}: {}", v.rule, v.message))
            .collect();

        for id in &touched.removed_footprints {
            ProjectData::remove_footprint_file(&self.root, id)?;
        }
        self.data = new;
        let mut changed = Vec::new();
        for id in &touched.footprints {
            self.data.save_footprint(&self.root, id)?;
            changed.push(format!("{FOOTPRINT_DIR}/{id}.json"));
        }
        for k in &touched.files {
            self.data.save(&self.root, *k)?;
            changed.push(k.file_name().to_string());
        }
        self.stamp = disk_stamp(&self.root);
        let errors = after
            .iter()
            .filter(|v| v.severity == Severity::Error)
            .count();
        let warnings = after.len() - errors;
        self.cached = Some(after);
        Ok(OpResult {
            summary: summary.into(),
            changed,
            new_violations,
            fixed,
            errors,
            warnings,
            notes: Vec::new(),
        })
    }

    /// Build the design source and reload.
    pub fn build(&mut self) -> Result<lamina_build::Report> {
        let r = lamina_build::build(&self.root)?;
        self.reload()?;
        Ok(r)
    }

    /// Write a starter `design.jsonnet` for a new board (if there is no source
    /// file) and build it.
    pub fn init_source(&mut self) -> Result<lamina_build::Report> {
        let src = lamina_build::source_path(&self.root);
        if !src.is_file() {
            write_atomic(
                &src,
                lamina_build::template(&self.data.board.name).as_bytes(),
            )?;
        }
        self.build()
    }

    /// The files that the design source generates. Direct edits must not change them.
    pub fn source_owned(&self) -> BTreeSet<FileKind> {
        lamina_build::owned(&self.root)
    }

    /// Make `new` the project state. Write only the files that are different.
    pub fn apply(&mut self, new: ProjectData, summary: impl Into<String>) -> Result<OpResult> {
        let touched = touched_between(&self.data, &new);
        self.commit(new, touched, false, summary)
    }
}

/// The files that are different between two states.
pub fn touched_between(a: &ProjectData, b: &ProjectData) -> Touched {
    let mut t = Touched::default();
    if a.board != b.board {
        t.files.insert(FileKind::Board);
    }
    if a.parts != b.parts {
        t.files.insert(FileKind::Parts);
    }
    if a.nets != b.nets {
        t.files.insert(FileKind::Nets);
    }
    if a.placement != b.placement {
        t.files.insert(FileKind::Placement);
    }
    if a.routing != b.routing {
        t.files.insert(FileKind::Routing);
    }
    if a.notes != b.notes {
        t.files.insert(FileKind::Notes);
    }
    for (id, fp) in &b.footprints {
        if a.footprints.get(id) != Some(fp) {
            t.footprints.insert(id.clone());
        }
    }
    for id in a.footprints.keys() {
        if !b.footprints.contains_key(id) {
            t.removed_footprints.insert(id.clone());
        }
    }
    t
}

fn disk_stamp(root: &Path) -> Vec<(String, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut add = |name: String, path: &Path| {
        if let Ok(m) = std::fs::metadata(path) {
            out.push((name, m.len(), m.modified().unwrap_or(std::time::UNIX_EPOCH)));
        }
    };
    for k in FileKind::ALL {
        add(k.file_name().to_string(), &root.join(k.file_name()));
    }
    if let Ok(rd) = std::fs::read_dir(root.join(FOOTPRINT_DIR)) {
        let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        files.sort();
        for f in files {
            if f.extension().is_some_and(|x| x == "json") {
                add(f.display().to_string(), &f);
            }
        }
    }
    out
}

/// The next free id (for example `t12`) after the ids that have the same prefix.
pub fn next_id<'a>(prefix: &str, existing: impl IntoIterator<Item = &'a str>) -> String {
    let max = existing
        .into_iter()
        .filter_map(|s| s.strip_prefix(prefix)?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("{prefix}{}", max + 1)
}
