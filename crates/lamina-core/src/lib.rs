//! Lamina project operations. Each public operation is one MCP tool
//! and one CLI subcommand.
//!
//! The core talks to the outside world through `lamina_ports` only. It
//! does not depend on an adapter crate. A project is opened with a
//! [`Ports`] bundle; `lamina-wiring` builds the standard one.

pub mod lock;
pub mod ops;
pub mod project;
pub mod reuse;
pub mod validate;

pub use lamina_model as model;
pub use lamina_model::error::{Error, Result};
pub use lamina_ports as ports;
pub use lamina_ports::Ports;
pub use ops::*;
pub use project::{OpResult, Project, Touched, next_id};
