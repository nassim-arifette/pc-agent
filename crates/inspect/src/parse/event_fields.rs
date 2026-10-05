//! Allowlisted EventData fields. Paths, report IDs, update GUIDs and message text are excluded.
use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Details {
    Crash {
        application: Option<String>,
        application_version: Option<String>,
        module: Option<String>,
        module_version: Option<String>,
        exception: Option<String>,
    },
    Hang {
        application: Option<String>,
        application_version: Option<String>,
    },
    Bugcheck {
        code: Option<String>,
        parameters: Option<Vec<String>>,
    },
    Update {
        error: Option<String>,
        kbs: Vec<String>,
    },
}
pub(crate) fn field_names(provider: &str) -> &'static [&'static str] {
    match provider {
        "Application Error" => &[
            "AppName",
            "AppVersion",
            "ModuleName",
            "ModuleVersion",
            "ExceptionCode",
        ],
        "Application Hang" => &["AppName", "AppVersion"],
        "Microsoft-Windows-WER-SystemErrorReporting" => &["param1"],
        "Microsoft-Windows-WindowsUpdateClient" => &["errorCode", "updateTitle"],
        "Service Control Manager" => &["param1"],
        _ => &[],
    }
}
fn filename(value: Option<&String>) -> Option<String> {
    let name = value?.rsplit(['\\', '/']).next()?.trim();
    if name.is_empty()
        || name.len() > 128
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_ ()".contains(c))
    {
        return None;
    }
    // Names only: do not pass an unrecognized text field or command through a name projection.
    let extension = name.rsplit_once('.')?.1;
    if !["exe", "dll", "sys", "ocx"]
        .iter()
        .any(|ext| extension.eq_ignore_ascii_case(ext))
    {
        return None;
    }
    Some(name.to_owned())
}
fn version(value: Option<&String>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty()
        || value.len() > 32
        || !value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some(value.to_owned())
}
fn hex(value: &str) -> Option<u64> {
    let value = value
        .trim()
        .strip_prefix("0x")
        .or_else(|| value.trim().strip_prefix("0X"))
        .unwrap_or(value.trim());
    if value.is_empty() || value.len() > 16 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(value, 16).ok()
}
fn error(value: Option<&String>) -> Option<String> {
    let value = hex(value?)?;
    (value <= u32::MAX as u64).then(|| format!("0x{value:08X}"))
}
fn bugcheck(value: Option<&String>) -> (Option<String>, Option<Vec<String>>) {
    let Some(value) = value.filter(|v| v.len() <= 180) else {
        return (None, None);
    };
    let Some((code, rest)) = value.split_once('(') else {
        return (None, None);
    };
    let Some(code) = hex(code).filter(|code| *code <= u32::MAX as u64) else {
        return (None, None);
    };
    let Some(rest) = rest.trim().strip_suffix(')') else {
        return (None, None);
    };
    let parts = rest.split(',').collect::<Vec<_>>();
    if parts.len() != 4 {
        return (None, None);
    }
    let Some(parts) = parts.into_iter().map(hex).collect::<Option<Vec<_>>>() else {
        return (None, None);
    };
    (
        Some(format!("0x{code:08X}")),
        Some(parts.into_iter().map(|n| format!("0x{n:016X}")).collect()),
    )
}
pub(crate) fn kb_numbers(title: Option<&String>) -> Vec<String> {
    let Some(title) = title.filter(|v| v.len() <= 512) else {
        return Vec::new();
    };
    let bytes = title.as_bytes();
    let mut result = Vec::new();
    for at in 0..bytes.len().saturating_sub(2) {
        if (bytes[at] != b'K' && bytes[at] != b'k')
            || (bytes[at + 1] != b'B' && bytes[at + 1] != b'b')
            || (at > 0 && bytes[at - 1].is_ascii_alphanumeric())
        {
            continue;
        }
        let start = at + 2;
        let end = start
            + bytes[start..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
        if !(6..=8).contains(&(end - start))
            || bytes.get(end).is_some_and(|b| b.is_ascii_alphanumeric())
        {
            continue;
        }
        let kb = format!("KB{}", &title[start..end]);
        if !result.contains(&kb) {
            result.push(kb);
        }
        if result.len() == 16 {
            break;
        }
    }
    result
}
pub(crate) fn parse(provider: &str, id: u16, fields: &BTreeMap<String, String>) -> Option<Details> {
    Some(match (provider, id) {
        ("Application Error", 1000) => Details::Crash {
            application: filename(fields.get("AppName")),
            application_version: version(fields.get("AppVersion")),
            module: filename(fields.get("ModuleName")),
            module_version: version(fields.get("ModuleVersion")),
            exception: error(fields.get("ExceptionCode")),
        },
        ("Application Hang", 1002) => Details::Hang {
            application: filename(fields.get("AppName")),
            application_version: version(fields.get("AppVersion")),
        },
        ("Microsoft-Windows-WER-SystemErrorReporting", 1001) => {
            let (code, parameters) = bugcheck(fields.get("param1"));
            Details::Bugcheck { code, parameters }
        }
        ("Microsoft-Windows-WindowsUpdateClient", 20 | 25 | 31) => Details::Update {
            error: error(fields.get("errorCode")),
            kbs: kb_numbers(fields.get("updateTitle")),
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fields(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }
    #[test]
    fn crash_details_keep_only_filenames_and_selected_fields() {
        let source = fields(&[
            ("AppName", r"C:\Users\PRIVATE\game.exe"),
            ("AppVersion", "1.2.3.4"),
            ("ModuleName", r"C:\PRIVATE\nvwgf2umx.dll"),
            ("ModuleVersion", "32.0.15.6094"),
            ("ExceptionCode", "c0000005"),
            ("ReportId", "PRIVATE-REPORT"),
            ("AppPath", r"C:\Users\PRIVATE\game.exe"),
        ]);
        let parsed = parse("Application Error", 1000, &source).unwrap();
        assert_eq!(
            parsed,
            Details::Crash {
                application: Some("game.exe".into()),
                application_version: Some("1.2.3.4".into()),
                module: Some("nvwgf2umx.dll".into()),
                module_version: Some("32.0.15.6094".into()),
                exception: Some("0xC0000005".into())
            }
        );
        assert!(!format!("{parsed:?}").contains("PRIVATE"));
        assert!(!field_names("Application Error").contains(&"AppPath"));
    }
    #[test]
    fn bugcheck_reads_numeric_param1_and_never_dump_or_report_fields() {
        let source = fields(&[
            (
                "param1",
                "0x0000007e (0xffffffffc0000005, 0xfffff803c8888666, 0x0, 0x0)",
            ),
            ("param2", r"C:\Users\PRIVATE\crash.dmp"),
            ("param3", "PRIVATE-REPORT"),
        ]);
        let parsed = parse("Microsoft-Windows-WER-SystemErrorReporting", 1001, &source).unwrap();
        assert!(
            matches!(&parsed, Details::Bugcheck { code: Some(code), parameters: Some(values) } if code == "0x0000007E" && values.len() == 4)
        );
        assert!(!format!("{parsed:?}").contains("PRIVATE"));
        assert_eq!(
            field_names("Microsoft-Windows-WER-SystemErrorReporting"),
            &["param1"]
        );
        let invalid = fields(&[("param1", "0x7e (0x0, 0x0, C:\\PRIVATE, 0x0)")]);
        assert_eq!(
            parse("Microsoft-Windows-WER-SystemErrorReporting", 1001, &invalid),
            Some(Details::Bugcheck {
                code: None,
                parameters: None
            })
        );
    }
    #[test]
    fn update_codes_and_kbs_are_selected_without_sharing_titles_or_guids() {
        let source = fields(&[
            ("errorCode", "0x800f0922"),
            (
                "updateTitle",
                "PRIVATE title (KB5034441) and kb5034441, KB1234567",
            ),
            ("updateGuid", "PRIVATE-GUID"),
        ]);
        assert_eq!(
            parse("Microsoft-Windows-WindowsUpdateClient", 20, &source),
            Some(Details::Update {
                error: Some("0x800F0922".into()),
                kbs: vec!["KB5034441".into(), "KB1234567".into()]
            })
        );
        assert_eq!(
            parse(
                "Microsoft-Windows-WindowsUpdateClient",
                25,
                &fields(&[("errorCode", "0x800f0922")])
            ),
            Some(Details::Update {
                error: Some("0x800F0922".into()),
                kbs: Vec::new()
            })
        );
        assert!(kb_numbers(Some(&"notKB1234567x KB12345 KB123456789".into())).is_empty());
    }
    #[test]
    fn absent_or_hostile_fields_remain_unknown() {
        let parsed = parse(
            "Application Hang",
            1002,
            &fields(&[
                ("AppName", "run command.exe --flag"),
                ("AppVersion", "<script>"),
            ]),
        );
        assert_eq!(
            parsed,
            Some(Details::Hang {
                application: None,
                application_version: None
            })
        );
        assert_eq!(parse("unknown-provider", 1000, &BTreeMap::new()), None);
    }
}
