//! Local watch definitions and immutable alert measurements. Only the sampler owns the lease.
use crate::{stored, EventObservation, Store, StoreError, SAMPLE_RETENTION_MS};
use pcagent_shared::new_id;
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

pub(super) const MIGRATION: &str = "
BEGIN;
CREATE TABLE watches (
 id TEXT PRIMARY KEY, target TEXT NOT NULL UNIQUE, rule TEXT NOT NULL,
 enabled INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
 last_notified INTEGER
);
CREATE TABLE watch_alerts (
 id TEXT PRIMARY KEY, watch_id TEXT NOT NULL REFERENCES watches(id) ON DELETE CASCADE,
 owner TEXT NOT NULL, started_at INTEGER NOT NULL, last_checked INTEGER NOT NULL,
 ended_at INTEGER, seen INTEGER NOT NULL DEFAULT 0,
 notification TEXT NOT NULL, notified_at INTEGER, payload TEXT NOT NULL
);
CREATE INDEX watch_alerts_time ON watch_alerts(started_at DESC, id DESC);
CREATE UNIQUE INDEX watch_one_open ON watch_alerts(watch_id) WHERE ended_at IS NULL;
CREATE TABLE watch_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE watch_lease (
 slot INTEGER PRIMARY KEY CHECK(slot=1), owner TEXT NOT NULL,
 heartbeat INTEGER NOT NULL, expires_at INTEGER NOT NULL
);
PRAGMA user_version=7;
COMMIT;";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchResource {
    Cpu,
    Memory,
    DiskBusy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WatchRule {
    VolumeFreeBelow {
        root: String,
        threshold_bytes: u64,
    },
    ResourceAbove {
        resource: WatchResource,
        threshold_basis_points: u64,
        sustained_seconds: u64,
    },
    GpuTemperatureAbove {
        nvml_index: u32,
        threshold_celsius: u64,
        sustained_seconds: u64,
    },
    AppCpuAbove {
        executable: String,
        threshold_basis_points: u64,
        sustained_seconds: u64,
    },
    AppMemoryAbove {
        executable: String,
        threshold_bytes: u64,
        sustained_seconds: u64,
    },
}

impl WatchRule {
    pub fn validate(&self) -> Result<(), StoreError> {
        let valid = match self {
            Self::VolumeFreeBelow {
                root,
                threshold_bytes,
            } => {
                let bytes = root.as_bytes();
                bytes.len() == 3
                    && bytes[0].is_ascii_alphabetic()
                    && &bytes[1..] == b":\\"
                    && *threshold_bytes >= 1 << 30
                    && *threshold_bytes <= i64::MAX as u64
            }
            Self::ResourceAbove {
                threshold_basis_points,
                ..
            } => (1000..=10_000).contains(threshold_basis_points),
            Self::GpuTemperatureAbove {
                threshold_celsius, ..
            } => (50..=105).contains(threshold_celsius),
            Self::AppCpuAbove {
                executable,
                threshold_basis_points,
                ..
            } => valid_executable(executable) && (100..=10_000).contains(threshold_basis_points),
            Self::AppMemoryAbove {
                executable,
                threshold_bytes,
                ..
            } => {
                valid_executable(executable)
                    && (64 * 1024 * 1024..=1_u64 << 40).contains(threshold_bytes)
            }
        };
        if valid && matches!(self.sustained_seconds(), 0 | 60 | 300 | 900) {
            Ok(())
        } else {
            Err(StoreError::InvalidQuery)
        }
    }

    pub fn target(&self) -> String {
        match self {
            Self::VolumeFreeBelow { root, .. } => format!("volume:{}", root.to_ascii_uppercase()),
            Self::ResourceAbove { resource, .. } => format!("resource:{resource:?}"),
            Self::GpuTemperatureAbove { nvml_index, .. } => format!("gpu:{nvml_index}"),
            Self::AppCpuAbove { executable, .. } => {
                format!("app_cpu:{}", executable.to_lowercase())
            }
            Self::AppMemoryAbove { executable, .. } => {
                format!("app_memory:{}", executable.to_lowercase())
            }
        }
    }

    pub fn threshold(&self) -> u64 {
        match self {
            Self::VolumeFreeBelow {
                threshold_bytes, ..
            }
            | Self::AppMemoryAbove {
                threshold_bytes, ..
            } => *threshold_bytes,
            Self::ResourceAbove {
                threshold_basis_points,
                ..
            }
            | Self::AppCpuAbove {
                threshold_basis_points,
                ..
            } => *threshold_basis_points,
            Self::GpuTemperatureAbove {
                threshold_celsius, ..
            } => *threshold_celsius,
        }
    }

    pub fn sustained_seconds(&self) -> u64 {
        match self {
            Self::VolumeFreeBelow { .. } => 0,
            Self::ResourceAbove {
                sustained_seconds, ..
            }
            | Self::GpuTemperatureAbove {
                sustained_seconds, ..
            }
            | Self::AppCpuAbove {
                sustained_seconds, ..
            }
            | Self::AppMemoryAbove {
                sustained_seconds, ..
            } => *sustained_seconds,
        }
    }

    pub fn is_volume(&self) -> bool {
        matches!(self, Self::VolumeFreeBelow { .. })
    }
    pub fn is_application(&self) -> bool {
        matches!(self, Self::AppCpuAbove { .. } | Self::AppMemoryAbove { .. })
    }
    pub fn exceeded(&self, value: u64) -> bool {
        if self.is_volume() {
            value < self.threshold()
        } else {
            value >= self.threshold()
        }
    }
    pub fn returned(&self, value: u64) -> bool {
        let threshold = self.threshold();
        match self {
            Self::VolumeFreeBelow { .. } => {
                value >= threshold.saturating_add((1 << 30).max(threshold / 10))
            }
            Self::GpuTemperatureAbove { .. } => value <= threshold - 5,
            Self::AppMemoryAbove { .. } => value <= threshold * 9 / 10,
            _ => value <= threshold - 500.min(threshold / 2),
        }
    }
}

fn valid_executable(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= 260
        && !value
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    pub id: String,
    pub version: u32,
    pub rule: WatchRule,
    pub enabled: bool,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub last_notified_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchCloseReason {
    Returned,
    NoLongerObservedAbove,
    Disabled,
    Modified,
    Interrupted,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchNotification {
    Pending,
    Sent,
    Disabled,
    Failed,
    RuleCooldown,
    HourlyLimit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchAlert {
    pub id: String,
    pub watch_id: String,
    pub rule: WatchRule,
    pub started_at_unix_ms: u64,
    pub last_checked_unix_ms: u64,
    pub ended_at_unix_ms: Option<u64>,
    pub close_reason: Option<WatchCloseReason>,
    pub observation: EventObservation,
    pub extreme_observation: EventObservation,
    pub lower_bound: bool,
    pub notification: WatchNotification,
    pub seen: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WatchAlertPage {
    pub alerts: Vec<WatchAlert>,
    pub total: u64,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WatchSummary {
    pub from_unix_ms: u64,
    pub to_unix_ms: u64,
    pub total: u64,
    pub open: u64,
    pub returned: u64,
    pub interrupted: u64,
    pub by_measure: BTreeMap<String, u64>,
    pub local_only: bool,
}

fn encode<T: Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|_| StoreError::Corrupt)
}
fn decode<T: serde::de::DeserializeOwned>(value: String) -> Result<T, StoreError> {
    serde_json::from_str(&value).map_err(|_| StoreError::Corrupt)
}

impl Store {
    pub fn watches(&self) -> Result<Vec<Watch>, StoreError> {
        let mut stmt = self.conn.prepare("SELECT id,rule,enabled,created_at,updated_at,last_notified FROM watches ORDER BY created_at,id")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get(2)?,
                    row.get::<_, i64>(3)? as u64,
                    row.get::<_, i64>(4)? as u64,
                    row.get::<_, Option<i64>>(5)?.map(|v| v as u64),
                ))
            })?
            .map(|row| {
                let (
                    id,
                    rule,
                    enabled,
                    created_at_unix_ms,
                    updated_at_unix_ms,
                    last_notified_unix_ms,
                ) = row?;
                Ok(Watch {
                    id,
                    version: 1,
                    rule: decode(rule)?,
                    enabled,
                    created_at_unix_ms,
                    updated_at_unix_ms,
                    last_notified_unix_ms,
                })
            })
            .collect();
        rows
    }

    pub fn watch_create(&self, rule: WatchRule, now: u64) -> Result<Watch, StoreError> {
        rule.validate()?;
        let watch = Watch {
            id: new_id("watch"),
            version: 1,
            rule,
            enabled: true,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            last_notified_unix_ms: None,
        };
        self.conn.execute("INSERT INTO watches(id,target,rule,enabled,created_at,updated_at) VALUES(?1,?2,?3,1,?4,?4)",
            params![watch.id, watch.rule.target(), encode(&watch.rule)?, stored(now)])?;
        Ok(watch)
    }

    pub fn watch_update(&self, id: &str, rule: WatchRule, now: u64) -> Result<(), StoreError> {
        rule.validate()?;
        let tx = self.conn.unchecked_transaction()?;
        if tx.execute(
            "UPDATE watches SET target=?2,rule=?3,updated_at=?4 WHERE id=?1",
            params![id, rule.target(), encode(&rule)?, stored(now)],
        )? == 0
        {
            return Err(StoreError::InvalidQuery);
        }
        self.close_watch_alerts(Some(id), None, WatchCloseReason::Modified)?;
        tx.commit()?;
        Ok(())
    }

    pub fn watch_set_enabled(&self, id: &str, enabled: bool, now: u64) -> Result<(), StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        if tx.execute(
            "UPDATE watches SET enabled=?2,updated_at=?3 WHERE id=?1",
            params![id, enabled, stored(now)],
        )? == 0
        {
            return Err(StoreError::InvalidQuery);
        }
        if !enabled {
            self.close_watch_alerts(Some(id), None, WatchCloseReason::Disabled)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn watch_delete(&self, id: &str) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM watches WHERE id=?1", [id])?;
        Ok(())
    }

    /// One conditional SQLite statement, including first acquisition and future-clock recovery.
    pub fn watch_lease(&self, owner: &str, now: u64) -> Result<bool, StoreError> {
        Ok(self.conn.execute(
            "INSERT INTO watch_lease(slot,owner,heartbeat,expires_at) VALUES(1,?1,?2,?3)
             ON CONFLICT(slot) DO UPDATE SET owner=excluded.owner,heartbeat=excluded.heartbeat,expires_at=excluded.expires_at
             WHERE watch_lease.owner=excluded.owner OR watch_lease.expires_at<=excluded.heartbeat OR watch_lease.heartbeat>excluded.heartbeat",
            params![owner, stored(now), stored(now.saturating_add(30_000))])? == 1)
    }

    pub fn owns_watch_lease(&self, owner: &str) -> Result<bool, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT owner=?1 FROM watch_lease WHERE slot=1",
                [owner],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub fn watch_release(&self, owner: &str) -> Result<(), StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        self.close_watch_alerts(None, Some(owner), WatchCloseReason::Interrupted)?;
        tx.execute("DELETE FROM watch_lease WHERE owner=?1", [owner])?;
        tx.commit()?;
        Ok(())
    }

    /// On takeover, only old-owner alerts close; a second, paused window never closes live ones.
    pub fn watch_recover(&self, owner: &str) -> Result<(), StoreError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        if !self.owns_watch_lease(owner)? {
            return Ok(());
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM watch_alerts WHERE ended_at IS NULL AND owner<>?1")?;
        let ids = stmt
            .query_map([owner], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for id in ids {
            if let Some(mut alert) = self.watch_alert(&id)? {
                alert.ended_at_unix_ms = Some(alert.last_checked_unix_ms);
                alert.close_reason = Some(WatchCloseReason::Interrupted);
                self.save_watch_alert(&alert)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn close_watch_alerts(
        &self,
        watch: Option<&str>,
        owner: Option<&str>,
        reason: WatchCloseReason,
    ) -> Result<(), StoreError> {
        let mut stmt = self.conn.prepare("SELECT id FROM watch_alerts WHERE ended_at IS NULL AND (?1 IS NULL OR watch_id=?1) AND (?2 IS NULL OR owner=?2)")?;
        let ids = stmt
            .query_map(params![watch, owner], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for id in ids {
            if let Some(mut alert) = self.watch_alert(&id)? {
                alert.ended_at_unix_ms = Some(alert.last_checked_unix_ms);
                alert.close_reason = Some(reason);
                self.save_watch_alert(&alert)?;
            }
        }
        Ok(())
    }

    pub fn watch_alert(&self, id: &str) -> Result<Option<WatchAlert>, StoreError> {
        let row: Option<(String, bool)> = self
            .conn
            .query_row(
                "SELECT payload,seen FROM watch_alerts WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(payload, seen)| {
            let mut alert: WatchAlert = decode(payload)?;
            alert.seen = seen;
            Ok(alert)
        })
        .transpose()
    }

    pub fn open_watch_alert(
        &self,
        owner: &str,
        watch: &Watch,
        observation: EventObservation,
        now: u64,
    ) -> Result<Option<WatchAlert>, StoreError> {
        let alert = WatchAlert {
            id: new_id("alert"),
            watch_id: watch.id.clone(),
            rule: watch.rule.clone(),
            started_at_unix_ms: now,
            last_checked_unix_ms: now,
            ended_at_unix_ms: None,
            close_reason: None,
            lower_bound: watch.rule.is_application(),
            extreme_observation: observation.clone(),
            observation,
            notification: WatchNotification::Pending,
            seen: false,
        };
        // Validate ownership and the exact current rule in the same statement as opening.
        let changed = self.conn.execute(
            "INSERT INTO watch_alerts(id,watch_id,owner,started_at,last_checked,notification,payload)
             SELECT ?1,?2,?3,?4,?4,'pending',?5 WHERE EXISTS(SELECT 1 FROM watch_lease WHERE owner=?3 AND heartbeat<=?4 AND expires_at>?4)
             AND EXISTS(SELECT 1 FROM watches WHERE id=?2 AND enabled=1 AND rule=?6)",
            params![alert.id, watch.id, owner, stored(now), encode(&alert)?, encode(&watch.rule)?])?;
        Ok((changed == 1).then_some(alert))
    }

    pub(crate) fn save_watch_alert(&self, alert: &WatchAlert) -> Result<(), StoreError> {
        self.conn.execute("UPDATE watch_alerts SET last_checked=?2,ended_at=?3,notification=?4,payload=?5 WHERE id=?1",
            params![alert.id, stored(alert.last_checked_unix_ms), alert.ended_at_unix_ms.map(stored),
                notification_key(alert.notification), encode(alert)?])?;
        Ok(())
    }

    pub fn watch_alerts(
        &self,
        from: u64,
        to: u64,
        limit: u64,
        offset: u64,
    ) -> Result<WatchAlertPage, StoreError> {
        if from > to
            || to - from > SAMPLE_RETENTION_MS
            || !(1..=200).contains(&limit)
            || offset > i64::MAX as u64
        {
            return Err(StoreError::InvalidQuery);
        }
        let tx = self.conn.unchecked_transaction()?;
        let total: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM watch_alerts WHERE started_at BETWEEN ?1 AND ?2",
            params![stored(from), stored(to)],
            |r| r.get::<_, i64>(0),
        )? as u64;
        let mut stmt = self.conn.prepare("SELECT id FROM watch_alerts WHERE started_at BETWEEN ?1 AND ?2 ORDER BY started_at DESC,id DESC LIMIT ?3 OFFSET ?4")?;
        let ids = stmt
            .query_map(
                params![stored(from), stored(to), stored(limit), stored(offset)],
                |r| r.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let alerts = ids
            .iter()
            .map(|id| self.watch_alert(id)?.ok_or(StoreError::Corrupt))
            .collect::<Result<Vec<_>, _>>()?;
        let next = offset.saturating_add(alerts.len() as u64);
        tx.commit()?;
        Ok(WatchAlertPage {
            alerts,
            total,
            next_offset: (next < total).then_some(next),
        })
    }

    pub fn watch_alerts_seen(&self, up_to: u64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE watch_alerts SET seen=1 WHERE started_at<=?1",
            [stored(up_to)],
        )?;
        Ok(())
    }
    pub fn watch_unseen(&self) -> Result<u64, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM watch_alerts WHERE seen=0", [], |r| {
                r.get::<_, i64>(0)
            })? as u64)
    }
    pub fn watch_language(&self) -> Result<String, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM watch_settings WHERE key='language'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| "en".into()))
    }
    pub fn watch_language_set(&self, language: &str) -> Result<(), StoreError> {
        if !matches!(language, "en" | "fr") {
            return Err(StoreError::InvalidQuery);
        }
        self.conn.execute("INSERT INTO watch_settings(key,value) VALUES('language',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [language])?;
        Ok(())
    }

    /// Reserve notification delivery atomically; failed/disabled delivery is not silently retried.
    pub fn watch_notification_reserve(
        &self,
        id: &str,
        now: u64,
    ) -> Result<Option<WatchAlert>, StoreError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some(mut alert) = self.watch_alert(id)? else {
            return Ok(None);
        };
        if alert.notification != WatchNotification::Pending {
            return Ok(None);
        }
        let last = tx
            .query_row(
                "SELECT last_notified FROM watches WHERE id=?1",
                [&alert.watch_id],
                |r| r.get::<_, Option<i64>>(0),
            )?
            .map(|v| v as u64);
        let recent: u64 = tx.query_row(
            "SELECT COUNT(*) FROM watch_alerts WHERE notified_at BETWEEN ?1 AND ?2",
            params![stored(now.saturating_sub(3_600_000)), stored(now)],
            |r| r.get::<_, i64>(0),
        )? as u64;
        alert.notification = if last.is_some_and(|last| now >= last && now - last < 1_800_000) {
            WatchNotification::RuleCooldown
        } else if recent >= 6 {
            WatchNotification::HourlyLimit
        } else {
            WatchNotification::Failed
        };
        self.save_watch_alert(&alert)?;
        if alert.notification == WatchNotification::Failed {
            tx.execute(
                "UPDATE watches SET last_notified=?2 WHERE id=?1",
                params![alert.watch_id, stored(now)],
            )?;
            tx.execute(
                "UPDATE watch_alerts SET notified_at=?2 WHERE id=?1",
                params![id, stored(now)],
            )?;
        }
        tx.commit()?;
        Ok(Some(alert))
    }

    pub fn watch_notification_result(
        &self,
        id: &str,
        result: WatchNotification,
    ) -> Result<(), StoreError> {
        if !matches!(
            result,
            WatchNotification::Sent | WatchNotification::Disabled | WatchNotification::Failed
        ) {
            return Err(StoreError::InvalidQuery);
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let reserved: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM watch_alerts WHERE id=?1 AND notified_at IS NOT NULL)",
            [id],
            |r| r.get(0),
        )?;
        if let Some(mut alert) = self.watch_alert(id)?.filter(|_| reserved) {
            alert.notification = result;
            self.save_watch_alert(&alert)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// History clearing removes measurements, not the user's rules or notification cooldowns.
    pub fn clear_watch_alerts(&self) -> Result<usize, StoreError> {
        Ok(self.conn.execute("DELETE FROM watch_alerts", [])?)
    }

    pub fn maintain_watches(&self, now: u64) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM watch_alerts WHERE started_at<?1",
            [stored(now.saturating_sub(SAMPLE_RETENTION_MS))],
        )?;
        Ok(())
    }

    pub fn watch_weekly_summary(&self, now: u64) -> Result<WatchSummary, StoreError> {
        let from = now.saturating_sub(SAMPLE_RETENTION_MS);
        let mut result = WatchSummary {
            from_unix_ms: from,
            to_unix_ms: now,
            total: 0,
            open: 0,
            returned: 0,
            interrupted: 0,
            by_measure: BTreeMap::new(),
            local_only: true,
        };
        let mut offset = 0;
        loop {
            let page = self.watch_alerts(from, now, 200, offset)?;
            result.total = page.total;
            for alert in page.alerts {
                *result
                    .by_measure
                    .entry(alert.observation.metric)
                    .or_default() += 1;
                if alert.ended_at_unix_ms.is_none() {
                    result.open += 1;
                }
                match alert.close_reason {
                    Some(WatchCloseReason::Returned | WatchCloseReason::NoLongerObservedAbove) => {
                        result.returned += 1
                    }
                    Some(WatchCloseReason::Interrupted) => result.interrupted += 1,
                    _ => {}
                }
            }
            match page.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
        Ok(result)
    }
}

fn notification_key(value: WatchNotification) -> &'static str {
    match value {
        WatchNotification::Pending => "pending",
        WatchNotification::Sent => "sent",
        WatchNotification::Disabled => "disabled",
        WatchNotification::Failed => "failed",
        WatchNotification::RuleCooldown => "rule_cooldown",
        WatchNotification::HourlyLimit => "hourly_limit",
    }
}
