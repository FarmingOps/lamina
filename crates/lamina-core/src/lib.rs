//! Lamina project operations. Each public operation is one MCP tool
//! and one CLI subcommand.

pub mod lock;
pub mod ops;
pub mod project;
pub mod reuse;
pub mod validate;
pub mod wiring;

pub use lamina_model as model;
pub use lamina_model::error::{Error, Result};
pub use lamina_ports as ports;
pub use lamina_ports::Ports;
pub use ops::*;
pub use project::{OpResult, Project, Touched, next_id};
