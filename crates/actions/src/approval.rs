use crate::{ActionError, ActionPlan, PlanState};
use pcagent_shared::unix_ms;
use std::{
    collections::HashMap,
    fmt,
    sync::{Mutex, MutexGuard},
    time::Duration,
};
use uuid::Uuid;

pub const MAX_APPROVAL_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_OUTSTANDING: usize = 256;

struct Issued {
    plan_id: String,
    fingerprint: String,
    expires_at_unix_ms: u64,
}

/// Trusted local UI session. Only code holding this value can issue or redeem approvals.
pub struct LocalSession {
    id: Uuid,
    issued: Mutex<HashMap<Uuid, Issued>>,
}
impl Default for LocalSession {
    fn default() -> Self {
        Self::new()
    }
}
impl fmt::Debug for LocalSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LocalSession(..)")
    }
}
impl LocalSession {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            issued: Mutex::new(HashMap::new()),
        }
    }
    fn issued(&self) -> MutexGuard<'_, HashMap<Uuid, Issued>> {
        self.issued.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Redeems a token once. The entry is removed before any check, so a rejected use still burns it.
    pub(crate) fn consume(
        &self,
        token: &ApprovalToken,
        plan: &ActionPlan,
    ) -> Result<(), ApprovalRejection> {
        if token.session != self.id {
            return Err(ApprovalRejection::WrongSession);
        }
        let issued = self
            .issued()
            .remove(&token.secret)
            .ok_or(ApprovalRejection::UnknownOrAlreadyUsed)?;
        if issued.plan_id != plan.plan_id() || issued.fingerprint != plan.fingerprint() {
            return Err(ApprovalRejection::PlanMismatch);
        }
        if unix_ms() >= issued.expires_at_unix_ms {
            return Err(ApprovalRejection::Expired);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalRejection {
    WrongSession,
    UnknownOrAlreadyUsed,
    PlanMismatch,
    Expired,
    TooManyOutstanding,
    InvalidTtl,
}

/// Single-use local consent bound to plan fingerprint, session and expiry. Deliberately not
/// `Clone`, `Serialize` or `Deserialize`; `Debug` never shows the secret. Keep it out of any
/// model context, log or IPC payload.
pub struct ApprovalToken {
    secret: Uuid,
    session: Uuid,
    plan_id: String,
    expires_at_unix_ms: u64,
}
impl fmt::Debug for ApprovalToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApprovalToken")
            .field("plan_id", &self.plan_id)
            .field("expires_at_unix_ms", &self.expires_at_unix_ms)
            .finish_non_exhaustive()
    }
}
impl ApprovalToken {
    pub fn plan_id(&self) -> &str {
        &self.plan_id
    }
    pub fn expires_at_unix_ms(&self) -> u64 {
        self.expires_at_unix_ms
    }
}

/// Call only from the trusted local UI in response to an explicit user gesture on this exact plan.
pub fn approve_locally(
    plan: &mut ActionPlan,
    session: &LocalSession,
    ttl: Duration,
) -> Result<ApprovalToken, ActionError> {
    if ttl.is_zero() {
        return Err(ActionError::ApprovalRejected(ApprovalRejection::InvalidTtl));
    }
    plan.verify_fingerprint()?;
    if plan.check_expiry() {
        return Err(ActionError::Expired);
    }
    if plan.state() != PlanState::AwaitingApproval {
        return Err(ActionError::IllegalTransition {
            from: plan.state(),
            to: PlanState::Approved,
        });
    }
    let now = unix_ms();
    let ttl_ms = u64::try_from(ttl.min(MAX_APPROVAL_TTL).as_millis()).unwrap_or(u64::MAX);
    let expires = now.saturating_add(ttl_ms).min(plan.expires_at_unix_ms());
    let mut issued = session.issued();
    issued.retain(|_, i| i.expires_at_unix_ms > now);
    if issued.len() >= MAX_OUTSTANDING {
        return Err(ActionError::ApprovalRejected(
            ApprovalRejection::TooManyOutstanding,
        ));
    }
    plan.transition(PlanState::Approved)?;
    let secret = Uuid::new_v4();
    issued.insert(
        secret,
        Issued {
            plan_id: plan.plan_id().into(),
            fingerprint: plan.fingerprint().into(),
            expires_at_unix_ms: expires,
        },
    );
    Ok(ApprovalToken {
        secret,
        session: session.id,
        plan_id: plan.plan_id().into(),
        expires_at_unix_ms: expires,
    })
}
