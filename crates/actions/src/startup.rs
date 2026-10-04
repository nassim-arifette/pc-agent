//! Reversible changes to the current user's Run approval state. Registry commands are never read.
use crate::{
    ActionError, ActionKind, Guard, ItemOutcome, Journal, JournalPhase, PreState, Refusal,
};
use pcagent_shared::{unix_ms, Status, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};

/// Only the observed 12-byte Windows approval formats are writable. Future formats fail closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "bytes", rename_all = "snake_case")]
pub enum StartupApproval {
    Missing,
    Binary([u8; 12]),
}
impl StartupApproval {
    pub fn from_bytes(bytes: Option<&[u8]>) -> Result<Self, Status> {
        let Some(bytes) = bytes else {
            return Ok(Self::Missing);
        };
        let data: [u8; 12] = bytes.try_into().map_err(|_| Status::Unsupported)?;
        let value = Self::Binary(data);
        if value.enabled().is_none() {
            return Err(Status::Unsupported);
        }
        Ok(value)
    }
    pub fn enabled(&self) -> Option<bool> {
        match self {
            Self::Missing => None,
            Self::Binary(data) => match u32::from_le_bytes(data[..4].try_into().unwrap()) {
                2 | 6 => Some(true),
                3 | 7 => Some(false),
                _ => None,
            },
        }
    }
    fn disabled(&self) -> Self {
        let state: u32 = if matches!(self, Self::Binary(data) if data[0] == 6) {
            7
        } else {
            3
        };
        let mut data = [0_u8; 12];
        data[..4].copy_from_slice(&state.to_le_bytes());
        let filetime = unix_ms()
            .saturating_add(11_644_473_600_000)
            .saturating_mul(10_000);
        data[4..].copy_from_slice(&filetime.to_le_bytes());
        Self::Binary(data)
    }
    pub(crate) fn known(&self) -> bool {
        matches!(self, Self::Missing) || self.enabled().is_some()
    }
}

/// The Run key timestamp detects entry replacement without reading command values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupIdentity {
    pub run_key_last_write_filetime: u64,
    pub approval: StartupApproval,
}

/// Local-only target; the model receives only its opaque reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupTarget {
    value_name: String,
    identity: StartupIdentity,
    desired_approval: StartupApproval,
}
impl StartupTarget {
    /// Prepare a local restoration only from one durably completed disable operation. Reading
    /// the journal never approves or executes it; the broker must revalidate the new plan too.
    pub fn restore_from_journal(
        journal: &Journal,
        source_plan_id: &str,
        item_index: usize,
        guard: &dyn Guard,
    ) -> Result<Self, ActionError> {
        let (records, malformed) = journal.read(source_plan_id)?;
        if malformed != 0
            || records.iter().any(|record| {
                record.schema_version != SCHEMA_VERSION || record.plan_id != source_plan_id
            })
        {
            return Err(ActionError::InvalidIntent("unverifiable startup journal"));
        }
        let records: Vec<_> = records
            .iter()
            .filter(|record| record.item_index == item_index)
            .collect();
        let [started, finished] = records.as_slice() else {
            return Err(ActionError::InvalidIntent(
                "startup disable is not durably completed",
            ));
        };
        if started.phase != JournalPhase::Started
            || finished.phase != JournalPhase::Finished
            || started.outcome.is_some()
            || finished.pre_state.is_some()
            || !matches!(finished.outcome, Some(ItemOutcome::Completed { .. }))
            || started.fingerprint.is_empty()
            || started.fingerprint != finished.fingerprint
            || started.target_ref != finished.target_ref
        {
            return Err(ActionError::InvalidIntent(
                "startup disable is not durably completed",
            ));
        }
        let Some(PreState::UserStartup { target }) = &started.pre_state else {
            return Err(ActionError::InvalidIntent(
                "journal item is not a startup disable",
            ));
        };
        target
            .prepare_restore(guard)
            .map_err(|reason| match reason {
                Refusal::Unverifiable { status } => ActionError::Resolve {
                    target_ref: source_plan_id.into(),
                    status,
                },
                _ => ActionError::IdentityChanged(source_plan_id.into()),
            })
    }

    /// Capture a user Run entry without its command, ready for local review and approval.
    pub fn disable(value_name: &str, guard: &dyn Guard) -> Result<Self, Status> {
        if !valid_name(value_name) {
            return Err(Status::Unsupported);
        }
        let identity = guard
            .observe_user_startup(value_name)?
            .ok_or(Status::Unsupported)?;
        if !identity.approval.known() || identity.approval.enabled() == Some(false) {
            return Err(Status::Unsupported);
        }
        Ok(Self {
            value_name: value_name.into(),
            desired_approval: identity.approval.disabled(),
            identity,
        })
    }
    /// Build a new approved restoration plan from a saved disable target (including a journal's
    /// started pre-state). Refuse if the entry or the approval has changed since that disable.
    pub fn prepare_restore(&self, guard: &dyn Guard) -> Result<Self, Refusal> {
        self.validate(ActionKind::DisableUserStartup)
            .map_err(|_| Refusal::StartupEntryChanged)?;
        let current = guard
            .observe_user_startup(&self.value_name)
            .map_err(|status| Refusal::Unverifiable { status })?
            .ok_or(Refusal::TargetMissing)?;
        if current.run_key_last_write_filetime != self.identity.run_key_last_write_filetime
            || current.approval != self.desired_approval
        {
            return Err(Refusal::StartupEntryChanged);
        }
        Ok(Self {
            value_name: self.value_name.clone(),
            identity: current,
            desired_approval: self.identity.approval.clone(),
        })
    }
    pub fn value_name(&self) -> &str {
        &self.value_name
    }
    pub fn identity(&self) -> &StartupIdentity {
        &self.identity
    }
    pub fn desired_approval(&self) -> &StartupApproval {
        &self.desired_approval
    }
    pub(crate) fn validate(&self, kind: ActionKind) -> Result<(), ActionError> {
        if !valid_name(&self.value_name)
            || self.identity.run_key_last_write_filetime == 0
            || !self.identity.approval.known()
            || !self.desired_approval.known()
            || self.identity.approval == self.desired_approval
        {
            return Err(ActionError::InvalidIntent("invalid user startup target"));
        }
        let allowed = match kind {
            ActionKind::DisableUserStartup => {
                self.identity.approval.enabled() != Some(false)
                    && self.desired_approval.enabled() == Some(false)
            }
            ActionKind::RestoreUserStartup => {
                self.identity.approval.enabled() == Some(false)
                    && self.desired_approval.enabled() != Some(false)
            }
            _ => false,
        };
        if allowed {
            Ok(())
        } else {
            Err(ActionError::InvalidIntent(
                "startup action and state do not match",
            ))
        }
    }
}

pub(crate) fn valid_name(value_name: &str) -> bool {
    !value_name.is_empty()
        && value_name.encode_utf16().count() <= 256
        && !value_name.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_approval_encodings_are_not_writable() {
        assert_eq!(
            StartupApproval::from_bytes(None).unwrap(),
            StartupApproval::Missing
        );
        for size in [0, 4, 11, 13, 4096] {
            assert_eq!(
                StartupApproval::from_bytes(Some(&vec![0; size])),
                Err(Status::Unsupported)
            );
        }
        for state in [0_u32, 1, 4, 8, 0x00000102, u32::MAX] {
            let mut bytes = [0_u8; 12];
            bytes[..4].copy_from_slice(&state.to_le_bytes());
            assert_eq!(
                StartupApproval::from_bytes(Some(&bytes)),
                Err(Status::Unsupported)
            );
        }
        for state in [2_u32, 6] {
            let mut bytes = [0_u8; 12];
            bytes[..4].copy_from_slice(&state.to_le_bytes());
            let value = StartupApproval::from_bytes(Some(&bytes)).unwrap();
            let StartupApproval::Binary(disabled) = value.disabled() else {
                panic!()
            };
            assert_eq!(
                u32::from_le_bytes(disabled[..4].try_into().unwrap()),
                state + 1
            );
            assert_eq!(StartupApproval::Binary(disabled).enabled(), Some(false));
        }
    }
}
