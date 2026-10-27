//! Native adapters implement the existing `Guard` and permit-gated `Effects` contracts.
mod adapter;
pub use adapter::{NativeEffects, NativeGuard};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod windows;
