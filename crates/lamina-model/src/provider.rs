//! Part providers. A provider is a place that gives footprints, 3D models
//! and part data: a parts vendor, a footprint library, a folder on disk.
//! Lamina does not depend on any one of them. Each provider implements
//! [`Provider`], and the core treats them all the same way: search, fetch,
//! cache in the project, keep a copy in the library.
//!
//! A part is named `provider:id`. The id format belongs to the provider.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::footprint::{Footprint, ModelRef};

/// What a provider is and how its ids look.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ProviderInfo {
    /// The short id that goes before the colon, for example `lcsc`.
    pub id: String,
    pub name: String,
    /// What the provider gives and how to use it.
    pub description: String,
    /// An example of a full part name, for example `lcsc:C6186`.
    pub example: String,
    /// True if a fetch can need the network.
    pub online: bool,
}

/// One search result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Hit {
    pub provider: String,
    /// The id inside the provider. `provider:id` is the full part name.
    pub id: String,
    /// The part name or the footprint name.
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stock: Option<u64>,
    /// The unit price in USD at quantity 1, if it is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
    /// True for a part with no extra assembly fee (a vendor "basic" part).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasheet: Option<String>,
    /// True if the part is a file on this computer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
}

/// Part data for the BOM.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PartInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mpn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasheet: Option<String>,
    /// The order code at the parts vendor, if the provider sells parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_code: Option<String>,
}

/// A 3D model file with its placement on the footprint.
#[derive(Debug, Clone)]
pub struct ModelFile {
    /// The file name, with the extension (`step`, `stp` or `obj`).
    pub name: String,
    pub bytes: Vec<u8>,
    pub offset: [f64; 3],
    pub rotation: [f64; 3],
    pub scale: [f64; 3],
    /// The top of the placed model above the board, if the provider knows it.
    pub height: Option<f64>,
}

/// What a fetch gives: the footprint, its model and the part data.
#[derive(Debug, Clone)]
pub struct Bundle {
    pub footprint: Footprint,
    pub model: Option<ModelFile>,
    pub info: PartInfo,
    /// What the provider skipped or made approximate.
    pub notes: Vec<String>,
}

/// A fetched part as the project cache and the library keep it. The model
/// file is next to it under `models/<provider>/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub provider: String,
    pub id: String,
    pub footprint: Footprint,
    pub info: PartInfo,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Record {
    pub fn model(&self) -> Option<&ModelRef> {
        self.footprint.model.as_ref()
    }
}

pub trait Provider: Send + Sync {
    fn info(&self) -> ProviderInfo;
    /// Search by words, a part number or an id.
    fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>, String>;
    /// Get a part. With `model: false`, do not get the 3D model.
    fn fetch(&self, id: &str, model: bool) -> Result<Bundle, String>;
    /// Get only a 3D model by id, for `set_model`.
    fn model(&self, _id: &str) -> Result<ModelFile, String> {
        Err(format!(
            "The provider `{}` does not give models by id.",
            self.info().id
        ))
    }
}

/// Split `provider:id`. The id can have more colons.
pub fn split_name(name: &str) -> Option<(&str, &str)> {
    let (p, id) = name.trim().split_once(':')?;
    let p = p.trim();
    let id = id.trim();
    let ok = !p.is_empty()
        && !id.is_empty()
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    ok.then_some((p, id))
}

/// A file name for an id: every character that is not safe in a file name
/// becomes `_`.
pub fn id_file(id: &str) -> String {
    let mut s: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    s.truncate(120);
    s
}

/// The record file of a part, relative to a cache or a library root.
pub fn record_rel(provider: &str, id: &str) -> String {
    format!("parts/{provider}/{}.json", id_file(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_files() {
        assert_eq!(split_name("lcsc:C6186"), Some(("lcsc", "C6186")));
        assert_eq!(
            split_name("kicad:LED_SMD:LED_0603"),
            Some(("kicad", "LED_SMD:LED_0603"))
        );
        assert_eq!(split_name("C6186"), None);
        assert_eq!(split_name("bad id:x"), None);
        assert_eq!(id_file("LED_SMD:LED_0603/x"), "LED_SMD_LED_0603_x");
        assert_eq!(record_rel("lcsc", "C6186"), "parts/lcsc/C6186.json");
    }
}
