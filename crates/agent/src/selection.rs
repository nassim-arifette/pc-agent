//! Host-owned, already measured selections. Neither references nor model calls collect data.
mod projection;
mod reference;
mod source;
mod toolbox;

pub use projection::FrozenSelection;
pub use reference::{is_battery_ref, Resource, SelectionIntent, SelectionRef, SelectionTarget};
pub use source::{RecordingSource, SelectionSource, StorageSource};
pub use toolbox::{SelectedTools, GET_SELECTED_EVIDENCE, SELECTED_EVIDENCE};

pub const MAX_SELECTED_BYTES: usize = 128 * 1024;
pub const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_SOURCE_TARGETS: usize = 256;
const MAX_HISTORY_POINTS: usize = 30;

#[cfg(test)]
mod tests;
