use lamina_model::error::Result;
use lamina_model::*;
use lamina_ports::Ports;
use lamina_ports::fab::FabInfo;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::project::Project;

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct Export {
    /// Output folder, relative to the project (default `output`).
    #[serde(default)]
    pub dir: Option<String>,
    /// Export also if DRC finds errors (default false).
    #[serde(default)]
    pub force: bool,
    /// The fabricator id (see `fabricators`). The default is the first one.
    #[serde(default)]
    pub fabricator: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Exported {
    pub files: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListNotes {
    /// Include resolved notes.
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AddNote {
    pub text: String,
    #[serde(default)]
    pub at: Option<Point>,
    #[serde(default)]
    pub refdes: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ResolveNotes {
    pub ids: Vec<String>,
}

impl Project {
    /// The fabrication outputs that are plugged in, in the default order.
    pub fn fabricators(&self) -> Vec<FabInfo> {
        self.ports.fabricators.iter().map(|f| f.info()).collect()
    }

    pub fn export(&self, p: Export) -> Result<Exported> {
        let errors: Vec<Violation> = self
            .check()
            .into_iter()
            .filter(|v| v.severity == Severity::Error)
            .collect();
        if !errors.is_empty() && !p.force {
            let list: Vec<String> = errors.iter().take(8).map(|v| v.message.clone()).collect();
            return Err(Error::Refused(format!(
                "There are {} DRC error(s). To export, fix them or pass force: true.                  The first errors are: {}",
                errors.len(),
                list.join("; ")
            )));
        }
        let fab = match &p.fabricator {
            Some(id) => self.ports.fabricator(id).ok_or_else(|| {
                Error::invalid(format!(
                    "There is no fabricator `{id}`. Call `fabricators` for the list."
                ))
            })?,
            None => self
                .ports
                .fabricators
                .first()
                .map(|f| f.as_ref())
                .ok_or_else(|| Error::invalid(Ports::missing("fabricator")))?,
        };
        let (a, s2, _) = lamina_scene::build(&self.data, self.root());
        let name: String = self
            .data
            .board
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let artwork = fab.artwork(&s2, &a.geometry, &name);
        let (asm, mut warnings) = fab.assembly(&self.data, &name);
        let archive = fab.archive(&artwork, &name).map_err(Error::Invalid)?;
        let rel = p.dir.unwrap_or_else(|| "output".into());
        if rel.contains("..") {
            return Err(Error::invalid(
                "dir must be in the project folder. Do not use `..` in it.",
            ));
        }
        let dir = self.root().join(rel);
        std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        let mut files = Vec::new();
        for o in artwork.iter().chain(&asm).chain(&archive) {
            let path = dir.join(&o.name);
            std::fs::write(&path, &o.contents).map_err(|e| Error::io(&path, e))?;
            files.push(path.display().to_string());
        }
        if !errors.is_empty() {
            warnings.insert(
                0,
                format!(
                    "The files were exported with {} DRC error(s).",
                    errors.len()
                ),
            );
        }
        let unrouted = a.ratsnest.len();
        if unrouted > 0 {
            warnings.push(format!("{unrouted} connection(s) are not routed yet."));
        }
        Ok(Exported { files, warnings })
    }

    fn notes_from_disk(&self) -> NotesFile {
        std::fs::read_to_string(self.root().join("notes.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save_notes(&mut self, n: NotesFile) -> Result<()> {
        self.data.notes = n;
        self.data.notes.schema = SCHEMA;
        self.data.save(self.root(), FileKind::Notes)
    }

    pub fn list_notes(&mut self, p: ListNotes) -> Vec<Note> {
        let n = self.notes_from_disk();
        self.data.notes = n.clone();
        n.notes
            .into_iter()
            .filter(|x| p.all || !x.resolved)
            .collect()
    }

    pub fn add_note(&mut self, p: AddNote) -> Result<Note> {
        if p.text.trim().is_empty() {
            return Err(Error::invalid("The note text is empty. Give a text."));
        }
        let mut n = self.notes_from_disk();
        let next = n
            .notes
            .iter()
            .filter_map(|x| x.id.strip_prefix('n')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let note = Note {
            id: format!("n{next}"),
            text: p.text,
            at: p.at,
            refdes: p.refdes,
            author: "agent".into(),
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            resolved: false,
        };
        n.notes.push(note.clone());
        self.save_notes(n)?;
        Ok(note)
    }

    pub fn resolve_notes(&mut self, p: ResolveNotes) -> Result<usize> {
        let mut n = self.notes_from_disk();
        let mut k = 0;
        for x in n.notes.iter_mut() {
            if p.ids.contains(&x.id) && !x.resolved {
                x.resolved = true;
                k += 1;
            }
        }
        self.save_notes(n)?;
        Ok(k)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ExportStep {
    /// The output file. The default is `output/<board name>.step` in the project.
    #[serde(default)]
    pub path: Option<String>,
    /// Also drill the vias through the board (default false).
    #[serde(default)]
    pub vias: bool,
    /// Do not include the part models. Export only the bare board.
    #[serde(default)]
    pub board_only: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StepExported {
    pub path: String,
    /// The number of parts that have their exact STEP model.
    pub exact: usize,
    /// The number of parts that are faceted meshes (models that are only OBJ).
    pub meshed: usize,
    pub bytes: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl Project {
    /// Export the assembled board as a STEP file for mechanical CAD.
    pub fn export_step(&self, p: ExportStep) -> Result<StepExported> {
        let name = self.data.board.name.clone();
        let path = match p.path {
            Some(s) => std::path::PathBuf::from(s),
            None => self.root().join("output").join(format!("{name}.step")),
        };
        let writer = self
            .ports
            .step
            .as_ref()
            .ok_or_else(|| Error::invalid(Ports::missing("STEP writer")))?;
        let x = writer
            .export(
                &self.data,
                self.root(),
                &name,
                &lamina_ports::mesh::StepOptions {
                    vias: p.vias,
                    parts: !p.board_only,
                },
            )
            .map_err(Error::Invalid)?;
        write_atomic(&path, &x.bytes)?;
        Ok(StepExported {
            path: path.display().to_string(),
            exact: x.exact,
            meshed: x.meshed,
            bytes: x.bytes.len(),
            notes: x.notes,
        })
    }
}
