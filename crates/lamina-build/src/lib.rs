//! Builds the `board.json`, `parts.json` and `nets.json` of a board from a
//! modular `design.jsonnet` source.
//!
//! The source is Jsonnet: JSON plus variables, functions and `import`.
//! It evaluates to an object with one or more of the keys `board`, `parts`
//! and `nets`. For each key that is present, the source generates ("owns")
//! the matching file. An import looks first in the folder of the importing
//! file, then in the workspace library folders. `import 'lamina.libsonnet'`
//! is built in.
//!
//! `cache/build.json` records which files the source owns and the hash of
//! each input. [`needs_build`] uses this record to find when a new build is
//! necessary.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use jrsonnet_evaluator::manifest::JsonFormat;
use jrsonnet_evaluator::trace::{CompactFormat, PathResolver, TraceFormat};
use jrsonnet_evaluator::{
    AsPathLike, FileImportResolver, ImportResolver, SourcePath, SourceVirtual, State,
};
use jrsonnet_gcmodule::Acyclic;
use lamina_model::error::{Error, Result};
use lamina_model::{
    BoardFile, FileKind, NetsFile, PartsFile, ProjectData, SCHEMA, Workspace, json, valid_net_name,
    valid_refdes, write_atomic,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SOURCE_FILE: &str = "design.jsonnet";
pub const RECORD_FILE: &str = "cache/build.json";
pub const HELPER_NAME: &str = "lamina.libsonnet";
/// The built-in helper library. Import it as `lamina.libsonnet`.
pub const HELPER: &str = include_str!("lamina.libsonnet");

const ENGINE: &str = env!("CARGO_PKG_VERSION");

/// What the last build read and wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BuildRecord {
    pub schema: u32,
    pub engine: String,
    pub source: String,
    /// File kinds the source generates: `board`, `parts`, `nets`.
    pub owns: Vec<String>,
    pub inputs: Vec<Input>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Input {
    pub path: String,
    pub hash: String,
}

pub use lamina_ports::source::Report;

pub mod port;
pub use port::JsonnetSource;

pub fn source_path(root: &Path) -> PathBuf {
    root.join(SOURCE_FILE)
}

pub fn has_source(root: &Path) -> bool {
    source_path(root).is_file()
}

pub fn record(root: &Path) -> Option<BuildRecord> {
    let text = std::fs::read_to_string(root.join(RECORD_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

fn kind_name(k: FileKind) -> &'static str {
    match k {
        FileKind::Board => "board",
        FileKind::Parts => "parts",
        FileKind::Nets => "nets",
        FileKind::Placement => "placement",
        FileKind::Routing => "routing",
        FileKind::Notes => "notes",
    }
}

fn kind_of(name: &str) -> Option<FileKind> {
    FileKind::ALL.into_iter().find(|k| kind_name(*k) == name)
}

/// File kinds that the source generates. A build replaces edits to these files.
pub fn owned(root: &Path) -> BTreeSet<FileKind> {
    if !has_source(root) {
        return BTreeSet::new();
    }
    record(root)
        .map(|r| r.owns.iter().filter_map(|n| kind_of(n)).collect())
        .unwrap_or_default()
}

/// The reason to build the source, or `None` if the outputs are up to date.
pub fn needs_build(root: &Path) -> Option<String> {
    if !has_source(root) {
        return None;
    }
    let Some(rec) = record(root) else {
        return Some("not built yet".into());
    };
    if rec.engine != ENGINE {
        return Some(format!(
            "Lamina {} made the last build. This is Lamina {ENGINE}.",
            rec.engine
        ));
    }
    for i in &rec.inputs {
        match std::fs::read(&i.path) {
            Ok(bytes) if hash(&bytes) == i.hash => {}
            Ok(_) => return Some(format!("{} changed", display_rel(root, Path::new(&i.path)))),
            Err(_) => {
                return Some(format!(
                    "{} is missing",
                    display_rel(root, Path::new(&i.path))
                ));
            }
        }
    }
    for k in rec.owns.iter().filter_map(|n| kind_of(n)) {
        if !root.join(k.file_name()).is_file() {
            return Some(format!("{} is missing", k.file_name()));
        }
    }
    None
}

/// Build if the source changed. If a build ran, return its report.
pub fn ensure_built(root: &Path) -> Result<Option<Report>> {
    match needs_build(root) {
        Some(_) => build(root).map(Some),
        None => Ok(None),
    }
}

/// Evaluate the source and write the generated files.
pub fn build(root: &Path) -> Result<Report> {
    let src = source_path(root);
    if !src.is_file() {
        return Err(Error::invalid(format!(
            "{} has no {SOURCE_FILE}",
            root.display()
        )));
    }
    let (value, inputs) = evaluate(root, &src)?;
    let Value::Object(top) = value else {
        return Err(Error::invalid(format!(
            "{SOURCE_FILE} must evaluate to an object with `board`, `parts` or `nets`"
        )));
    };
    for k in top.keys() {
        if !matches!(k.as_str(), "board" | "parts" | "nets") {
            return Err(Error::invalid(format!(
                "{SOURCE_FILE}: unknown top-level key `{k}`. Use board, parts or nets."
            )));
        }
    }
    if top.is_empty() {
        return Err(Error::invalid(format!(
            "{SOURCE_FILE} evaluates to an empty object. Add `parts`, `nets` or `board`."
        )));
    }

    let mut report = Report {
        source: SOURCE_FILE.into(),
        inputs: inputs.iter().map(|p| display_rel(root, p)).collect(),
        ..Default::default()
    };
    let prev_parts: Option<PartsFile> = read_or_none(&root.join(FileKind::Parts.file_name()));
    let prev_board: Option<BoardFile> = read_or_none(&root.join(FileKind::Board.file_name()));

    let mut owns = Vec::new();
    let mut writes: Vec<(FileKind, String)> = Vec::new();

    if let Some(b) = top.get("board") {
        let name = b
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| prev_board.as_ref().map(|p| p.name.clone()))
            .or_else(|| {
                root.file_name()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "board".into());
        let board = board_from(b, &name)?;
        board
            .stackup
            .check()
            .map_err(|e| Error::invalid(format!("board.stackup: {e}")))?;
        if let Some(o) = &board.outline {
            o.check()
                .map_err(|e| Error::invalid(format!("board.outline: {e}")))?;
        }
        owns.push(FileKind::Board);
        writes.push((FileKind::Board, to_text(&board)?));
    }

    let mut parts: Option<PartsFile> = None;
    if let Some(p) = top.get("parts") {
        let file = parts_from(p)?;
        report.parts = file.parts.len();
        owns.push(FileKind::Parts);
        writes.push((FileKind::Parts, to_text(&file)?));
        parts = Some(file);
    }
    if let Some(n) = top.get("nets") {
        let file = nets_from(n)?;
        if let Some(pf) = &parts {
            for (name, net) in &file.nets {
                for pad in &net.pads {
                    if !pf.parts.contains_key(&pad.part) {
                        report.warnings.push(format!(
                            "net {name}: pad {pad} refers to a part that is not in `parts`"
                        ));
                    }
                }
            }
        }
        report.nets = file.nets.len();
        owns.push(FileKind::Nets);
        writes.push((FileKind::Nets, to_text(&file)?));
    }

    // Remove the placement of parts that are not in the source now. If not,
    // the layout refers to parts that do not exist.
    if let (Some(new), Some(old)) = (&parts, &prev_parts) {
        report.added_parts = new
            .parts
            .keys()
            .filter(|r| !old.parts.contains_key(*r))
            .cloned()
            .collect();
        report.removed_parts = old
            .parts
            .keys()
            .filter(|r| !new.parts.contains_key(*r))
            .cloned()
            .collect();
        if !report.removed_parts.is_empty() {
            let path = root.join(FileKind::Placement.file_name());
            if let Some(mut pl) = read_or_none::<lamina_model::PlacementFile>(&path) {
                let placed: Vec<String> = report
                    .removed_parts
                    .iter()
                    .filter(|r| pl.placements.contains_key(*r))
                    .cloned()
                    .collect();
                if !placed.is_empty() {
                    pl.placements.retain(|r, _| new.parts.contains_key(r));
                    report.unplaced = placed;
                    writes.push((FileKind::Placement, to_text(&pl)?));
                }
            }
        }
    }

    for (k, text) in writes {
        let path = root.join(k.file_name());
        let same = std::fs::read(&path).is_ok_and(|b| b == text.as_bytes());
        if !same {
            write_atomic(&path, text.as_bytes())?;
            report.written.push(k.file_name().to_string());
        }
    }
    report.owns = owns.iter().map(|k| kind_name(*k).to_string()).collect();
    let rec = BuildRecord {
        schema: SCHEMA,
        engine: ENGINE.into(),
        source: SOURCE_FILE.into(),
        owns: report.owns.clone(),
        inputs: inputs
            .iter()
            .map(|p| Input {
                path: canon(p).to_string_lossy().replace('\\', "/"),
                hash: std::fs::read(p).map(|b| hash(&b)).unwrap_or_default(),
            })
            .collect(),
    };
    let rec_path = root.join(RECORD_FILE);
    write_atomic(
        &rec_path,
        json::to_string(&rec)
            .map_err(|e| Error::Json {
                path: rec_path.clone(),
                source: e,
            })?
            .as_bytes(),
    )?;
    Ok(report)
}

/// A template source for a new board.
pub fn template(name: &str) -> String {
    format!(
        r#"// {name}: Lamina design source. To build it, run `lamina build`.
// The viewer and the MCP server build it again automatically when this file
// or an imported file changes.
local lamina = import 'lamina.libsonnet';

local width = 50;
local height = 30;

lamina.merge([
  {{
    board: {{
      name: '{name}',
      outline: lamina.rect(width, height, 2),
      holes: lamina.corner_holes(width, height, 3.2, 4),
      stackup: {{ copper_layers: 2, thickness: 1.6 }},
      rules: {{ preset: 'jlcpcb-2layer' }},
    }},
    parts: {{}},
    nets: {{}},
  }},
])
"#
    )
}

// ---------------------------------------------------------------- evaluation

#[derive(Acyclic)]
struct Resolver {
    inner: FileImportResolver,
    loaded: Rc<RefCell<BTreeSet<PathBuf>>>,
}

impl ImportResolver for Resolver {
    fn resolve_from(
        &self,
        from: &SourcePath,
        path: &dyn AsPathLike,
    ) -> jrsonnet_evaluator::Result<SourcePath> {
        if path.as_path().to_owned().to_string() == HELPER_NAME {
            return Ok(SourcePath::new(SourceVirtual(HELPER_NAME.into())));
        }
        self.inner.resolve_from(from, path)
    }

    fn load_file_contents(&self, resolved: &SourcePath) -> jrsonnet_evaluator::Result<Vec<u8>> {
        if resolved.downcast_ref::<SourceVirtual>().is_some() {
            return Ok(HELPER.as_bytes().to_vec());
        }
        if let Some(p) = resolved.path() {
            self.loaded.borrow_mut().insert(p.to_path_buf());
        }
        self.inner.load_file_contents(resolved)
    }
}

fn evaluate(root: &Path, src: &Path) -> Result<(Value, Vec<PathBuf>)> {
    let jpaths = Workspace::find(root)
        .map(|w| w.lib_dirs())
        .unwrap_or_default();
    let loaded = Rc::new(RefCell::new(BTreeSet::new()));
    let resolver = Resolver {
        inner: FileImportResolver::new(jpaths),
        loaded: loaded.clone(),
    };
    let mut b = State::builder();
    b.import_resolver(resolver)
        .context_initializer(jrsonnet_stdlib::ContextInitializer::new(
            PathResolver::Relative(root.to_path_buf()),
        ));
    let state = b.build();
    let fmt = CompactFormat {
        resolver: PathResolver::Relative(root.to_path_buf()),
        max_trace: 12,
        padding: 2,
    };
    let explain = |e: jrsonnet_evaluator::Error| {
        Error::invalid(format!(
            "{SOURCE_FILE}: {}",
            fmt.format(&e).unwrap_or_else(|_| e.to_string())
        ))
    };
    let text = {
        let _guard = state.enter();
        let val = state.import(src).map_err(explain)?;
        val.manifest(JsonFormat::minify()).map_err(explain)?
    };
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| Error::invalid(format!("{SOURCE_FILE}: the result is not valid JSON: {e}")))?;
    let inputs: Vec<PathBuf> = loaded.borrow().iter().cloned().collect();
    Ok((value, inputs))
}

// ---------------------------------------------------------------- conversion

fn board_from(v: &Value, name: &str) -> Result<BoardFile> {
    if !v.is_object() {
        return Err(Error::invalid("board must be an object"));
    }
    let mut base = serde_json::to_value(BoardFile::new(name)).expect("board to json");
    let mut over = v.clone();
    // A rules preset gives the base values that the source does not give.
    if let Some(Value::Object(rules)) = over.get_mut("rules") {
        if let Some(Value::String(preset)) = rules.get("preset") {
            let Some(r) = lamina_drc::presets::get(preset) else {
                let names: Vec<&str> = lamina_drc::presets::PRESETS
                    .iter()
                    .map(|p| p.name)
                    .collect();
                return Err(Error::invalid(format!(
                    "board.rules.preset `{preset}` is unknown. Use one of these presets: {}",
                    names.join(", ")
                )));
            };
            let mut pr = serde_json::to_value(r).expect("rules to json");
            let own = std::mem::take(rules);
            deep_merge(&mut pr, Value::Object(own));
            *rules = match pr {
                Value::Object(m) => m,
                _ => unreachable!(),
            };
        }
    }
    deep_merge(&mut base, over);
    if let Value::Object(m) = &mut base {
        m.insert("schema".into(), Value::from(SCHEMA));
    }
    serde_json::from_value(base).map_err(|e| Error::invalid(format!("board: {e}")))
}

fn parts_from(v: &Value) -> Result<PartsFile> {
    let Value::Object(m) = v else {
        return Err(Error::invalid("parts must be an object with refdes keys"));
    };
    let mut parts = BTreeMap::new();
    for (refdes, p) in m {
        if !valid_refdes(refdes) {
            return Err(Error::invalid(format!(
                "parts: `{refdes}` is not a valid refdes (letters and then digits, for example R12)"
            )));
        }
        let part: lamina_model::Part = serde_json::from_value(p.clone())
            .map_err(|e| Error::invalid(format!("parts.{refdes}: {e}")))?;
        parts.insert(refdes.clone(), part);
    }
    Ok(PartsFile {
        schema: SCHEMA,
        parts,
    })
}

fn nets_from(v: &Value) -> Result<NetsFile> {
    let Value::Object(m) = v else {
        return Err(Error::invalid(
            "nets must be an object with net names as keys",
        ));
    };
    let mut nets = BTreeMap::new();
    let mut seen: BTreeMap<lamina_model::PadRef, String> = BTreeMap::new();
    for (name, n) in m {
        if !valid_net_name(name) {
            return Err(Error::invalid(format!(
                "nets: `{name}` is not a valid net name"
            )));
        }
        let mut net: lamina_model::Net = serde_json::from_value(n.clone())
            .map_err(|e| Error::invalid(format!("nets.{name}: {e}")))?;
        net.pads.sort();
        net.pads.dedup();
        for pad in &net.pads {
            if let Some(other) = seen.insert(pad.clone(), name.clone()) {
                return Err(Error::invalid(format!(
                    "nets: pad {pad} is in both {other} and {name}"
                )));
            }
        }
        nets.insert(name.clone(), net);
    }
    Ok(NetsFile {
        schema: SCHEMA,
        nets,
    })
}

/// If a field is an object in both `over` and `base`, merge the two objects.
/// Other fields of `over` replace the fields of `base`. If `over` is not an
/// object, it replaces `base`.
fn deep_merge(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(bv) if bv.is_object() && v.is_object() => deep_merge(bv, v),
                    _ => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
}

fn to_text<T: Serialize>(v: &T) -> Result<String> {
    json::to_string(v).map_err(|e| Error::invalid(e.to_string()))
}

fn read_or_none<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Canonical path without Windows' `\\?\` prefix.
fn canon(p: &Path) -> PathBuf {
    let c = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = c.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => c,
    }
}

fn display_rel(root: &Path, p: &Path) -> String {
    let p = canon(p);
    let rel = p
        .strip_prefix(canon(root))
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| {
            // Show a path from the workspace library relative to the workspace root.
            Workspace::find(root)
                .and_then(|w| p.strip_prefix(canon(&w.root)).ok().map(Path::to_path_buf))
                .unwrap_or_else(|| p.to_path_buf())
        });
    rel.to_string_lossy().replace('\\', "/")
}

/// FNV-1a. It is enough to find a changed file.
fn hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Evaluate the source and return its data. This function writes no files.
pub fn preview(root: &Path) -> Result<ProjectData> {
    let (value, _) = evaluate(root, &source_path(root))?;
    let mut d = ProjectData::new("preview");
    if let Some(b) = value.get("board") {
        d.board = board_from(b, "preview")?;
    }
    if let Some(p) = value.get("parts") {
        d.parts = parts_from(p)?;
    }
    if let Some(n) = value.get("nets") {
        d.nets = nets_from(n)?;
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    fn workspace() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let ws = t.path().join("ws");
        let board = ws.join("boards/a");
        std::fs::create_dir_all(&board).unwrap();
        let mut w = Workspace::create(&ws, "demo").unwrap();
        w.add_board(&board).unwrap();
        write(
            &ws.join("lib/res.libsonnet"),
            r#"
local lamina = import 'lamina.libsonnet';
{
  divider(name, top, bottom, mid, gnd):: {
    parts: { [name + '1']: { footprint: 'R0603', value: '10k' }, [name + '2']: { footprint: 'R0603', value: '10k' } },
    nets: {
      [top]: lamina.net([name + '1.1']),
      [mid]: lamina.net([name + '1.2', name + '2.1']),
      [gnd]: lamina.net([name + '2.2']),
    },
  },
}
"#,
        );
        write(
            &board.join(SOURCE_FILE),
            r#"
local lamina = import 'lamina.libsonnet';
local res = import 'res.libsonnet';
lamina.merge([
  { board: { name: 'a', outline: lamina.rect(20, 10, 1), rules: { preset: 'jlcpcb-2layer', net_classes: { power: { track_width: 0.5 } } }, stackup: { mask_color: 'black' } } },
  res.divider('RA', 'VIN', 'GND', 'VA', 'GND'),
  res.divider('RB', 'VIN', 'GND', 'VB', 'GND'),
])
"#,
        );
        (t, board)
    }

    #[test]
    fn builds_and_merges_nets() {
        let (_t, board) = workspace();
        assert_eq!(needs_build(&board).as_deref(), Some("not built yet"));
        let r = build(&board).unwrap();
        assert_eq!(r.owns, vec!["board", "parts", "nets"]);
        assert_eq!(r.parts, 4);
        assert_eq!(r.nets, 4);
        assert!(
            r.inputs.iter().any(|i| i.ends_with("lib/res.libsonnet")),
            "{:?}",
            r.inputs
        );
        let d = ProjectData::load(&board).unwrap();
        assert_eq!(d.nets.nets["GND"].pads.len(), 2, "GND joins both dividers");
        assert_eq!(d.nets.nets["VA"].pads.len(), 2);
        assert_eq!(d.nets.nets["VIN"].pads.len(), 2);
        assert_eq!(d.board.rules.preset.as_deref(), Some("jlcpcb-2layer"));
        assert_eq!(d.board.rules.net_classes["power"].track_width, Some(0.5));
        assert_eq!(d.board.stackup.mask_color, "black");
        assert_eq!(d.board.stackup.copper_layers, 2);
        assert_eq!(d.board.outline.as_ref().unwrap().vertices.len(), 8);
        assert_eq!(needs_build(&board), None);
        assert_eq!(owned(&board).len(), 3);
    }

    #[test]
    fn group_tags_every_part_of_a_block() {
        let (_t, board) = workspace();
        write(
            &board.join(SOURCE_FILE),
            r#"
local lamina = import 'lamina.libsonnet';
local res = import 'res.libsonnet';
lamina.merge([
  { board: { name: 'a' } },
  lamina.group('a', res.divider('RA', 'VIN', 'GND', 'VA', 'GND')),
  lamina.group('b', res.divider('RB', 'VIN', 'GND', 'VB', 'GND')),
  lamina.group('empty', { nets: { X: lamina.net([]) } }),
])
"#,
        );
        build(&board).unwrap();
        let d = ProjectData::load(&board).unwrap();
        let g = d.parts.groups();
        assert_eq!(g["a"], vec!["RA1", "RA2"]);
        assert_eq!(g["b"], vec!["RB1", "RB2"]);
        assert_eq!(g.len(), 2);
        assert_eq!(d.parts.parts["RA1"].value.as_deref(), Some("10k"));
    }

    #[test]
    fn library_change_triggers_rebuild_and_unplaces_removed_parts() {
        let (_t, board) = workspace();
        build(&board).unwrap();
        let mut d = ProjectData::load(&board).unwrap();
        d.placement
            .placements
            .insert("RB1".into(), lamina_model::Placement::at(1.0, 1.0));
        d.placement
            .placements
            .insert("RA1".into(), lamina_model::Placement::at(2.0, 1.0));
        d.save(&board, FileKind::Placement).unwrap();
        assert_eq!(needs_build(&board), None);

        let src = board.join(SOURCE_FILE);
        let text = std::fs::read_to_string(&src).unwrap();
        write(
            &src,
            &text.replace("  res.divider('RB', 'VIN', 'GND', 'VB', 'GND'),\n", ""),
        );
        assert_eq!(
            needs_build(&board).as_deref(),
            Some("design.jsonnet changed")
        );
        let r = ensure_built(&board).unwrap().unwrap();
        assert_eq!(r.removed_parts, vec!["RB1", "RB2"]);
        assert_eq!(r.unplaced, vec!["RB1"]);
        let d = ProjectData::load(&board).unwrap();
        assert!(d.placement.placements.contains_key("RA1"));
        assert!(!d.placement.placements.contains_key("RB1"));

        let lib = board.join("../../lib/res.libsonnet");
        let text = std::fs::read_to_string(&lib).unwrap();
        write(&lib, &text.replace("'10k'", "'1k'"));
        assert!(
            needs_build(&board)
                .unwrap()
                .ends_with("lib/res.libsonnet changed")
        );
        build(&board).unwrap();
        let d = ProjectData::load(&board).unwrap();
        assert_eq!(d.parts.parts["RA1"].value.as_deref(), Some("1k"));
    }

    #[test]
    fn errors_point_at_the_source() {
        let (_t, board) = workspace();
        let src = board.join(SOURCE_FILE);
        write(&src, "local x = import 'missing.libsonnet';\n{ parts: x }");
        let e = build(&board).unwrap_err().to_string();
        assert!(e.contains("missing.libsonnet"), "{e}");
        write(&src, "{ parts: { 'bad name': { footprint: 'x' } } }");
        let e = build(&board).unwrap_err().to_string();
        assert!(e.contains("bad name"), "{e}");
        write(
            &src,
            "{ nets: { A: { pads: ['R1.1'] }, B: { pads: ['R1.1'] } } }",
        );
        let e = build(&board).unwrap_err().to_string();
        assert!(e.contains("R1.1 is in both A and B"), "{e}");
        write(&src, "{ stuff: 1 }");
        let e = build(&board).unwrap_err().to_string();
        assert!(e.contains("unknown top-level key `stuff`"), "{e}");
        write(
            &src,
            "local lamina = import 'lamina.libsonnet';\nlamina.merge([{ parts: { R1: {footprint: 'a'} } }, { parts: { R1: {footprint: 'b'} } }])",
        );
        let e = build(&board).unwrap_err().to_string();
        assert!(
            e.contains("more than one fragment has these parts: R1"),
            "{e}"
        );
    }

    #[test]
    fn partial_source_owns_only_its_keys() {
        let (_t, board) = workspace();
        let d = ProjectData::new("hand");
        d.save_all(&board).unwrap();
        write(
            &board.join(SOURCE_FILE),
            "{ nets: { GND: { pads: ['C1.2'] } } }",
        );
        let r = build(&board).unwrap();
        assert_eq!(r.owns, vec!["nets"]);
        assert_eq!(owned(&board), BTreeSet::from([FileKind::Nets]));
        let d = ProjectData::load(&board).unwrap();
        assert_eq!(d.board.name, "hand");
    }

    #[test]
    fn template_builds() {
        let t = tempfile::tempdir().unwrap();
        let board = t.path().join("b");
        write(&board.join(SOURCE_FILE), &template("demo"));
        let r = build(&board).unwrap();
        assert_eq!(r.owns, vec!["board", "parts", "nets"]);
        let d = ProjectData::load(&board).unwrap();
        assert_eq!(d.board.name, "demo");
        assert_eq!(d.board.holes.len(), 4);
    }
}
