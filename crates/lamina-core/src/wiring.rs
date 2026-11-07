//! The standard adapters, plugged into a [`Ports`] bundle.
//!
//! This file is the composition root while the ports are being introduced.
//! When every operation goes through a port, it moves out of `lamina-core`
//! into the binaries, and `lamina-core` stops depending on the adapter
//! crates.

use std::path::Path;
use std::sync::Arc;

use lamina_model::Workspace;
use lamina_ports::Ports;

/// The standard ports for a board in `board` (or none).
pub fn standard(board: Option<&Path>) -> Ports {
    let ws = board.and_then(Workspace::find);
    let fp_dirs = ws
        .as_ref()
        .map(|w| w.kicad_footprint_dirs())
        .unwrap_or_default();
    let m_dirs = ws
        .as_ref()
        .map(|w| w.kicad_model_dirs())
        .unwrap_or_default();
    Ports {
        providers: vec![
            Arc::new(lamina_parts::provider::LcscProvider::online()),
            Arc::new(lamina_kicad::provider::KicadProvider::online(
                fp_dirs, m_dirs,
            )),
        ],
        formats: vec![
            Arc::new(lamina_kicad::KicadFormat),
            Arc::new(lamina_parts::EasyedaFormat),
            Arc::new(lamina_export::GerberFormat),
        ],
        routers: vec![Arc::new(lamina_specctra::SpecctraExchange)],
        ..Ports::default()
    }
}
