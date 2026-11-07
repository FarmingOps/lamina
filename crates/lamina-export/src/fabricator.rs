//! The port adapters of this crate: the Gerber fabricator and the STEP
//! exporter.

use std::path::Path;

use lamina_geom::BoardGeometry;
use lamina_model::ProjectData;
use lamina_ports::fab::{FabInfo, Fabricator, Output};
use lamina_ports::mesh::{StepExport, StepOptions, StepWriter};
use lamina_scene::Scene2D;

/// Gerber X2 artwork, Excellon drills, and the JLCPCB BOM and CPL files.
#[derive(Debug, Clone, Copy, Default)]
pub struct GerberFabricator;

impl Fabricator for GerberFabricator {
    fn info(&self) -> FabInfo {
        FabInfo {
            id: "gerber".into(),
            name: "Gerber X2".into(),
            description: "Gerber X2 artwork, Excellon drill files (PTH and NPTH),                           a JLCPCB BOM and CPL, and a zip of the artwork."
                .into(),
        }
    }

    fn artwork(&self, scene: &Scene2D, geometry: &BoardGeometry, name: &str) -> Vec<Output> {
        let mut out = crate::gerbers(scene, name);
        out.extend(crate::drills(geometry, name));
        out
    }

    fn assembly(&self, data: &ProjectData, name: &str) -> (Vec<Output>, Vec<String>) {
        crate::assembly(data, name)
    }

    fn archive(&self, artwork: &[Output], name: &str) -> Result<Option<Output>, String> {
        crate::archive(artwork, name).map(Some)
    }
}

/// Writes the assembled board as a STEP (AP214) file.
#[derive(Debug, Clone, Copy, Default)]
pub struct StepExporter;

impl StepWriter for StepExporter {
    fn export(
        &self,
        data: &ProjectData,
        root: &Path,
        name: &str,
        options: &StepOptions,
    ) -> Result<StepExport, String> {
        crate::step_out::export_step(data, root, name, options)
    }
}
