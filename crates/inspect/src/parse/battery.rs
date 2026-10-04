//! Fixed-size numeric battery records. Names, serials and unique IDs are never decoded.

pub(crate) const INFORMATION_SIZE: usize = 36;
pub(crate) const STATUS_SIZE: usize = 16;
const SYSTEM_BATTERY: u32 = 0x8000_0000;
const CAPACITY_RELATIVE: u32 = 0x4000_0000;
const SHORT_TERM: u32 = 0x2000_0000;
const ON_LINE: u32 = 1;
const DISCHARGING: u32 = 2;
const CHARGING: u32 = 4;
const CRITICAL: u32 = 8;
const STATE_BITS: u32 = ON_LINE | DISCHARGING | CHARGING | CRITICAL;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Information {
    pub capabilities: u32,
    pub technology: u8,
    pub design_capacity: u32,
    pub full_charge_capacity: u32,
    pub critical_bias: u32,
    pub cycle_count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub power_state: u32,
    pub capacity: u32,
    pub voltage: u32,
    pub rate: i32,
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

pub(crate) fn information(bytes: &[u8]) -> Option<Information> {
    if bytes.len() != INFORMATION_SIZE {
        return None;
    }
    Some(Information {
        capabilities: u32_at(bytes, 0)?,
        technology: bytes[4],
        design_capacity: u32_at(bytes, 12)?,
        full_charge_capacity: u32_at(bytes, 16)?,
        critical_bias: u32_at(bytes, 28)?,
        cycle_count: u32_at(bytes, 32)?,
    })
}

pub(crate) fn status(bytes: &[u8]) -> Option<Status> {
    if bytes.len() != STATUS_SIZE {
        return None;
    }
    Some(Status {
        power_state: u32_at(bytes, 0)?,
        capacity: u32_at(bytes, 4)?,
        voltage: u32_at(bytes, 8)?,
        rate: i32::from_le_bytes(bytes[12..16].try_into().ok()?),
    })
}

pub(crate) fn scalar(bytes: &[u8]) -> Option<u32> {
    if bytes.len() != 4 {
        return None;
    }
    u32_at(bytes, 0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    System,
    ShortTerm,
    NonSystem,
}
impl Kind {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::ShortTerm => "short_term",
            Self::NonSystem => "non_system",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Units {
    MilliwattHours,
    Relative,
}
impl Units {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::MilliwattHours => "milliwatt_hours",
            Self::Relative => "relative",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChargeState {
    Charging,
    Discharging,
    Idle,
    Unknown,
}
impl ChargeState {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::Charging => "charging",
            Self::Discharging => "discharging",
            Self::Idle => "idle",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Facts {
    pub kind: Option<Kind>,
    pub units: Option<Units>,
    pub rechargeable: Option<bool>,
    pub design_capacity_mwh: Option<u32>,
    pub full_charge_capacity_mwh: Option<u32>,
    pub critical_bias_mwh: Option<u32>,
    pub cycle_count: Option<u32>,
    pub capacities_consistent: Option<bool>,
    pub capacity_retention_percent: Option<f64>,
    pub estimated_wear_percent: Option<f64>,
    pub remaining_capacity_mwh: Option<u32>,
    pub charge_percent: Option<f64>,
    pub power_rate_mw: Option<i32>,
    pub discharge_power_mw: Option<u32>,
    pub charge_power_mw: Option<u32>,
    pub voltage_mv: Option<u32>,
    pub charge_state: Option<ChargeState>,
    pub on_ac: Option<bool>,
    pub critical: Option<bool>,
    pub state_consistent: Option<bool>,
    pub runtime_windows_seconds: Option<u32>,
    pub runtime_at_current_rate_seconds: Option<f64>,
}

fn known(value: u32) -> Option<u32> {
    (value != u32::MAX).then_some(value)
}
fn positive(value: u32) -> Option<u32> {
    known(value).filter(|value| *value != 0)
}

/// Keep batteries separate: there is deliberately no implicit total or average.
pub(crate) fn normalize(
    info: Option<Information>,
    status: Option<Status>,
    estimated_time: Option<u32>,
) -> Facts {
    let mut facts = Facts::default();
    let design = info.and_then(|info| positive(info.design_capacity));
    let full = info.and_then(|info| positive(info.full_charge_capacity));
    let remaining = status.and_then(|status| known(status.capacity));
    if let Some(info) = info {
        facts.kind = Some(if info.capabilities & SHORT_TERM != 0 {
            Kind::ShortTerm
        } else if info.capabilities & SYSTEM_BATTERY != 0 {
            Kind::System
        } else {
            Kind::NonSystem
        });
        facts.units = Some(if info.capabilities & CAPACITY_RELATIVE != 0 {
            Units::Relative
        } else {
            Units::MilliwattHours
        });
        facts.rechargeable = match info.technology {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        facts.cycle_count = positive(info.cycle_count);
        if facts.units == Some(Units::MilliwattHours) {
            facts.design_capacity_mwh = design;
            facts.full_charge_capacity_mwh = full;
            facts.critical_bias_mwh = known(info.critical_bias);
            facts.remaining_capacity_mwh = remaining;
        }
    }
    let design_full_invalid = matches!((design, full), (Some(design), Some(full)) if full > design);
    let remaining_full_invalid =
        matches!((remaining, full), (Some(remaining), Some(full)) if remaining > full);
    facts.capacities_consistent = if design_full_invalid || remaining_full_invalid {
        Some(false)
    } else if status.is_some() {
        match (remaining, full) {
            (Some(_), Some(_)) => Some(true),
            _ => None,
        }
    } else {
        match (design, full) {
            (Some(_), Some(_)) => Some(true),
            _ => None,
        }
    };
    if facts.capacities_consistent == Some(true) {
        if let (Some(remaining), Some(full)) = (remaining, full) {
            facts.charge_percent = Some(f64::from(remaining) / f64::from(full) * 100.0);
        }
    }
    // Health uses valid static capacities independently of an unknown current capacity.
    // Known contradictory capacities still block these estimates.
    if facts.units == Some(Units::MilliwattHours)
        && facts.rechargeable == Some(true)
        && !design_full_invalid
        && !remaining_full_invalid
    {
        if let (Some(design), Some(full)) = (design, full) {
            let retention = f64::from(full) / f64::from(design) * 100.0;
            facts.capacity_retention_percent = Some(retention);
            // A full capacity greater than design stays unknown, never clamped to zero wear.
            facts.estimated_wear_percent = Some(100.0 - retention);
        }
    }
    let Some(status) = status else {
        return facts;
    };
    facts.voltage_mv = positive(status.voltage);
    facts.on_ac = Some(status.power_state & ON_LINE != 0);
    facts.critical = Some(status.power_state & CRITICAL != 0);
    let charging = status.power_state & CHARGING != 0;
    let discharging = status.power_state & DISCHARGING != 0;
    let rate = (status.rate != i32::MIN).then_some(status.rate);
    let flags_valid = status.power_state & !STATE_BITS == 0
        && !(charging && discharging)
        && !(discharging && status.power_state & ON_LINE != 0);
    let sign_valid = match rate {
        Some(rate) if rate < 0 => discharging && !charging,
        Some(rate) if rate > 0 => charging && !discharging,
        _ => true,
    };
    let consistent = flags_valid && sign_valid;
    facts.state_consistent = Some(consistent);
    facts.charge_state = Some(if !consistent {
        ChargeState::Unknown
    } else if charging {
        ChargeState::Charging
    } else if discharging {
        ChargeState::Discharging
    } else {
        ChargeState::Idle
    });
    if facts.units == Some(Units::MilliwattHours) {
        // The signed native reading remains visible even when flags contradict it.
        facts.power_rate_mw = rate;
        if consistent {
            facts.discharge_power_mw = rate
                .filter(|rate| discharging && *rate < 0)
                .map(i32::unsigned_abs);
            facts.charge_power_mw = rate
                .filter(|rate| charging && *rate > 0)
                .map(|rate| rate as u32);
        }
    }
    if consistent && discharging {
        facts.runtime_windows_seconds = estimated_time.and_then(known);
        if facts.capacities_consistent == Some(true) {
            if let (Some(remaining), Some(discharge)) =
                (facts.remaining_capacity_mwh, facts.discharge_power_mw)
            {
                // CriticalBias has already been applied by the driver. This assumes constant drain.
                facts.runtime_at_current_rate_seconds =
                    Some(f64::from(remaining) / f64::from(discharge) * 3600.0);
            }
        }
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> Information {
        Information {
            capabilities: SYSTEM_BATTERY,
            technology: 1,
            design_capacity: 60_000,
            full_charge_capacity: 48_000,
            critical_bias: 500,
            cycle_count: 180,
        }
    }
    fn draining() -> Status {
        Status {
            power_state: DISCHARGING,
            capacity: 24_000,
            voltage: 11_700,
            rate: -12_000,
        }
    }
    #[test]
    fn binary_records_are_exactly_bounded_and_ignore_text_fields() {
        let mut bytes = [0_u8; INFORMATION_SIZE];
        bytes[0..4].copy_from_slice(&SYSTEM_BATTERY.to_le_bytes());
        bytes[4] = 1;
        bytes[8..12].copy_from_slice(b"TEST");
        for (offset, value) in [(12, 60_000_u32), (16, 48_000), (28, 500), (32, 180)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        assert_eq!(information(&bytes), Some(info()));
        assert!(information(&bytes[..35]).is_none());
        assert!(information(&[0_u8; INFORMATION_SIZE + 1]).is_none());
        let mut bytes = [0_u8; STATUS_SIZE];
        for (offset, value) in [(0, DISCHARGING), (4, 24_000), (8, 11_700)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[12..16].copy_from_slice(&(-12_000_i32).to_le_bytes());
        assert_eq!(status(&bytes), Some(draining()));
        assert!(status(&bytes[..15]).is_none());
        assert!(status(&[0_u8; STATUS_SIZE + 1]).is_none());
        assert_eq!(scalar(&42_u32.to_le_bytes()), Some(42));
        assert!(scalar(&[0; 5]).is_none());
    }
    #[test]
    fn capacities_signed_power_and_estimates_keep_distinct_meanings() {
        let facts = normalize(Some(info()), Some(draining()), Some(6_800));
        assert_eq!(facts.kind, Some(Kind::System));
        assert_eq!(facts.charge_percent, Some(50.0));
        assert_eq!(facts.capacity_retention_percent, Some(80.0));
        assert_eq!(facts.estimated_wear_percent, Some(20.0));
        assert_eq!(facts.power_rate_mw, Some(-12_000));
        assert_eq!(facts.discharge_power_mw, Some(12_000));
        assert_eq!(facts.charge_power_mw, None);
        assert_eq!(facts.runtime_windows_seconds, Some(6_800));
        assert_eq!(facts.runtime_at_current_rate_seconds, Some(7_200.0));
        assert_eq!(facts.cycle_count, Some(180));
    }
    #[test]
    fn charging_idle_and_zero_capacity_are_real_states() {
        let mut status = draining();
        status.power_state = ON_LINE | CHARGING;
        status.rate = 5_000;
        let facts = normalize(Some(info()), Some(status), Some(9_000));
        assert_eq!(facts.charge_state, Some(ChargeState::Charging));
        assert_eq!(facts.charge_power_mw, Some(5_000));
        assert_eq!(facts.runtime_windows_seconds, None);
        assert_eq!(facts.runtime_at_current_rate_seconds, None);
        status.power_state = 0;
        status.rate = 0;
        status.capacity = 0;
        let facts = normalize(Some(info()), Some(status), None);
        assert_eq!(facts.charge_state, Some(ChargeState::Idle));
        assert_eq!(facts.remaining_capacity_mwh, Some(0));
        assert_eq!(facts.charge_percent, Some(0.0));
        assert_eq!(facts.power_rate_mw, Some(0));
        assert_eq!(facts.discharge_power_mw, None);
    }
    #[test]
    fn missing_sources_and_unknown_sentinels_do_not_create_zero_readings() {
        assert_eq!(normalize(None, None, None), Facts::default());
        let mut info = info();
        info.design_capacity = 0;
        info.full_charge_capacity = u32::MAX;
        info.cycle_count = 0;
        let status = Status {
            power_state: DISCHARGING,
            capacity: u32::MAX,
            voltage: u32::MAX,
            rate: i32::MIN,
        };
        let facts = normalize(Some(info), Some(status), Some(u32::MAX));
        assert_eq!(facts.design_capacity_mwh, None);
        assert_eq!(facts.full_charge_capacity_mwh, None);
        assert_eq!(facts.remaining_capacity_mwh, None);
        assert_eq!(facts.cycle_count, None);
        assert_eq!(facts.voltage_mv, None);
        assert_eq!(facts.power_rate_mw, None);
        assert_eq!(facts.capacities_consistent, None);
        assert_eq!(facts.estimated_wear_percent, None);
        assert_eq!(facts.runtime_windows_seconds, None);
        assert_eq!(facts.runtime_at_current_rate_seconds, None);
        let facts = normalize(None, Some(draining()), Some(100));
        assert_eq!(facts.remaining_capacity_mwh, None);
        assert_eq!(facts.power_rate_mw, None);
    }
    #[test]
    fn unknown_remaining_capacity_does_not_hide_valid_static_health() {
        let status = Status {
            capacity: u32::MAX,
            ..draining()
        };
        let facts = normalize(Some(info()), Some(status), Some(6_800));
        assert_eq!(facts.design_capacity_mwh, Some(60_000));
        assert_eq!(facts.full_charge_capacity_mwh, Some(48_000));
        assert_eq!(facts.capacities_consistent, None);
        assert_eq!(facts.capacity_retention_percent, Some(80.0));
        assert_eq!(facts.estimated_wear_percent, Some(20.0));
        assert_eq!(facts.remaining_capacity_mwh, None);
        assert_eq!(facts.charge_percent, None);
        assert_eq!(facts.runtime_at_current_rate_seconds, None);
        assert_eq!(facts.runtime_windows_seconds, Some(6_800));
    }
    #[test]
    fn known_remaining_capacity_contradiction_still_blocks_health() {
        let status = Status {
            capacity: 49_000,
            ..draining()
        };
        let facts = normalize(Some(info()), Some(status), None);
        assert_eq!(facts.remaining_capacity_mwh, Some(49_000));
        assert_eq!(facts.capacities_consistent, Some(false));
        assert_eq!(facts.capacity_retention_percent, None);
        assert_eq!(facts.estimated_wear_percent, None);
        assert_eq!(facts.charge_percent, None);
        assert_eq!(facts.runtime_at_current_rate_seconds, None);
    }
    #[test]
    fn inconsistent_capacities_never_become_negative_or_clamped_wear() {
        let mut info = info();
        info.full_charge_capacity = 61_000;
        let facts = normalize(Some(info), Some(draining()), None);
        assert_eq!(facts.full_charge_capacity_mwh, Some(61_000));
        assert_eq!(facts.capacities_consistent, Some(false));
        assert_eq!(facts.estimated_wear_percent, None);
        assert_eq!(facts.capacity_retention_percent, None);
        assert_eq!(facts.charge_percent, None);
        assert_eq!(facts.runtime_at_current_rate_seconds, None);
        let mut status = draining();
        status.capacity = 70_000;
        let facts = normalize(Some(info), Some(status), None);
        assert_eq!(facts.remaining_capacity_mwh, Some(70_000));
        assert_eq!(facts.charge_percent, None);
    }
    #[test]
    fn conflicting_flags_signs_and_future_bits_block_derived_power() {
        for (flags, rate) in [
            (CHARGING | DISCHARGING, -10),
            (ON_LINE | DISCHARGING, -10),
            (DISCHARGING, 10),
            (CHARGING, -10),
            (0, -10),
            (0x100, 0),
        ] {
            let status = Status {
                power_state: flags,
                rate,
                ..draining()
            };
            let facts = normalize(Some(info()), Some(status), Some(100));
            assert_eq!(facts.state_consistent, Some(false));
            assert_eq!(facts.charge_state, Some(ChargeState::Unknown));
            assert_eq!(facts.power_rate_mw, Some(rate));
            assert_eq!(facts.discharge_power_mw, None);
            assert_eq!(facts.charge_power_mw, None);
            assert_eq!(facts.runtime_windows_seconds, None);
            assert_eq!(facts.runtime_at_current_rate_seconds, None);
        }
    }
    #[test]
    fn relative_and_short_term_devices_are_never_relabelled_as_system_energy() {
        let info = Information {
            capabilities: SYSTEM_BATTERY | SHORT_TERM | CAPACITY_RELATIVE,
            design_capacity: 100,
            full_charge_capacity: 100,
            ..info()
        };
        let status = Status {
            capacity: 50,
            rate: -25,
            ..draining()
        };
        let facts = normalize(Some(info), Some(status), Some(7_200));
        assert_eq!(facts.kind, Some(Kind::ShortTerm));
        assert_eq!(facts.units, Some(Units::Relative));
        assert_eq!(facts.charge_percent, Some(50.0));
        assert_eq!(facts.design_capacity_mwh, None);
        assert_eq!(facts.full_charge_capacity_mwh, None);
        assert_eq!(facts.remaining_capacity_mwh, None);
        assert_eq!(facts.power_rate_mw, None);
        assert_eq!(facts.estimated_wear_percent, None);
        assert_eq!(facts.runtime_at_current_rate_seconds, None);
        assert_eq!(facts.runtime_windows_seconds, Some(7_200));
    }
    #[test]
    fn batteries_remain_independent_and_unknown_design_does_not_hide_charge() {
        let first = normalize(Some(info()), Some(draining()), None);
        let second_info = Information {
            design_capacity: u32::MAX,
            full_charge_capacity: 10_000,
            ..info()
        };
        let second_status = Status {
            capacity: 2_000,
            rate: -1_000,
            ..draining()
        };
        let second = normalize(Some(second_info), Some(second_status), None);
        assert_eq!(first.remaining_capacity_mwh, Some(24_000));
        assert_eq!(second.remaining_capacity_mwh, Some(2_000));
        assert_eq!(second.charge_percent, Some(20.0));
        assert_eq!(second.capacities_consistent, Some(true));
        assert_eq!(second.estimated_wear_percent, None);
        assert_eq!(second.runtime_at_current_rate_seconds, Some(7_200.0));
    }
}
