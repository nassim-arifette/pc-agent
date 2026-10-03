use super::{error, wide};
use crate::{CoverageStatus as C, Inspection};
use std::time::{Duration, Instant};
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_NO_MORE_ITEMS},
    System::EventLog::*,
};

pub(super) struct EventHandle(EVT_HANDLE);
impl Drop for EventHandle {
    fn drop(&mut self) {
        // SAFETY: owned event handle, closed once.
        unsafe {
            EvtClose(self.0);
        }
    }
}
struct Group {
    channel: &'static str,
    provider: &'static str,
    ids: &'static [u16],
}
struct NativeEvent {
    id: u16,
    timestamp: u64,
    service_name: Option<String>,
}
const GROUPS: &[Group] = &[
    Group {
        channel: "System",
        provider: "Microsoft-Windows-Kernel-Power",
        ids: &[41],
    },
    Group {
        channel: "System",
        provider: "EventLog",
        ids: &[6008],
    },
    Group {
        channel: "System",
        provider: "Microsoft-Windows-WER-SystemErrorReporting",
        ids: &[1001],
    },
    Group {
        channel: "Application",
        provider: "Application Error",
        ids: &[1000],
    },
    Group {
        channel: "Application",
        provider: "Application Hang",
        ids: &[1002],
    },
    Group {
        channel: "System",
        provider: "Microsoft-Windows-WHEA-Logger",
        ids: &[],
    },
    Group {
        channel: "System",
        provider: "disk",
        ids: &[7, 51, 153],
    },
    Group {
        channel: "System",
        provider: "storport",
        ids: &[129],
    },
    Group {
        channel: "System",
        provider: "storahci",
        ids: &[129],
    },
    Group {
        channel: "System",
        provider: "stornvme",
        ids: &[129],
    },
    Group {
        channel: "System",
        provider: "Ntfs",
        ids: &[55],
    },
    Group {
        channel: "System",
        provider: "Microsoft-Windows-Ntfs",
        ids: &[55],
    },
    Group {
        channel: "System",
        provider: "Microsoft-Windows-WindowsUpdateClient",
        ids: &[20, 25, 31],
    },
    Group {
        channel: "System",
        provider: "Service Control Manager",
        ids: &[7000, 7001, 7031, 7034],
    },
    Group {
        channel: "System",
        provider: "Microsoft-Windows-Resource-Exhaustion-Detector",
        ids: &[2004],
    },
];
fn xpath(group: &Group, days: u32) -> String {
    let ids = if group.ids.is_empty() {
        String::new()
    } else {
        format!(
            " and ({})",
            group
                .ids
                .iter()
                .map(|n| format!("EventID={n}"))
                .collect::<Vec<_>>()
                .join(" or ")
        )
    };
    format!(
        "*[System[Provider[@Name='{}']{} and TimeCreated[timediff(@SystemTime) <= {}]]]",
        group.provider,
        ids,
        days as u64 * 86_400_000
    )
}

pub(super) fn collect(out: &mut Inspection, days: u32, limit: usize, channel: Option<&str>) {
    const SOURCE: &str = "Win32.EvtQuery/EvtRender (allowlisted system fields)";
    let start = Instant::now();
    let mut scanned = 0;
    let mut occurrences = 0;
    out.window.duration_ms = days as u64 * 86_400_000;
    out.window.started_at_unix_ms = out
        .measured_at_unix_ms
        .saturating_sub(out.window.duration_ms);
    for group in GROUPS
        .iter()
        .filter(|g| channel.is_none_or(|c| c == g.channel))
    {
        let scope = format!("{}:{}", group.channel, group.provider);
        if start.elapsed() > Duration::from_secs(4) || scanned >= 5000 {
            out.coverage(
                &scope,
                C::Partial,
                SOURCE,
                "Global event time/count bound reached; this source was not scanned.",
            );
            continue;
        }
        match read(group, days, (5000 - scanned).min(1000), start, &mut scanned) {
            Ok((events, partial)) => {
                let mut grouped = std::collections::BTreeMap::<u16, Vec<u64>>::new();
                for event in events {
                    grouped.entry(event.id).or_default().push(event.timestamp);
                }
                if grouped.is_empty() {
                    let entity = out.entity("events", None);
                    out.value(&entity, "event_source", group.provider, "provider", SOURCE);
                    out.value(&entity, "event_count", 0_u64, "count", SOURCE);
                }
                for (id, times) in grouped {
                    let entity = out.entity("events", None);
                    out.value(&entity, "event_source", group.provider, "provider", SOURCE);
                    out.value(&entity, "event_id", id as u64, "event_id", SOURCE);
                    out.value(&entity, "event_count", times.len() as u64, "count", SOURCE);
                    let remaining = limit.saturating_sub(occurrences);
                    let kept = times
                        .iter()
                        .take(remaining.min(5))
                        .copied()
                        .collect::<Vec<_>>();
                    occurrences += kept.len();
                    out.value(
                        &entity,
                        "latest_occurrences_unix_ms",
                        serde_json::json!(kept),
                        "unix_ms",
                        SOURCE,
                    );
                }
                out.coverage(&scope,if partial{C::Partial}else{C::Ok},SOURCE,if partial{"Counts are lower bounds because the scan was capped or some event fields could not be read."}else{"Counts cover matching events retained by Windows in the requested period; cleared/rotated logs cannot be reconstructed. Latest occurrences are capped."});
            }
            Err(e) => out.coverage(
                &scope,
                error(e),
                SOURCE,
                "Event source could not be read with the current account.",
            ),
        }
    }
}

pub(super) fn service_failures(out: &mut Inspection) {
    const SOURCE: &str = "Win32.EvtQuery/EvtRender (SCM structured service name only)";
    let group = GROUPS
        .iter()
        .find(|g| g.provider == "Service Control Manager")
        .expect("fixed SCM group");
    let previous = out.window.clone();
    out.window.duration_ms = 7 * 86_400_000;
    out.window.started_at_unix_ms = out
        .measured_at_unix_ms
        .saturating_sub(out.window.duration_ms);
    match read(group, 7, 1000, Instant::now(), &mut 0) {
        Ok((events, mut partial)) => {
            let mut grouped = std::collections::BTreeMap::<(String, u16), Vec<u64>>::new();
            for event in events {
                if let Some(name) = event.service_name {
                    grouped
                        .entry((name, event.id))
                        .or_default()
                        .push(event.timestamp);
                } else {
                    partial = true;
                }
            }
            if grouped.is_empty() && !partial {
                out.value(
                    "services",
                    "recent_service_failure_count",
                    0_u64,
                    "count",
                    SOURCE,
                );
            }
            for ((name, id), times) in grouped {
                let known = out
                    .names
                    .iter()
                    .find(|(_, n)| *n == &name)
                    .map(|(id, _)| id.strip_suffix("-display").unwrap_or(id).to_owned());
                let entity = known.unwrap_or_else(|| out.entity("service", Some(name)));
                out.value(
                    &entity,
                    &format!("service_failure_{id}_count"),
                    times.len() as u64,
                    "count",
                    SOURCE,
                );
                out.value(
                    &entity,
                    "latest_service_failure_unix_ms",
                    times
                        .into_iter()
                        .max()
                        .map(serde_json::Value::from)
                        .unwrap_or(serde_json::Value::Null),
                    "unix_ms",
                    SOURCE,
                );
            }
            out.coverage("recent_service_failures",if partial{C::Partial}else{C::Ok},SOURCE,"Failures in retained logs for the last seven days; only the structured SCM service-name field is read, never formatted event messages. Capped scans are lower bounds.");
        }
        Err(code) => out.coverage(
            "recent_service_failures",
            error(code),
            SOURCE,
            "Recent SCM failure records could not be read.",
        ),
    }
    out.window = previous;
}

fn read(
    group: &Group,
    days: u32,
    limit: usize,
    start: Instant,
    scanned: &mut usize,
) -> Result<(Vec<NativeEvent>, bool), u32> {
    // SAFETY: local query, terminated fixed channel/query strings; no remote session.
    let query = unsafe {
        EvtQuery(
            0,
            wide(group.channel).as_ptr(),
            wide(&xpath(group, days)).as_ptr(),
            EvtQueryChannelPath | EvtQueryReverseDirection,
        )
    };
    if query == 0 {
        // SAFETY: last error is read immediately after the failed native call.
        return Err(unsafe { GetLastError() });
    }
    let query = EventHandle(query);
    let mut paths = vec![
        wide("Event/System/EventID"),
        wide("Event/System/TimeCreated/@SystemTime"),
    ];
    let scm = group.provider == "Service Control Manager";
    if scm {
        paths.push(wide("Event/EventData/Data[@Name='param1']"));
    }
    let pointers = paths.iter().map(|p| p.as_ptr()).collect::<Vec<_>>();
    // SAFETY: terminated XPath strings outlive construction; selected SCM data is a service name.
    let context = unsafe {
        EvtCreateRenderContext(
            pointers.len() as u32,
            pointers.as_ptr(),
            EvtRenderContextValues,
        )
    };
    if context == 0 {
        // SAFETY: last error from context creation.
        return Err(unsafe { GetLastError() });
    }
    let context = EventHandle(context);
    let mut result = Vec::new();
    let mut partial = false;
    loop {
        if start.elapsed() > Duration::from_secs(4) || *scanned >= 5000 {
            return Ok((result, true));
        }
        let mut handle = 0;
        let mut returned = 0;
        // SAFETY: one handle slot and count output are valid; bounded native timeout.
        if unsafe { EvtNext(query.0, 1, &mut handle, 250, 0, &mut returned) } == 0 {
            // SAFETY: immediate last-error read.
            let code = unsafe { GetLastError() };
            if code == ERROR_NO_MORE_ITEMS {
                return Ok((result, partial));
            }
            return if result.is_empty() {
                Err(code)
            } else {
                Ok((result, true))
            };
        }
        if returned == 0 {
            return Ok((result, partial));
        }
        *scanned += 1;
        let event = EventHandle(handle);
        if result.len() >= limit {
            return Ok((result, true));
        }
        // u64 gives alignment suitable for EVT_VARIANT and space for the selected scalar values.
        let mut buffer = [0_u64; 1024];
        let mut used = 0;
        let mut properties = 0;
        // SAFETY: aligned buffer of the declared byte size; live context/event; valid outputs.
        let success = unsafe {
            EvtRender(
                context.0,
                event.0,
                EvtRenderEventValues,
                std::mem::size_of_val(&buffer) as u32,
                buffer.as_mut_ptr().cast(),
                &mut used,
                &mut properties,
            )
        };
        if success == 0
            || properties as usize != pointers.len()
            || used as usize > std::mem::size_of_val(&buffer)
            || (used as usize) < pointers.len() * std::mem::size_of::<EVT_VARIANT>()
        {
            partial = true;
            continue;
        }
        // SAFETY: successful EvtRender wrote the checked number of variants into aligned storage.
        let values = unsafe {
            std::slice::from_raw_parts(buffer.as_ptr().cast::<EVT_VARIANT>(), pointers.len())
        };
        // SAFETY: read only union members matching the native discriminants, and reject all others.
        let parsed = unsafe {
            if values[0].Type == EvtVarTypeUInt16 as u32
                && values[1].Type == EvtVarTypeFileTime as u32
            {
                values[1]
                    .Anonymous
                    .FileTimeVal
                    .checked_sub(116_444_736_000_000_000)
                    .map(|t| (values[0].Anonymous.UInt16Val, t / 10_000))
            } else {
                None
            }
        };
        let service_name = if scm && values[2].Type == EvtVarTypeString as u32 {
            // SAFETY: matching string discriminant; bounded_string validates buffer membership.
            bounded_string(
                unsafe { values[2].Anonymous.StringVal },
                &buffer,
                used as usize,
            )
        } else {
            None
        };
        match parsed {
            Some((id, timestamp)) => result.push(NativeEvent {
                id,
                timestamp,
                service_name,
            }),
            None => partial = true,
        }
    }
}

fn bounded_string(pointer: *const u16, buffer: &[u64], used: usize) -> Option<String> {
    let offset = (pointer as usize).checked_sub(buffer.as_ptr() as usize)?;
    if used > std::mem::size_of_val(buffer) || offset >= used || offset % 2 != 0 {
        return None;
    }
    let count = ((used - offset) / 2).min(512);
    // SAFETY: range lies within the successful native output buffer; UTF-16 is decoded lossily.
    let units = unsafe { std::slice::from_raw_parts(pointer, count) };
    let end = units.iter().position(|v| *v == 0)?;
    Some(String::from_utf16_lossy(&units[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_never_accept_user_code() {
        let x = xpath(&GROUPS[0], 30);
        assert!(x.contains("EventID=41"));
        assert!(x.contains("2592000000"));
        assert!(!x.contains("EventData"));
    }

    #[test]
    fn rendered_strings_must_be_aligned_terminated_and_inside_the_written_buffer() {
        let mut buffer = [0_u64; 4];
        buffer[1] = u64::from_le_bytes([b'S', 0, b'v', 0, b'c', 0, 0, 0]);
        let base = buffer.as_ptr().cast::<u8>();
        let name = base.wrapping_add(8).cast::<u16>();
        assert_eq!(bounded_string(name, &buffer, 16).as_deref(), Some("Svc"));
        assert_eq!(bounded_string(name, &buffer, 15), None);
        assert_eq!(
            bounded_string(base.wrapping_add(9).cast(), &buffer, 16),
            None
        );
        assert_eq!(
            bounded_string(base.wrapping_add(32).cast(), &buffer, 32),
            None
        );
        assert_eq!(bounded_string(name, &buffer, 33), None);
        assert_eq!(bounded_string(std::ptr::null(), &buffer, 16), None);
    }
}
