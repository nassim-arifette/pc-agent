//! Shared, bounded cloud projections for both host previews and tool results.
//! These functions consume supplied measurements; they never start collectors or recordings.
pub mod activity;
pub mod history;
pub mod system;
pub mod watches;
