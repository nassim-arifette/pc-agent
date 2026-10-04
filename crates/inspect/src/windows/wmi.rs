use super::{com_error, wide};
use crate::{CoverageStatus as C, Inspection};
use ::windows::{
    core::{BSTR, PCWSTR},
    Win32::System::{Com::*, Variant::*, Wmi::*},
};
use serde_json::Value;
use std::time::{Duration, Instant};

pub(super) struct ComApartment;
impl ComApartment {
    pub(super) fn new() -> ::windows::core::Result<Self> {
        // SAFETY: this inspection owns a dedicated thread and requests a multithreaded apartment.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: matched successful CoInitializeEx on this thread.
        unsafe {
            CoUninitialize();
        }
    }
}

pub(super) struct Column {
    pub property: &'static str,
    pub metric: &'static str,
    pub unit: &'static str,
    pub kind: Kind,
}
pub(super) enum Kind {
    Number(u64),
    Name,
    Text,
    Boolean,
    KelvinTenths,
}
const fn n(
    property: &'static str,
    metric: &'static str,
    unit: &'static str,
    factor: u64,
) -> Column {
    Column {
        property,
        metric,
        unit,
        kind: Kind::Number(factor),
    }
}
const fn text(property: &'static str, metric: &'static str) -> Column {
    Column {
        property,
        metric,
        unit: "text",
        kind: Kind::Text,
    }
}
const fn name(property: &'static str) -> Column {
    Column {
        property,
        metric: "name",
        unit: "name",
        kind: Kind::Name,
    }
}

pub(super) fn named(out: &mut Inspection, query: &str, limit: usize) {
    match query {
        "operating_system" => collect(
            out,
            "root\\cimv2",
            "Win32_OperatingSystem",
            &[
                text("Version", "windows_version"),
                text("BuildNumber", "windows_build"),
                text("OSArchitecture", "architecture"),
            ],
            None,
            limit,
        ),
        "processor" => collect(
            out,
            "root\\cimv2",
            "Win32_Processor",
            &[
                name("Name"),
                n("NumberOfCores", "cpu_core_count", "count", 1),
                n(
                    "NumberOfLogicalProcessors",
                    "cpu_logical_processor_count",
                    "count",
                    1,
                ),
                n("MaxClockSpeed", "cpu_max_clock_megahertz", "megahertz", 1),
                n(
                    "CurrentClockSpeed",
                    "cpu_current_clock_megahertz",
                    "megahertz",
                    1,
                ),
            ],
            None,
            limit,
        ),
        "physical_memory" => collect(
            out,
            "root\\cimv2",
            "Win32_PhysicalMemory",
            &[
                n("Capacity", "memory_capacity_bytes", "bytes", 1),
                n(
                    "Speed",
                    "memory_speed_megatransfers_per_second",
                    "megatransfers_per_second",
                    1,
                ),
                n(
                    "ConfiguredClockSpeed",
                    "memory_configured_speed_megatransfers_per_second",
                    "megatransfers_per_second",
                    1,
                ),
            ],
            None,
            limit,
        ),
        "pagefile" => collect(
            out,
            "root\\cimv2",
            "Win32_PageFileUsage",
            &[
                n(
                    "AllocatedBaseSize",
                    "pagefile_allocated_bytes",
                    "bytes",
                    1024 * 1024,
                ),
                n("CurrentUsage", "pagefile_used_bytes", "bytes", 1024 * 1024),
                n("PeakUsage", "pagefile_peak_bytes", "bytes", 1024 * 1024),
                Column {
                    property: "TempPageFile",
                    metric: "temporary_pagefile",
                    unit: "boolean",
                    kind: Kind::Boolean,
                },
            ],
            None,
            limit,
        ),
        "last_updates" => collect(
            out,
            "root\\cimv2",
            "Win32_QuickFixEngineering",
            &[
                text("HotFixID", "update_id"),
                text("InstalledOn", "installed_on"),
            ],
            None,
            limit,
        ),
        "thermal_zones" => collect(
            out,
            "root\\wmi",
            "MSAcpi_ThermalZoneTemperature",
            &[Column {
                property: "CurrentTemperature",
                metric: "thermal_zone_temperature_celsius",
                unit: "degrees_celsius",
                kind: Kind::KelvinTenths,
            }],
            None,
            limit,
        ),
        "storage_devices" => collect(
            out,
            "root\\Microsoft\\Windows\\Storage",
            "MSFT_PhysicalDisk",
            &[
                name("Model"),
                n("MediaType", "disk_media_type", "windows_media_type", 1),
                n("BusType", "disk_bus_type", "windows_bus_type", 1),
                n("Size", "disk_capacity_bytes", "bytes", 1),
            ],
            None,
            limit,
        ),
        _ => out.coverage(
            "query",
            C::Denied,
            "Fixed local allowlist",
            "Unknown query; no WMI request was made.",
        ),
    }
}

pub(super) fn collect(
    out: &mut Inspection,
    namespace: &str,
    class: &str,
    columns: &[Column],
    filter: Option<&str>,
    limit: usize,
) {
    let source = format!("WMI.{class}");
    let result = query(namespace, class, columns, filter, limit);
    match result {
        Ok((rows, partial)) => add_rows(out, class, columns, rows, partial),
        Err(e) => out.coverage(
            class,
            com_error(&e),
            &source,
            "The WMI provider is unavailable or denied this read-only query.",
        ),
    }
}
fn add_rows(
    out: &mut Inspection,
    class: &str,
    columns: &[Column],
    rows: Vec<Vec<Value>>,
    partial: bool,
) {
    let source = format!("WMI.{class}");
    let mut missing = false;
    for row in rows {
        let entity = out.entity("component", None);
        for (column, value) in columns.iter().zip(row) {
            let value = match column.kind {
                Kind::Name => {
                    if let Some(s) = value.as_str().filter(|s| !s.is_empty()) {
                        out.names.insert(entity.clone(), s.to_owned());
                    }
                    continue;
                }
                Kind::Number(factor) => integer(&value)
                    .and_then(|v| v.checked_mul(factor))
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                Kind::Boolean => value.as_bool().map(Value::from).unwrap_or(Value::Null),
                Kind::KelvinTenths => integer(&value)
                    .filter(|v| (2000..=4500).contains(v))
                    .map(|v| Value::from(v as f64 / 10.0 - 273.15))
                    .unwrap_or(Value::Null),
                Kind::Text => value
                    .as_str()
                    .filter(|s| {
                        s.len() <= 100
                            && s.chars()
                                .all(|c| c.is_ascii_alphanumeric() || ".-_/ :+()".contains(c))
                    })
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            };
            missing |= value.is_null();
            out.value(&entity, column.metric, value, column.unit, &source);
        }
    }
    out.coverage(class,if partial || missing { C::Partial } else { C::Ok },&source,if partial { "Row or time limit reached; returned records are a partial inventory." } else if missing { "Provider omitted or returned an unsupported value for some fields." } else { "Only fixed, allowlisted properties were queried. No identifiers or paths were requested." });
}
fn integer(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

pub(super) fn query(
    namespace: &str,
    class: &str,
    columns: &[Column],
    filter: Option<&str>,
    limit: usize,
) -> ::windows::core::Result<(Vec<Vec<Value>>, bool)> {
    let fields = columns
        .iter()
        .map(|c| c.property)
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        "SELECT {fields} FROM {class}{}",
        filter.map(|f| format!(" WHERE {f}")).unwrap_or_default()
    );
    let empty = BSTR::new();
    // SAFETY: COM is initialized on this thread; all strings are owned and interfaces are retained
    // until enumeration finishes. The WQL consists exclusively of internal constants.
    let enumerator = unsafe {
        let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
        let service = locator.ConnectServer(
            &BSTR::from(namespace),
            &empty,
            &empty,
            &empty,
            WBEM_FLAG_CONNECT_USE_MAX_WAIT.0,
            &empty,
            None,
        )?;
        CoSetProxyBlanket(
            &service,
            10,
            0,
            PCWSTR::null(),
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )?;
        service.ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from(query),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        )?
    };
    let start = Instant::now();
    let mut rows = Vec::new();
    loop {
        if start.elapsed() > Duration::from_secs(3) {
            return Ok((rows, true));
        }
        let mut objects = [None];
        let mut returned = 0;
        // SAFETY: one initialized optional interface slot and valid count output; timeout is bounded.
        let hr = unsafe { enumerator.Next(500, &mut objects, &mut returned) };
        if hr.is_err() && !rows.is_empty() {
            return Ok((rows, true));
        }
        hr.ok()?;
        if returned == 0 {
            if hr.0 == WBEM_S_TIMEDOUT.0 {
                continue;
            }
            return Ok((rows, false));
        }
        if rows.len() == limit {
            return Ok((rows, true));
        }
        let Some(object) = objects[0].take() else {
            return Ok((rows, true));
        };
        let mut row = Vec::new();
        for column in columns {
            let mut variant = VARIANT::default();
            // SAFETY: initialized VARIANT output; property string is terminated; no optional outputs.
            let read = unsafe {
                object.Get(
                    PCWSTR(wide(column.property).as_ptr()),
                    0,
                    &mut variant,
                    None,
                    None,
                )
            };
            row.push(if read.is_ok() {
                variant_value(&variant)
            } else {
                Value::Null
            });
            // SAFETY: returned variant is cleared once after its value was copied into owned JSON.
            unsafe {
                let _ = VariantClear(&mut variant);
            }
        }
        rows.push(row);
    }
}
fn variant_value(variant: &VARIANT) -> Value {
    // SAFETY: examine the discriminant first and read only its matching non-byref scalar member.
    unsafe {
        let v = &variant.Anonymous.Anonymous;
        match v.vt {
            VT_BSTR => Value::from(v.Anonymous.bstrVal.to_string()),
            VT_UI1 => Value::from(v.Anonymous.bVal),
            VT_UI2 => Value::from(v.Anonymous.uiVal),
            VT_UI4 => Value::from(v.Anonymous.ulVal),
            VT_UI8 => Value::from(v.Anonymous.ullVal),
            VT_I2 => Value::from(v.Anonymous.iVal),
            VT_I4 => Value::from(v.Anonymous.lVal),
            VT_I8 => Value::from(v.Anonymous.llVal),
            VT_R4 => serde_json::Number::from_f64(f64::from(v.Anonymous.fltVal))
                .map_or(Value::Null, Value::Number),
            VT_R8 => {
                serde_json::Number::from_f64(v.Anonymous.dblVal).map_or(Value::Null, Value::Number)
            }
            VT_BOOL => Value::from(v.Anonymous.boolVal.0 != 0),
            _ => Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_numbers_are_checked() {
        assert_eq!(integer(&Value::from("18446744073709551616")), None);
        assert_eq!(integer(&Value::from("4096")), Some(4096));
        assert_eq!(integer(&Value::from(-1)), None);
    }
    #[test]
    fn provider_rows_project_only_declared_fields_and_leave_unknown_values_missing() {
        let mut out = Inspection::new();
        let columns = [
            name("Name"),
            n("Capacity", "memory_capacity_bytes", "bytes", 1024),
            Column {
                property: "Temperature",
                metric: "thermal_zone_temperature_celsius",
                unit: "degrees_celsius",
                kind: Kind::KelvinTenths,
            },
            Column {
                property: "TempPageFile",
                metric: "temporary_pagefile",
                unit: "boolean",
                kind: Kind::Boolean,
            },
        ];
        add_rows(
            &mut out,
            "fixture",
            &columns,
            vec![vec![
                Value::from("Private device name"),
                Value::from("4096"),
                Value::from(3000),
                Value::Null,
            ]],
            false,
        );
        assert_eq!(out.observations[0].value, 4_194_304_u64);
        assert!((out.observations[1].value.as_f64().unwrap() - 26.85).abs() < 0.001);
        assert!(out.observations[2].value.is_null());
        assert_eq!(out.finish().status, pcagent_shared::Status::Partial);
    }
}
