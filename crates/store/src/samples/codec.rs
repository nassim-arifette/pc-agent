//! Private, lossless sample storage. Public samples and history responses keep their old shape.
//! Version 1 separates system values from the other values and interns repeated labels per row.
//! New readers accept legacy rows; old executables cannot read rows written with this codec.

use super::{SampleValue, StoredSample};
use crate::StoreError;
use pcagent_shared::{Metric, Origin, Status};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::{collections::BTreeMap, fmt};

const VERSION: u8 = 1;

/// JSON numbers extracted by SQLite can become floating point. The weighted numerator must
/// instead stay exact through every read, including values larger than u64::MAX.
#[derive(Clone, Copy)]
struct ExactSum(u128);

impl Serialize for ExactSum {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for ExactSum {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = ExactSum;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an unsigned u128 decimal string")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(E::custom("invalid weighted numerator"));
                }
                value
                    .parse()
                    .map(ExactSum)
                    .map_err(|_| E::custom("weighted numerator exceeds u128"))
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

/// Entity, metric, value, min, max, measured, expected, status, source, observed start/end,
/// measured duration, exact weighted numerator, averaging weight. Indexes refer only to this row.
#[derive(Serialize, Deserialize)]
struct Value(
    usize,
    usize,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    u64,
    u64,
    usize,
    usize,
    Option<u64>,
    Option<u64>,
    u64,
    ExactSum,
    u64,
);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<P> {
    v: u8,
    id: String,
    session: String,
    at: u64,
    interval: u64,
    resolution: u64,
    origin: Origin,
    refs: Vec<String>,
    sources: Vec<String>,
    metrics: Vec<Metric>,
    statuses: Vec<Status>,
    system: Vec<Value>,
    /// Original positions of system values: restore arbitrary interleaving on a full read.
    positions: Vec<usize>,
    processes: P,
    names: Vec<(usize, String)>,
}

/// Read and validate non-system tuples without allocating their vector or repeated labels.
/// Keeping their count and dictionary bounds still rejects malformed data on a system-only read.
#[derive(Default)]
struct SkippedProcesses {
    count: usize,
    maxima: [usize; 4],
}

impl<'de> Deserialize<'de> for SkippedProcesses {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = SkippedProcesses;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an array of non-system sample tuples")
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut skipped = SkippedProcesses::default();
                while let Some(value) = seq.next_element::<Value>()? {
                    if value.0 == 0 {
                        return Err(de::Error::custom("system value in process section"));
                    }
                    skipped.count = skipped
                        .count
                        .checked_add(1)
                        .ok_or_else(|| de::Error::custom("too many sample values"))?;
                    for (maximum, index) in skipped
                        .maxima
                        .iter_mut()
                        .zip([value.0, value.1, value.7, value.8])
                    {
                        *maximum = (*maximum).max(index);
                    }
                }
                Ok(skipped)
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}

fn intern<T: PartialEq + Clone>(values: &mut Vec<T>, value: &T) -> usize {
    if let Some(index) = values.iter().position(|item| item == value) {
        index
    } else {
        values.push(value.clone());
        values.len() - 1
    }
}

pub(super) fn encode(sample: &StoredSample) -> Result<String, StoreError> {
    let mut row = Envelope {
        v: VERSION,
        id: sample.id.clone(),
        session: sample.session_id.clone(),
        at: sample.measured_at_unix_ms,
        interval: sample.interval_ms,
        resolution: sample.resolution_ms,
        origin: sample.origin,
        // Index zero always means system, even for a row containing only process values.
        refs: vec!["system".into()],
        sources: Vec::new(),
        metrics: Vec::new(),
        statuses: Vec::new(),
        system: Vec::new(),
        positions: Vec::new(),
        processes: Vec::new(),
        names: Vec::new(),
    };
    for (position, value) in sample.values.iter().enumerate() {
        let packed = Value(
            intern(&mut row.refs, &value.entity_ref),
            intern(&mut row.metrics, &value.metric),
            value.value,
            value.min,
            value.max,
            value.measured,
            value.expected,
            intern(&mut row.statuses, &value.status),
            intern(&mut row.sources, &value.source),
            value.observed_from_unix_ms,
            value.observed_to_unix_ms,
            value.observed_duration_ms,
            ExactSum(value.weighted_sum),
            value.weight_ms,
        );
        if value.entity_ref == "system" {
            row.system.push(packed);
            row.positions.push(position);
        } else {
            row.processes.push(packed);
        }
    }
    for (reference, name) in &sample.process_names {
        row.names
            .push((intern(&mut row.refs, reference), name.clone()));
    }
    serde_json::to_string(&row).map_err(|_| StoreError::Corrupt)
}

fn is_compact(text: &str) -> Result<bool, StoreError> {
    // The private writer always emits v first. Avoid a second full JSON scan for these rows;
    // the actual envelope decoder still checks its version, syntax and all closed fields.
    if text.trim_start().starts_with("{\"v\":") {
        return Ok(true);
    }
    #[derive(Deserialize)]
    struct Version {
        v: Option<u8>,
    }
    let version: Version = serde_json::from_str(text).map_err(|_| StoreError::Corrupt)?;
    if version.v.is_some_and(|value| value != VERSION) {
        return Err(StoreError::Corrupt);
    }
    Ok(version.v.is_some())
}

fn unique<T: PartialEq>(values: &[T]) -> bool {
    values
        .iter()
        .enumerate()
        .all(|(index, value)| !values[..index].contains(value))
}

impl<P> Envelope<P> {
    fn validate(&self, process_count: usize) -> Result<(), StoreError> {
        let count = self
            .system
            .len()
            .checked_add(process_count)
            .ok_or(StoreError::Corrupt)?;
        if self.v != VERSION
            || self.refs.first().map(String::as_str) != Some("system")
            || !unique(&self.refs)
            || !unique(&self.sources)
            || !unique(&self.metrics)
            || !unique(&self.statuses)
            || self.system.len() != self.positions.len()
            || self.positions.windows(2).any(|pair| pair[0] >= pair[1])
            || self
                .positions
                .last()
                .is_some_and(|position| *position >= count)
            || self.system.iter().any(|value| value.0 != 0)
        {
            return Err(StoreError::Corrupt);
        }
        Ok(())
    }

    fn unpack(&self, value: &Value) -> Result<SampleValue, StoreError> {
        Ok(SampleValue {
            entity_ref: self.refs.get(value.0).ok_or(StoreError::Corrupt)?.clone(),
            metric: *self.metrics.get(value.1).ok_or(StoreError::Corrupt)?,
            value: value.2,
            min: value.3,
            max: value.4,
            measured: value.5,
            expected: value.6,
            status: *self.statuses.get(value.7).ok_or(StoreError::Corrupt)?,
            source: self
                .sources
                .get(value.8)
                .ok_or(StoreError::Corrupt)?
                .clone(),
            observed_from_unix_ms: value.9,
            observed_to_unix_ms: value.10,
            observed_duration_ms: value.11,
            weighted_sum: value.12 .0,
            weight_ms: value.13,
        })
    }

    fn into_sample(self, values: Vec<SampleValue>) -> Result<StoredSample, StoreError> {
        let mut process_names = BTreeMap::new();
        for (reference, name) in self.names {
            let reference = self.refs.get(reference).ok_or(StoreError::Corrupt)?;
            if process_names.insert(reference.clone(), name).is_some() {
                return Err(StoreError::Corrupt);
            }
        }
        Ok(StoredSample {
            id: self.id,
            session_id: self.session,
            measured_at_unix_ms: self.at,
            interval_ms: self.interval,
            resolution_ms: self.resolution,
            origin: self.origin,
            values,
            process_names,
        })
    }
}

pub(super) fn decode(text: &str) -> Result<StoredSample, StoreError> {
    if !is_compact(text)? {
        return serde_json::from_str(text).map_err(|_| StoreError::Corrupt);
    }
    let row: Envelope<Vec<Value>> = serde_json::from_str(text).map_err(|_| StoreError::Corrupt)?;
    row.validate(row.processes.len())?;
    if row.processes.iter().any(|value| value.0 == 0) {
        return Err(StoreError::Corrupt);
    }
    let mut system = row.system.iter();
    let mut processes = row.processes.iter();
    let mut positions = row.positions.iter().copied().peekable();
    let count = row.system.len() + row.processes.len();
    let mut values = Vec::with_capacity(count);
    for position in 0..count {
        let value = if positions.peek() == Some(&position) {
            positions.next();
            system.next()
        } else {
            processes.next()
        }
        .ok_or(StoreError::Corrupt)?;
        values.push(row.unpack(value)?);
    }
    row.into_sample(values)
}

pub(super) fn decode_system(text: &str) -> Result<StoredSample, StoreError> {
    if !is_compact(text)? {
        let mut sample: StoredSample =
            serde_json::from_str(text).map_err(|_| StoreError::Corrupt)?;
        sample.values.retain(|value| value.entity_ref == "system");
        return Ok(sample);
    }
    let row: Envelope<SkippedProcesses> =
        serde_json::from_str(text).map_err(|_| StoreError::Corrupt)?;
    row.validate(row.processes.count)?;
    if row.processes.count != 0
        && row
            .processes
            .maxima
            .iter()
            .zip([
                row.refs.len(),
                row.metrics.len(),
                row.statuses.len(),
                row.sources.len(),
            ])
            .any(|(maximum, length)| *maximum >= length)
    {
        return Err(StoreError::Corrupt);
    }
    let values = row
        .system
        .iter()
        .map(|value| row.unpack(value))
        .collect::<Result<_, _>>()?;
    row.into_sample(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(entity: &str, metric: Metric, status: Status) -> SampleValue {
        let measured = status == Status::Ok;
        SampleValue {
            metric,
            entity_ref: entity.into(),
            value: measured.then_some(u64::MAX - 1),
            min: measured.then_some(0),
            max: measured.then_some(u64::MAX),
            measured: u64::from(measured),
            expected: 12,
            status,
            source: "fixture source \\ exact \"quoted\" label".into(),
            observed_from_unix_ms: measured.then_some(u64::MAX - 60_000),
            observed_to_unix_ms: measured.then_some(u64::MAX - 1),
            observed_duration_ms: if measured { 59_999 } else { 0 },
            weighted_sum: if measured { u128::MAX - 1 } else { 0 },
            weight_ms: if measured { u64::MAX } else { 0 },
        }
    }

    fn sample(origin: Origin) -> StoredSample {
        StoredSample {
            id: "sample-id".into(),
            session_id: "collector-session".into(),
            measured_at_unix_ms: u64::MAX,
            interval_ms: 5_000,
            resolution_ms: 60_000,
            origin,
            values: vec![
                value("proc-one", Metric::ProcessCpuBasisPoints, Status::Ok),
                value("system", Metric::CpuBusyBasisPoints, Status::Ok),
                value(
                    "proc-two",
                    Metric::ProcessWorkingSetBytes,
                    Status::Unsupported,
                ),
                value(
                    "system",
                    Metric::PhysicalAvailableBytes,
                    Status::Unsupported,
                ),
                value("other-entity", Metric::GpuTemperatureCelsius, Status::Stale),
            ],
            process_names: BTreeMap::from([
                ("proc-one".into(), "application.exe".into()),
                ("proc-two".into(), "échantillon.exe".into()),
                (
                    "proc-exited".into(),
                    "name without a retained value.exe".into(),
                ),
            ]),
        }
    }

    fn same(expected: &StoredSample, actual: &StoredSample) {
        // Compare the public typed serialization directly: Value would narrow the u128 field.
        assert_eq!(
            serde_json::to_string(expected).unwrap(),
            serde_json::to_string(actual).unwrap()
        );
    }

    #[test]
    fn legacy_and_compact_round_trip_all_fields_and_original_value_order() {
        for origin in [Origin::Live, Origin::SyntheticFixture] {
            let sample = sample(origin);
            let legacy = serde_json::to_string(&sample).unwrap();
            same(&sample, &decode(&legacy).unwrap());
            let compact = encode(&sample).unwrap();
            same(&sample, &decode(&compact).unwrap());
            assert!(compact.contains(&format!("\"{}\"", u128::MAX - 1)));
            assert!(serde_json::from_str::<StoredSample>(&compact).is_err());
        }
    }

    #[test]
    fn system_projection_preserves_metadata_names_unknowns_and_exact_numbers() {
        let sample = sample(Origin::Live);
        let mut expected = sample.clone();
        expected.values.retain(|value| value.entity_ref == "system");
        for text in [
            serde_json::to_string(&sample).unwrap(),
            encode(&sample).unwrap(),
        ] {
            same(&expected, &decode_system(&text).unwrap());
        }
    }

    #[test]
    fn legacy_rows_without_later_window_and_weight_fields_keep_their_defaults() {
        let text = r#"{"id":"old","session_id":"session","measured_at_unix_ms":1000,
            "interval_ms":5000,"resolution_ms":5000,"origin":"live","process_names":{},
            "values":[{"metric":"cpu_busy_basis_points","entity_ref":"system",
            "value":100,"min":100,"max":100,"measured":1,"expected":1,
            "status":"ok","source":"old sensor"}]}"#;
        let sample = decode(text).unwrap();
        assert_eq!(sample.values[0].observed_from_unix_ms, None);
        assert_eq!(sample.values[0].observed_to_unix_ms, None);
        assert_eq!(sample.values[0].observed_duration_ms, 0);
        assert_eq!(sample.values[0].weighted_sum, 0);
        assert_eq!(sample.values[0].weight_ms, 0);
        same(&sample, &decode_system(text).unwrap());
        same(&sample, &decode(&encode(&sample).unwrap()).unwrap());
    }

    #[test]
    fn empty_process_only_and_system_only_rows_are_lossless() {
        for kind in 0..3 {
            let mut sample = sample(Origin::SyntheticFixture);
            sample.values.retain(|value| match kind {
                0 => false,
                1 => value.entity_ref != "system",
                _ => value.entity_ref == "system",
            });
            let encoded = encode(&sample).unwrap();
            same(&sample, &decode(&encoded).unwrap());
            sample.values.retain(|value| value.entity_ref == "system");
            same(&sample, &decode_system(&encoded).unwrap());
        }
    }

    #[test]
    fn unknown_versions_fields_invalid_tuples_and_truncated_data_are_rejected() {
        let encoded = encode(&sample(Origin::Live)).unwrap();
        for invalid in [
            encoded.replacen("\"v\":1", "\"v\":2", 1),
            encoded.replacen("\"v\":1", "\"v\":null", 1),
            encoded.replacen("\"v\":1", "\"v\":1,\"unexpected\":true", 1),
            encoded[..encoded.len() - 1].into(),
            encoded.replacen(
                &format!("\"{}\"", u128::MAX - 1),
                "\"340282366920938463463374607431768211456\"",
                1,
            ),
            encoded.replacen(&format!("\"{}\"", u128::MAX - 1), "\"+1\"", 1),
            encoded.replacen(&format!("\"{}\"", u128::MAX - 1), "1.5", 1),
        ] {
            assert!(matches!(decode(&invalid), Err(StoreError::Corrupt)));
            assert!(matches!(decode_system(&invalid), Err(StoreError::Corrupt)));
        }
        let invalid_tuple = "{\"v\":1,\"system\":[[0,0]],\"processes\":[]}";
        assert!(matches!(decode(invalid_tuple), Err(StoreError::Corrupt)));
        assert!(matches!(
            decode_system(invalid_tuple),
            Err(StoreError::Corrupt)
        ));
    }

    #[test]
    fn bad_dictionary_indices_order_and_partition_fail_in_both_read_modes() {
        for case in 0..10 {
            let encoded = encode(&sample(Origin::Live)).unwrap();
            let mut row: Envelope<Vec<Value>> = serde_json::from_str(&encoded).unwrap();
            match case {
                0 => row.system[0].1 = row.metrics.len(),
                1 => row.processes[0].0 = row.refs.len(),
                2 => row.processes[0].1 = row.metrics.len(),
                3 => row.processes[0].7 = row.statuses.len(),
                4 => row.processes[0].8 = row.sources.len(),
                5 => row.names[0].0 = row.refs.len(),
                6 => row.positions[1] = row.positions[0],
                7 => row.positions[1] = usize::MAX,
                8 => row.processes[0].0 = 0,
                _ => row.system[0].0 = 1,
            }
            let invalid = serde_json::to_string(&row).unwrap();
            assert!(
                matches!(decode(&invalid), Err(StoreError::Corrupt)),
                "case {case}"
            );
            assert!(
                matches!(decode_system(&invalid), Err(StoreError::Corrupt)),
                "case {case}"
            );
        }
    }

    #[test]
    fn repeated_process_samples_are_substantially_smaller_without_losing_fields() {
        let mut sample = sample(Origin::Live);
        sample.values.clear();
        sample.process_names.clear();
        for process in 0..20 {
            let entity = format!("proc-{process:08}");
            sample
                .process_names
                .insert(entity.clone(), format!("application-{process}.exe"));
            for metric in [
                Metric::ProcessWorkingSetBytes,
                Metric::ProcessPrivateCommitBytes,
                Metric::ProcessCpuBasisPoints,
                Metric::ProcessIoReadBytesPerSecond,
                Metric::ProcessIoWriteBytesPerSecond,
                Metric::ProcessGpuBasisPoints,
            ] {
                sample.values.push(value(&entity, metric, Status::Ok));
            }
        }
        let compact = encode(&sample).unwrap();
        let legacy = serde_json::to_string(&sample).unwrap();
        assert!(
            compact.len() * 2 < legacy.len(),
            "compact={} legacy={}",
            compact.len(),
            legacy.len()
        );
        same(&sample, &decode(&compact).unwrap());
    }
}
