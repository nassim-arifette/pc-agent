//! Bounded E3 historical estimates. Raw CSV identities never leave this parser.
//!
//! The provider's civil timestamps are not UTC instants. Row durations can overlap;
//! neither their sum nor the span between stamps is a wall-clock measurement window.
#![cfg_attr(not(windows), allow(dead_code))]
use super::protocol::{ElevationError, ElevationStatus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

pub const MAX_EXPORT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_EXPORT_ROWS: usize = 100_000;
pub const MAX_ENERGY_APPS: usize = 32;
const MAX_GROUPS: usize = 1024;
const MAX_COLUMNS: usize = 64;
const MAX_CELL_BYTES: usize = 2048;
pub const MAX_EXECUTABLE_NAME_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnergyLimit {
    HistoricalEstimate,
    ProviderCoverageIncomplete,
    CivilTimeZoneUnknown,
    RowDurationsMayOverlap,
    NoCommittedOnBatteryRows,
    ExportUnavailable,
    ExportFailed,
    ExportEncodingUnsupported,
    ExportSchemaUnsupported,
    ExportMalformed,
    DuplicateRow,
    NumericOverflow,
    ExportSizeLimit,
    RowLimit,
    AppLimit,
    DurationLimit,
    Cancelled,
    PlatformUnsupported,
    NativeAccuracyUnvalidated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CivilStamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub nanosecond: u32,
}
impl CivilStamp {
    pub fn validate(self) -> Result<(), ElevationError> {
        let leap = self.year.is_multiple_of(4)
            && (!self.year.is_multiple_of(100) || self.year.is_multiple_of(400));
        let days = match self.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if leap {
                    29
                } else {
                    28
                }
            }
            _ => 0,
        };
        if !(1..=9999).contains(&self.year)
            || self.day == 0
            || self.day > days
            || self.hour > 23
            || self.minute > 59
            || self.second > 59
            || self.nanosecond >= 1_000_000_000
        {
            return Err(ElevationError::InvalidResponse);
        }
        Ok(())
    }
    pub fn display(self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:09} (provider civil time; zone unknown)",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.nanosecond
        )
    }
    fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() < 19
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || !matches!(bytes[10], b' ' | b'T' | b':')
            || bytes[13] != b':'
            || bytes[16] != b':'
        {
            return None;
        }
        let number = |start, end| -> Option<u16> {
            let digits = bytes.get(start..end)?;
            if !digits.iter().all(u8::is_ascii_digit) {
                return None;
            }
            std::str::from_utf8(digits).ok()?.parse().ok()
        };
        let nanosecond = match bytes.get(19..) {
            Some([]) => 0,
            Some(fraction)
                if (2..=10).contains(&fraction.len())
                    && fraction[0] == b'.'
                    && fraction[1..].iter().all(u8::is_ascii_digit) =>
            {
                let value: u32 = std::str::from_utf8(&fraction[1..]).ok()?.parse().ok()?;
                value.checked_mul(10_u32.pow(10 - fraction.len() as u32))?
            }
            _ => return None,
        };
        let stamp = Self {
            year: number(0, 4)?,
            month: number(5, 7)? as u8,
            day: number(8, 10)? as u8,
            hour: number(11, 13)? as u8,
            minute: number(14, 16)? as u8,
            second: number(17, 19)? as u8,
            nanosecond,
        };
        stamp.validate().ok()?;
        Some(stamp)
    }
}

/// Local helper DTO. The basename belongs to the separate C5 names map.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnergySample {
    pub app_ref: String,
    pub executable_name: Option<String>,
    pub estimated_energy_millijoules: u64,
    pub screen_on_energy_millijoules: u64,
    pub foreground_energy_millijoules: u64,
    pub committed_rows: u32,
    pub reported_row_duration_ms: u64,
    pub first_civil_stamp: CivilStamp,
    pub last_civil_stamp: CivilStamp,
}
impl fmt::Debug for EnergySample {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnergySample")
            .field("app_ref", &self.app_ref)
            .field(
                "estimated_energy_millijoules",
                &self.estimated_energy_millijoules,
            )
            .field("committed_rows", &self.committed_rows)
            .finish_non_exhaustive()
    }
}
impl EnergySample {
    pub fn validate(&self) -> Result<(), ElevationError> {
        if !self.app_ref.strip_prefix("energy-").is_some_and(|suffix| {
            suffix.len() == 4 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) || self
            .executable_name
            .as_deref()
            .is_some_and(|name| !valid_basename(name))
            || self.committed_rows == 0
            || self.committed_rows as usize > MAX_EXPORT_ROWS
            || self.screen_on_energy_millijoules > self.estimated_energy_millijoules
            || self.foreground_energy_millijoules > self.estimated_energy_millijoules
            || self.first_civil_stamp > self.last_civil_stamp
        {
            return Err(ElevationError::InvalidResponse);
        }
        self.first_civil_stamp.validate()?;
        self.last_civil_stamp.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnergySummary {
    pub status: ElevationStatus,
    pub samples: Vec<EnergySample>,
    pub rows_scanned: u32,
    pub committed_on_battery_rows: u32,
    pub distinct_apps: u32,
    /// Sum of accepted rows only; absent when no qualifying measurement was read.
    pub estimated_energy_millijoules: Option<u64>,
    pub first_civil_stamp: Option<CivilStamp>,
    pub last_civil_stamp: Option<CivilStamp>,
    pub truncated: bool,
    pub limitations: Vec<EnergyLimit>,
}
impl EnergySummary {
    pub(crate) fn unavailable(status: ElevationStatus, limit: EnergyLimit) -> Self {
        Self {
            status,
            samples: Vec::new(),
            rows_scanned: 0,
            committed_on_battery_rows: 0,
            distinct_apps: 0,
            estimated_energy_millijoules: None,
            first_civil_stamp: None,
            last_civil_stamp: None,
            truncated: false,
            limitations: vec![limit],
        }
    }
    pub fn validate(&self) -> Result<(), ElevationError> {
        if self.samples.len() > MAX_ENERGY_APPS
            || self.rows_scanned as usize > MAX_EXPORT_ROWS
            || self.committed_on_battery_rows > self.rows_scanned
            || self.distinct_apps > self.committed_on_battery_rows
            || self.distinct_apps as usize > MAX_GROUPS
            || self.samples.len() != (self.distinct_apps as usize).min(MAX_ENERGY_APPS)
            || self.limitations.len() > 16
            || self.status == ElevationStatus::Completed
            || self
                .limitations
                .iter()
                .enumerate()
                .any(|(index, limit)| self.limitations[..index].contains(limit))
            || (self.truncated
                && !self.limitations.contains(&EnergyLimit::RowLimit)
                && !self.limitations.contains(&EnergyLimit::AppLimit))
            || (matches!(
                self.status,
                ElevationStatus::Unsupported | ElevationStatus::Denied | ElevationStatus::Error
            ) && (!self.samples.is_empty() || self.estimated_energy_millijoules.is_some()))
        {
            return Err(ElevationError::InvalidResponse);
        }
        let mut refs = BTreeSet::new();
        let mut sum = 0_u64;
        let mut rows = 0_u32;
        for sample in &self.samples {
            sample.validate()?;
            if !refs.insert(&sample.app_ref) {
                return Err(ElevationError::InvalidResponse);
            }
            sum = sum
                .checked_add(sample.estimated_energy_millijoules)
                .ok_or(ElevationError::InvalidResponse)?;
            rows = rows
                .checked_add(sample.committed_rows)
                .ok_or(ElevationError::InvalidResponse)?;
        }
        match (
            self.estimated_energy_millijoules,
            self.first_civil_stamp,
            self.last_civil_stamp,
        ) {
            (None, None, None)
                if self.committed_on_battery_rows == 0 && self.distinct_apps == 0 => {}
            (Some(total), Some(first), Some(last))
                if self.committed_on_battery_rows > 0
                    && first <= last
                    && sum <= total
                    && rows <= self.committed_on_battery_rows =>
            {
                first.validate()?;
                last.validate()?;
                if [
                    EnergyLimit::HistoricalEstimate,
                    EnergyLimit::ProviderCoverageIncomplete,
                    EnergyLimit::CivilTimeZoneUnknown,
                    EnergyLimit::RowDurationsMayOverlap,
                ]
                .iter()
                .any(|limit| !self.limitations.contains(limit))
                {
                    return Err(ElevationError::InvalidResponse);
                }
                if self.samples.iter().any(|sample| {
                    sample.first_civil_stamp < first || sample.last_civil_stamp > last
                }) || (self.distinct_apps as usize <= MAX_ENERGY_APPS
                    && (sum != total
                        || rows != self.committed_on_battery_rows
                        || self
                            .samples
                            .iter()
                            .map(|sample| sample.first_civil_stamp)
                            .min()
                            != Some(first)
                        || self
                            .samples
                            .iter()
                            .map(|sample| sample.last_civil_stamp)
                            .max()
                            != Some(last)))
                {
                    return Err(ElevationError::InvalidResponse);
                }
            }
            _ => return Err(ElevationError::InvalidResponse),
        }
        Ok(())
    }
}

fn valid_basename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_EXECUTABLE_NAME_BYTES
        && value.to_ascii_lowercase().ends_with(".exe")
        && !value
            .chars()
            .any(|c| c.is_control() || "\\/:<>\"|?*".contains(c))
        && !value.ends_with([' ', '.'])
        && !value.starts_with('.')
}
fn basename(app: &str) -> Option<String> {
    let value = app.rsplit(['\\', '/']).next()?;
    valid_basename(value).then(|| value.to_owned())
}

const REQUIRED: &[&str] = &[
    "AppId",
    "UserId",
    "TimeStamp",
    "OnBattery",
    "ScreenOn",
    "BatterySaverActive",
    "LowPowerEpochActive",
    "Foreground",
    "InteractivityState",
    "Container",
    "Committed",
    "TimeInMSec",
    "MeasuredBitmap",
    "TotalEnergyConsumption",
];
// Exact additional fields in the Microsoft E3 article's enlarged "Benchmark data" screenshot.
// They are never interpreted or summed; TotalEnergyConsumption is the sole energy value.
const OPTIONAL: &[&str] = &[
    "EnergyLoss",
    "CPUEnergyConsumption",
    "SocEnergyConsumption",
    "DisplayEnergyConsumption",
    "DiskEnergyConsumption",
    "NetworkEnergyConsumption",
    "MBBEnergyConsumption",
    "OtherEnergyConsumption",
    "EmiEnergyConsumption",
    "CPUEnergyConsumptionWorkOnBehalf",
    "CPUEnergyConsumptionAttributed",
];

fn decode(bytes: &[u8]) -> Result<String, EnergyLimit> {
    if bytes.len() > MAX_EXPORT_BYTES {
        return Err(EnergyLimit::ExportSizeLimit);
    }
    let text = if let Some(bytes) = bytes.strip_prefix(&[0xff, 0xfe]) {
        if !bytes.len().is_multiple_of(2) {
            return Err(EnergyLimit::ExportEncodingUnsupported);
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16(&units).map_err(|_| EnergyLimit::ExportEncodingUnsupported)?
    } else {
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        std::str::from_utf8(bytes)
            .map_err(|_| EnergyLimit::ExportEncodingUnsupported)?
            .to_owned()
    };
    if text
        .chars()
        .any(|c| c == '\0' || (c.is_control() && !matches!(c, '\r' | '\n' | '\t')))
    {
        return Err(EnergyLimit::ExportMalformed);
    }
    Ok(text)
}

/// CSV reader with a bound before every allocation, no multiline cells and no silent recovery.
struct Csv<'a> {
    remaining: &'a str,
}
impl<'a> Csv<'a> {
    fn row(&mut self) -> Result<Option<Vec<String>>, EnergyLimit> {
        if self.remaining.is_empty() {
            return Ok(None);
        }
        let length = self.remaining.find('\n').unwrap_or(self.remaining.len());
        let line = self.remaining[..length]
            .strip_suffix('\r')
            .unwrap_or(&self.remaining[..length]);
        self.remaining = if length == self.remaining.len() {
            ""
        } else {
            &self.remaining[length + 1..]
        };
        if line.len() > MAX_COLUMNS * (MAX_CELL_BYTES + 3) {
            return Err(EnergyLimit::ExportMalformed);
        }
        let mut cells = Vec::new();
        let bytes = line.as_bytes();
        let mut cursor = 0;
        loop {
            if cells.len() == MAX_COLUMNS {
                return Err(EnergyLimit::ExportSchemaUnsupported);
            }
            let mut cell = Vec::new();
            if bytes.get(cursor) == Some(&b'"') {
                cursor += 1;
                loop {
                    let byte = *bytes.get(cursor).ok_or(EnergyLimit::ExportMalformed)?;
                    cursor += 1;
                    if byte == b'"' {
                        if bytes.get(cursor) == Some(&b'"') {
                            cursor += 1;
                        } else {
                            break;
                        }
                    }
                    if cell.len() == MAX_CELL_BYTES {
                        return Err(EnergyLimit::ExportMalformed);
                    }
                    cell.push(byte);
                }
                if bytes.get(cursor).is_some_and(|byte| *byte != b',') {
                    return Err(EnergyLimit::ExportMalformed);
                }
            } else {
                while let Some(&byte) = bytes.get(cursor).filter(|byte| **byte != b',') {
                    if byte == b'"' || cell.len() == MAX_CELL_BYTES {
                        return Err(EnergyLimit::ExportMalformed);
                    }
                    cell.push(byte);
                    cursor += 1;
                }
            }
            cells.push(String::from_utf8(cell).map_err(|_| EnergyLimit::ExportMalformed)?);
            if cursor == bytes.len() {
                break;
            }
            cursor += 1;
        }
        Ok(Some(cells))
    }
}

fn uint(value: &str) -> Option<u64> {
    (!value.is_empty() && value.len() <= 20 && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}
fn boolean(value: &str) -> Option<bool> {
    match value {
        "1" | "true" | "True" | "TRUE" => Some(true),
        "0" | "false" | "False" | "FALSE" => Some(false),
        _ => None,
    }
}
fn bitmap(value: &str) -> Option<u64> {
    if let Some(binary) = value.strip_prefix('b') {
        if binary.is_empty()
            || binary.len() > 64
            || !binary.bytes().all(|byte| matches!(byte, b'0' | b'1'))
        {
            return None;
        }
        u64::from_str_radix(binary, 2).ok()
    } else {
        uint(value)
    }
}

struct Aggregate {
    name: Option<String>,
    energy: u64,
    screen: u64,
    foreground: u64,
    rows: u32,
    duration: u64,
    first: CivilStamp,
    last: CivilStamp,
}

/// Parse a fixed private powercfg export. No SID, path or raw application identifier is returned.
pub(crate) fn parse_export(bytes: &[u8], owner_sid: &str) -> EnergySummary {
    match parse(bytes, owner_sid) {
        Ok(value) => value,
        Err(limit) => EnergySummary::unavailable(ElevationStatus::Unsupported, limit),
    }
}
fn parse(bytes: &[u8], owner_sid: &str) -> Result<EnergySummary, EnergyLimit> {
    if !owner_sid.starts_with("S-1-")
        || owner_sid.len() > 184
        || !owner_sid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'S' | b'-'))
    {
        return Err(EnergyLimit::ExportUnavailable);
    }
    let text = decode(bytes)?;
    let mut csv = Csv { remaining: &text };
    let header = csv.row()?.ok_or(EnergyLimit::ExportSchemaUnsupported)?;
    let mut columns = BTreeMap::new();
    for (index, name) in header.iter().enumerate() {
        if (!REQUIRED.contains(&name.as_str()) && !OPTIONAL.contains(&name.as_str()))
            || columns.insert(name.as_str(), index).is_some()
        {
            return Err(EnergyLimit::ExportSchemaUnsupported);
        }
    }
    if REQUIRED.iter().any(|name| !columns.contains_key(name)) {
        return Err(EnergyLimit::ExportSchemaUnsupported);
    }
    let mut result = EnergySummary {
        status: ElevationStatus::Partial,
        samples: Vec::new(),
        rows_scanned: 0,
        committed_on_battery_rows: 0,
        distinct_apps: 0,
        estimated_energy_millijoules: None,
        first_civil_stamp: None,
        last_civil_stamp: None,
        truncated: false,
        limitations: vec![
            EnergyLimit::HistoricalEstimate,
            EnergyLimit::ProviderCoverageIncomplete,
            EnergyLimit::CivilTimeZoneUnknown,
            EnergyLimit::RowDurationsMayOverlap,
        ],
    };
    let mut apps: BTreeMap<String, Aggregate> = BTreeMap::new();
    let mut identities: BTreeSet<[u8; 32]> = BTreeSet::new();
    while let Some(row) = csv.row()? {
        if result.rows_scanned as usize == MAX_EXPORT_ROWS {
            result.truncated = true;
            result.limitations.push(EnergyLimit::RowLimit);
            break;
        }
        if row.len() != header.len() {
            return Err(EnergyLimit::ExportMalformed);
        }
        result.rows_scanned += 1;
        let get = |name| -> &str { &row[columns[name]] };
        // Filter before retaining/interpreting any other account's identity or app information.
        if get("UserId") != owner_sid {
            continue;
        }
        let on_battery = boolean(get("OnBattery")).ok_or(EnergyLimit::ExportMalformed)?;
        let committed = boolean(get("Committed")).ok_or(EnergyLimit::ExportMalformed)?;
        if !on_battery || !committed {
            continue;
        }
        let screen = boolean(get("ScreenOn")).ok_or(EnergyLimit::ExportMalformed)?;
        let foreground = boolean(get("Foreground")).ok_or(EnergyLimit::ExportMalformed)?;
        let saver = boolean(get("BatterySaverActive")).ok_or(EnergyLimit::ExportMalformed)?;
        let low_power = boolean(get("LowPowerEpochActive")).ok_or(EnergyLimit::ExportMalformed)?;
        // This named provider state is not a counter and is not projected to the model.
        let interactivity = get("InteractivityState");
        if interactivity.is_empty()
            || interactivity.len() > 64
            || !interactivity
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(EnergyLimit::ExportMalformed);
        }
        let container = boolean(get("Container")).ok_or(EnergyLimit::ExportMalformed)?;
        let measured = bitmap(get("MeasuredBitmap")).ok_or(EnergyLimit::ExportMalformed)?;
        let stamp =
            CivilStamp::parse(get("TimeStamp")).ok_or(EnergyLimit::ExportSchemaUnsupported)?;
        let duration = uint(get("TimeInMSec")).ok_or(EnergyLimit::ExportMalformed)?;
        let energy = uint(get("TotalEnergyConsumption")).ok_or(EnergyLimit::ExportMalformed)?;
        let app = get("AppId");
        if app.is_empty() || app.chars().any(char::is_control) {
            return Err(EnergyLimit::ExportMalformed);
        }
        // All state/time columns define a bucket. Energy is excluded so a conflicting duplicate fails too.
        let mut hash = Sha256::new();
        hash.update(app.as_bytes());
        hash.update([0]);
        hash.update(stamp.year.to_le_bytes());
        hash.update([
            stamp.month,
            stamp.day,
            stamp.hour,
            stamp.minute,
            stamp.second,
        ]);
        hash.update(stamp.nanosecond.to_le_bytes());
        hash.update([
            u8::from(screen),
            u8::from(foreground),
            u8::from(saver),
            u8::from(low_power),
            u8::from(container),
        ]);
        hash.update(interactivity.as_bytes());
        hash.update([0]);
        hash.update(duration.to_le_bytes());
        hash.update(measured.to_le_bytes());
        if !identities.insert(hash.finalize().into()) {
            return Err(EnergyLimit::DuplicateRow);
        }
        if !apps.contains_key(app) && apps.len() == MAX_GROUPS {
            result.truncated = true;
            result.limitations.push(EnergyLimit::AppLimit);
            break;
        }
        let aggregate = apps.entry(app.to_owned()).or_insert_with(|| Aggregate {
            name: basename(app),
            energy: 0,
            screen: 0,
            foreground: 0,
            rows: 0,
            duration: 0,
            first: stamp,
            last: stamp,
        });
        aggregate.energy = aggregate
            .energy
            .checked_add(energy)
            .ok_or(EnergyLimit::NumericOverflow)?;
        aggregate.screen = aggregate
            .screen
            .checked_add(if screen { energy } else { 0 })
            .ok_or(EnergyLimit::NumericOverflow)?;
        aggregate.foreground = aggregate
            .foreground
            .checked_add(if foreground { energy } else { 0 })
            .ok_or(EnergyLimit::NumericOverflow)?;
        aggregate.duration = aggregate
            .duration
            .checked_add(duration)
            .ok_or(EnergyLimit::NumericOverflow)?;
        aggregate.rows += 1;
        aggregate.first = aggregate.first.min(stamp);
        aggregate.last = aggregate.last.max(stamp);
        result.committed_on_battery_rows += 1;
        result.estimated_energy_millijoules = Some(
            result
                .estimated_energy_millijoules
                .unwrap_or(0)
                .checked_add(energy)
                .ok_or(EnergyLimit::NumericOverflow)?,
        );
        result.first_civil_stamp = Some(
            result
                .first_civil_stamp
                .map_or(stamp, |first| first.min(stamp)),
        );
        result.last_civil_stamp = Some(
            result
                .last_civil_stamp
                .map_or(stamp, |last| last.max(stamp)),
        );
    }
    result.distinct_apps = apps.len() as u32;
    let mut groups: Vec<_> = apps.into_values().collect();
    groups.sort_by(|a, b| b.energy.cmp(&a.energy));
    if groups.len() > MAX_ENERGY_APPS {
        result.truncated = true;
        if !result.limitations.contains(&EnergyLimit::AppLimit) {
            result.limitations.push(EnergyLimit::AppLimit);
        }
    }
    for (index, aggregate) in groups.into_iter().take(MAX_ENERGY_APPS).enumerate() {
        result.samples.push(EnergySample {
            app_ref: format!("energy-{:04x}", index + 1),
            executable_name: aggregate.name,
            estimated_energy_millijoules: aggregate.energy,
            screen_on_energy_millijoules: aggregate.screen,
            foreground_energy_millijoules: aggregate.foreground,
            committed_rows: aggregate.rows,
            reported_row_duration_ms: aggregate.duration,
            first_civil_stamp: aggregate.first,
            last_civil_stamp: aggregate.last,
        });
    }
    if result.committed_on_battery_rows == 0 {
        result
            .limitations
            .push(EnergyLimit::NoCommittedOnBatteryRows);
    }
    result
        .validate()
        .map_err(|_| EnergyLimit::ExportMalformed)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    const OWNER: &str = "S-1-5-21-111-222-333-1001";
    fn header() -> String {
        REQUIRED.join(",") + ",CPUEnergyConsumption\r\n"
    }
    fn row(
        app: &str,
        user: &str,
        stamp: &str,
        battery: &str,
        committed: &str,
        energy: &str,
    ) -> String {
        format!("\"{app}\",{user},{stamp},{battery},1,0,0,1,NotUnique,FALSE,{committed},1000,b00000000,{energy},900000\r\n")
    }
    #[test]
    fn current_user_committed_battery_rows_only_and_total_is_not_double_counted() {
        let csv = header()
            + &row(
                r"C:\Users\private\app.exe",
                OWNER,
                "2026-10-02 12:00:00.123",
                "1",
                "1",
                "7",
            )
            + &row(
                r"C:\Users\private\app.exe",
                OWNER,
                "2026-10-02 12:01:00",
                "1",
                "1",
                "5",
            )
            + &row(
                "else.exe", "S-1-5-99", "invalid", "unknown", "unknown", "999",
            )
            + &row("ac.exe", OWNER, "invalid", "0", "1", "999")
            + &row("pending.exe", OWNER, "invalid", "1", "0", "999");
        let value = parse_export(csv.as_bytes(), OWNER);
        value.validate().unwrap();
        assert_eq!(value.status, ElevationStatus::Partial);
        assert_eq!(value.estimated_energy_millijoules, Some(12));
        assert_eq!(value.rows_scanned, 5);
        assert_eq!(value.committed_on_battery_rows, 2);
        assert_eq!(value.samples[0].reported_row_duration_ms, 2000);
        assert_eq!(value.samples[0].executable_name.as_deref(), Some("app.exe"));
        assert_eq!(value.first_civil_stamp.unwrap().nanosecond, 123_000_000);
        let json = serde_json::to_string(&value).unwrap();
        assert!(!json.contains("private") && !json.contains(OWNER) && !json.contains("C:"));
        assert!(!format!("{value:?}").contains("app.exe"));
    }
    #[test]
    fn zero_is_a_measurement_and_no_measurements_remain_unknown() {
        let empty = parse_export(header().as_bytes(), OWNER);
        empty.validate().unwrap();
        assert_eq!(empty.estimated_energy_millijoules, None);
        let zero = parse_export(
            (header() + &row("zero.exe", OWNER, "2026-10-02 12:00:00", "1", "1", "0")).as_bytes(),
            OWNER,
        );
        zero.validate().unwrap();
        assert_eq!(zero.estimated_energy_millijoules, Some(0));
    }
    #[test]
    fn duplicate_conflicting_buckets_and_overflow_discard_numeric_claims() {
        let duplicate = header()
            + &row("a.exe", OWNER, "2026-10-02 12:00:00", "1", "1", "1")
            + &row("a.exe", OWNER, "2026-10-02 12:00:00", "1", "1", "2");
        let value = parse_export(duplicate.as_bytes(), OWNER);
        assert_eq!(value.limitations, [EnergyLimit::DuplicateRow]);
        assert_eq!(value.estimated_energy_millijoules, None);
        let equivalent = header()
            + &row("a.exe", OWNER, "2026-10-02 12:00:00", "1", "1", "1")
            + &row(
                "a.exe",
                OWNER,
                "2026-10-02:12:00:00.0000",
                "TRUE",
                "TRUE",
                "1",
            );
        assert_eq!(
            parse_export(equivalent.as_bytes(), OWNER).limitations,
            [EnergyLimit::DuplicateRow]
        );
        let overflow = header()
            + &row(
                "a.exe",
                OWNER,
                "2026-10-02 12:00:00",
                "1",
                "1",
                &u64::MAX.to_string(),
            )
            + &row("a.exe", OWNER, "2026-10-02 12:01:00", "1", "1", "1");
        assert_eq!(
            parse_export(overflow.as_bytes(), OWNER).limitations,
            [EnergyLimit::NumericOverflow]
        );
    }
    #[test]
    fn schema_encoding_quoting_and_periods_fail_closed() {
        let valid = header() + &row("a,b.exe", OWNER, "2024-02-29 12:00:00", "TRUE", "TRUE", "9");
        let bytes: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain(valid.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(
            parse_export(&bytes, OWNER).estimated_energy_millijoules,
            Some(9)
        );
        for invalid in [
            valid.replace("TotalEnergyConsumption", "TotalEnergyConsumptionWh"),
            valid.replace("CPUEnergyConsumption", "UnknownColumn"),
            valid.replace("2024-02-29", "2023-02-29"),
            valid.replace("12:00:00", "12:00:00Z"),
            valid.replace(",9,900000", ",9.5,900000"),
            valid.replace("\"a,b.exe\"", "\"a,b.exe\"junk"),
        ] {
            let result = parse_export(invalid.as_bytes(), OWNER);
            assert_eq!(result.status, ElevationStatus::Unsupported);
            assert_eq!(result.estimated_energy_millijoules, None);
        }
        assert_eq!(
            parse_export(&[0xff, 0xfe, 0x41], OWNER).limitations,
            [EnergyLimit::ExportEncodingUnsupported]
        );
    }
    #[test]
    fn app_bound_retains_largest_estimates_and_preserves_total_accepted_rows() {
        let mut csv = header();
        for index in 0..40 {
            csv += &row(
                &format!("app{index}.exe"),
                OWNER,
                "2026-10-02 12:00:00",
                "1",
                "1",
                &index.to_string(),
            );
        }
        let value = parse_export(csv.as_bytes(), OWNER);
        value.validate().unwrap();
        assert!(value.truncated);
        assert_eq!(value.samples.len(), MAX_ENERGY_APPS);
        assert_eq!(value.distinct_apps, 40);
        assert_eq!(value.estimated_energy_millijoules, Some(780));
        assert_eq!(value.samples[0].estimated_energy_millijoules, 39);
        let mut forged = value.clone();
        forged.estimated_energy_millijoules = Some(1);
        assert!(forged.validate().is_err());
        let mut forged = value;
        forged.first_civil_stamp = None;
        assert!(forged.validate().is_err());
    }
    #[test]
    fn wire_contract_rejects_additional_fields_and_debug_has_no_names() {
        let value = parse_export(
            (header() + &row("secret.exe", OWNER, "2026-10-02 12:00:00", "1", "1", "3")).as_bytes(),
            OWNER,
        );
        let mut json = serde_json::to_value(value).unwrap();
        json["sid"] = OWNER.into();
        assert!(serde_json::from_value::<EnergySummary>(json).is_err());
    }
    #[test]
    fn microsoft_documented_schema_and_state_formats_are_recognized_without_summing_components() {
        // Synthetic values and identity, exact field spellings/state formats from the Microsoft screenshot.
        let mut fields = REQUIRED[..13].to_vec();
        fields.extend_from_slice(OPTIONAL);
        fields.push("TotalEnergyConsumption");
        let mut values = vec![
            r"\Device\HarddiskVolume3\Users\fixture\app.exe",
            OWNER,
            "2020-09-08:20:13:00.0000",
            "TRUE",
            "TRUE",
            "FALSE",
            "FALSE",
            "TRUE",
            "NotUnique",
            "FALSE",
            "TRUE",
            "60001",
            "b00000000",
        ];
        values.extend(std::iter::repeat_n("999999", OPTIONAL.len()));
        values.push("410");
        let csv = fields.join(",") + "\r\n" + &values.join(",") + "\r\n";
        let value = parse_export(csv.as_bytes(), OWNER);
        value.validate().unwrap();
        assert_eq!(value.estimated_energy_millijoules, Some(410));
        assert_eq!(value.samples[0].reported_row_duration_ms, 60001);
        assert_eq!(value.first_civil_stamp.unwrap().year, 2020);
    }
}
