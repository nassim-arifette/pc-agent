//! Local implementations of the agent's tools. The host owns collectors and approvals;
//! the agent core receives only the bounded Toolbox contract and projected observations.
#![forbid(unsafe_code)]
mod action_targets;
pub mod activity_tools;
pub mod elevated_tools;
pub mod external_tools;
pub mod history_tools;
mod proposals;
pub mod system_tools;
pub mod tavily;
mod workstation;
pub use proposals::{ActionDesk, FixedTargets, PendingPlan, PolicyProtection, MAX_PENDING_PLANS};
pub use workstation::Workstation;
