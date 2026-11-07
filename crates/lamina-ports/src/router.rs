//! Exchange with an external autorouter: write the board for it, read its
//! routes back.

use std::collections::BTreeSet;

use lamina_model::{ProjectData, Track, Via};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RouterInfo {
    /// The short id, for example `specctra`.
    pub id: String,
    pub name: String,
    pub description: String,
    /// The extension of the file Lamina writes, without the dot.
    pub design_extension: String,
    /// The extension of the file Lamina reads back, without the dot.
    pub session_extension: String,
}

/// The routes that came back from the router.
#[derive(Debug, Default)]
pub struct Session {
    pub tracks: Vec<Track>,
    pub vias: Vec<Via>,
    /// The nets that have routes in the session.
    pub nets: BTreeSet<String>,
    pub notes: Vec<String>,
}

pub trait RouterExchange: Send + Sync {
    fn info(&self) -> RouterInfo;
    /// The design file for the router. With `reroute_all`, leave the
    /// existing routes out.
    fn export(&self, data: &ProjectData, name: &str, reroute_all: bool) -> Result<String, String>;
    /// Read the routes from a session file.
    fn import_session(&self, text: &str, data: &ProjectData) -> Result<Session, String>;
}
