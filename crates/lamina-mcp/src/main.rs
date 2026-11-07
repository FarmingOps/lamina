mod server;

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use lamina_core::Project;
use rmcp::ServiceExt;

#[derive(Parser)]
#[command(
    name = "lamina",
    version,
    about = "PCB layout engine for coding agents"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the MCP server on stdio.
    Mcp {
        /// Open this project when the server starts.
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Create a new project.
    New {
        dir: PathBuf,
        #[arg(long)]
        name: Option<String>,
        /// Start from a modular design.jsonnet source.
        #[arg(long)]
        modular: bool,
    },
    /// Build the design.jsonnet of a board into its JSON files.
    Build { dir: PathBuf },
    /// Import a board file of another program into a project. If the project
    /// does not exist, create it. Run `lamina formats` for the formats.
    Import {
        /// The board file, or a folder or zip of Gerber files.
        file: PathBuf,
        dir: PathBuf,
        /// The format id. The default is the format that detects the file.
        #[arg(long)]
        format: Option<String>,
        /// Do not get the 3D models.
        #[arg(long)]
        no_models: bool,
        /// Replace a board that is already in the project.
        #[arg(long)]
        replace: bool,
    },
    /// List the board formats that Lamina reads or writes.
    Formats,
    /// Write the assembled board as a STEP file.
    ExportStep {
        dir: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        /// Also drill the vias through the board.
        #[arg(long)]
        vias: bool,
        /// Do not include the part models.
        #[arg(long)]
        board_only: bool,
    },
    /// Write a project as a board file of another program.
    ExportBoard {
        dir: PathBuf,
        /// The format id, for example `kicad`.
        #[arg(long)]
        format: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Write the design file for an external autorouter.
    ExportRouting {
        dir: PathBuf,
        /// The router id, for example `specctra`.
        #[arg(long)]
        router: String,
        #[arg(long)]
        out: Option<PathBuf>,
        /// Do not include the existing routes.
        #[arg(long)]
        reroute_all: bool,
    },
    /// Read the session file of an external autorouter back into a project.
    ImportRouting {
        dir: PathBuf,
        file: PathBuf,
        /// The router id, for example `specctra`.
        #[arg(long)]
        router: String,
    },
    /// List the external autorouters that Lamina exchanges files with.
    Routers,
    /// Copy the layout of one block to other blocks. A block is a group name
    /// or a list of refdes with commas between them (`D1,R1`).
    RepeatLayout {
        dir: PathBuf,
        #[arg(long)]
        from: String,
        #[arg(long, required = true)]
        to: Vec<String>,
        /// The part that does not move (in the source or in a target).
        #[arg(long)]
        anchor: Option<String>,
        /// Copy only the placement.
        #[arg(long)]
        no_routing: bool,
        /// Also move locked parts.
        #[arg(long)]
        force: bool,
    },
    /// Save the layout of a block as layouts/<name>.json (in the workspace library,
    /// or in the board if there is no workspace).
    SaveLayout {
        dir: PathBuf,
        #[arg(long)]
        from: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        anchor: Option<String>,
        #[arg(long)]
        replace: bool,
    },
    /// Put a saved layout on blocks of a board.
    PlaceLayout {
        dir: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long, required = true)]
        to: Vec<String>,
        /// The target part that does not move.
        #[arg(long)]
        anchor: Option<String>,
        #[arg(long)]
        no_routing: bool,
        #[arg(long)]
        force: bool,
    },
    /// Create a workspace (lamina.json with a lib folder) in a folder.
    Workspace {
        dir: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    /// Download parts into the part library, so that boards can import them
    /// without a network. A part is provider:id, for example lcsc:C6186 or
    /// kicad:LED_SMD:LED_0603_1608Metric.
    Fetch {
        parts: Vec<String>,
        /// Download again, also when the library has the files.
        #[arg(long)]
        refresh: bool,
    },
    /// Print the part library folders and contents as JSON.
    Library,
    /// Print the project status as JSON.
    Status { dir: PathBuf },
    /// List the rule violations as JSON.
    Check { dir: PathBuf },
    /// Print the JSON schema of a file kind.
    Schema { kind: String },
}

/// A block on the command line: `name` is a group, `A,B,C` is a part list.
fn block(s: &str) -> lamina_core::reuse::Block {
    if s.contains(',') {
        lamina_core::reuse::Block::Parts(
            s.split(',')
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty())
                .collect(),
        )
    } else {
        lamina_core::reuse::Block::Group(s.to_string())
    }
}

/// Hook for checkers from other crates (DRC). Lamina calls it each time a
/// project opens.
pub fn register_checkers(_p: &mut Project) {}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Mcp { project } => run_mcp(project),
        Cmd::New { dir, name, modular } => {
            let path = dir.to_string_lossy().to_string();
            if lamina_core::model::ProjectData::is_project(&dir) {
                anyhow::bail!("{path} already contains a project");
            }
            let p = server::open_project(&path, true, name, modular)?;
            if let Some(mut ws) = lamina_core::model::Workspace::find(&dir) {
                ws.add_board(&dir)?;
            }
            println!("{}", serde_json::to_string_pretty(&p.status())?);
            Ok(())
        }
        Cmd::Import {
            file,
            dir,
            format,
            no_models,
            replace,
        } => {
            let path = dir.to_string_lossy().to_string();
            let name = file
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string);
            let mut p = server::open_project(&path, true, name, false)?;
            let r = p.import_board(lamina_core::ImportBoard {
                path: file.to_string_lossy().to_string(),
                format,
                models: !no_models,
                replace,
                strict: false,
            })?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        Cmd::Formats => {
            let f = lamina_wiring::standard(None).formats;
            let infos: Vec<_> = f.iter().map(|f| f.info()).collect();
            println!("{}", serde_json::to_string_pretty(&infos)?);
            Ok(())
        }
        Cmd::Routers => {
            let r = lamina_wiring::standard(None).routers;
            let infos: Vec<_> = r.iter().map(|r| r.info()).collect();
            println!("{}", serde_json::to_string_pretty(&infos)?);
            Ok(())
        }
        Cmd::ExportStep {
            dir,
            out,
            vias,
            board_only,
        } => {
            let p = server::open_project(&dir.to_string_lossy(), false, None, false)?;
            let r = p.export_step(lamina_core::ExportStep {
                path: out.map(|o| o.to_string_lossy().to_string()),
                vias,
                board_only,
            })?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        Cmd::ExportBoard { dir, format, out } => {
            let p = server::open_project(&dir.to_string_lossy(), false, None, false)?;
            let r = p.export_board(lamina_core::ExportBoard {
                format,
                path: out.map(|o| o.to_string_lossy().to_string()),
            })?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        Cmd::ExportRouting {
            dir,
            router,
            out,
            reroute_all,
        } => {
            let p = server::open_project(&dir.to_string_lossy(), false, None, false)?;
            let r = p.export_for_router(lamina_core::ExportForRouter {
                router,
                path: out.map(|o| o.to_string_lossy().to_string()),
                reroute_all,
            })?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        Cmd::ImportRouting { dir, file, router } => {
            let mut p = server::open_project(&dir.to_string_lossy(), false, None, false)?;
            let r = p.import_router_session(lamina_core::ImportRouterSession {
                router,
                path: file.to_string_lossy().to_string(),
                replace: true,
                strict: false,
            })?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        Cmd::RepeatLayout {
            dir,
            from,
            to,
            anchor,
            no_routing,
            force,
        } => {
            let mut p = server::open_project(&dir.to_string_lossy(), false, None, false)?;
            let r = p.repeat_layout(lamina_core::RepeatLayout {
                from: block(&from),
                to: to.iter().map(|t| block(t)).collect(),
                anchor,
                at: None,
                routing: !no_routing,
                force,
                strict: false,
            })?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        Cmd::SaveLayout {
            dir,
            from,
            name,
            anchor,
            replace,
        } => {
            let p = server::open_project(&dir.to_string_lossy(), false, None, false)?;
            let r = p.save_layout_block(lamina_core::SaveLayoutBlock {
                from: block(&from),
                name,
                anchor,
                replace,
            })?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        Cmd::PlaceLayout {
            dir,
            name,
            to,
            anchor,
            no_routing,
            force,
        } => {
            let mut p = server::open_project(&dir.to_string_lossy(), false, None, false)?;
            let r = p.place_layout_block(lamina_core::PlaceLayoutBlock {
                name,
                to: to.iter().map(|t| block(t)).collect(),
                anchor,
                at: None,
                routing: !no_routing,
                force,
                strict: false,
            })?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        Cmd::Build { dir } => {
            let mut p = server::open_project(&dir.to_string_lossy(), false, None, false)?;
            println!("{}", serde_json::to_string_pretty(&p.build()?)?);
            Ok(())
        }
        Cmd::Workspace { dir, name } => {
            let name = name.unwrap_or_else(|| {
                dir.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("workspace")
                    .to_string()
            });
            let ws = lamina_core::model::Workspace::create(&dir, &name)?;
            println!("{}", serde_json::to_string_pretty(&ws.file)?);
            Ok(())
        }
        Cmd::Fetch { parts, refresh } => {
            let r = lamina_core::provider::fetch(
                &lamina_core::model::Library::user(),
                &lamina_core::Providers::new(lamina_wiring::standard(None).providers),
                &lamina_core::FetchLibrary { parts, refresh },
            )?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            if r.failed.is_empty() {
                Ok(())
            } else {
                anyhow::bail!("{} item(s) failed", r.failed.len())
            }
        }
        Cmd::Library => {
            let info = lamina_core::provider::library_info(&lamina_core::model::Library::user());
            println!("{}", serde_json::to_string_pretty(&info)?);
            Ok(())
        }
        Cmd::Status { dir } => {
            let p = server::read_project(&dir.to_string_lossy())?;
            println!("{}", serde_json::to_string_pretty(&p.status())?);
            Ok(())
        }
        Cmd::Check { dir } => {
            let p = server::read_project(&dir.to_string_lossy())?;
            println!("{}", serde_json::to_string_pretty(&p.check())?);
            Ok(())
        }
        Cmd::Schema { kind } => {
            let s = lamina_core::model::schema_for(&kind)
                .with_context(|| format!("unknown kind `{kind}`"))?;
            println!("{}", serde_json::to_string_pretty(&s)?);
            Ok(())
        }
    }
}

fn run_mcp(project: Option<PathBuf>) -> anyhow::Result<()> {
    let initial = match project {
        Some(dir) => Some(server::open_project(
            &dir.to_string_lossy(),
            false,
            None,
            false,
        )?),
        None => None,
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let service = server::Lamina::new(initial)
            .serve(rmcp::transport::stdio())
            .await
            .context("cannot start the MCP server")?;
        service.waiting().await?;
        Ok(())
    })
}
