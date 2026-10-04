//! Safeguard fields have finite interpretations; absence cannot prove upgrade eligibility.
pub(crate) fn hold_status(status: Option<u32>) -> Option<bool> {
    match status {
        Some(0) => Some(true),
        Some(2) => Some(false),
        _ => None,
    }
}
pub(crate) fn target_name(name: &str) -> bool {
    name.len() <= 32
        && name.len() >= 4
        && name.bytes().all(|b| b.is_ascii_alphanumeric())
        && name.as_bytes()[name.len() - 4..]
            .iter()
            .enumerate()
            .all(|(index, byte)| match index {
                0 | 1 => byte.is_ascii_digit(),
                2 => *byte == b'H',
                3 => matches!(*byte, b'1' | b'2'),
                _ => false,
            })
}
pub(crate) fn block_ids(value: Option<&str>) -> Vec<String> {
    let Some(value) = value.filter(|v| v.len() <= 512) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for id in value.split([',', ';', ' ']).filter(|s| !s.is_empty()) {
        if id.len() != 8 || !id.bytes().all(|b| b.is_ascii_digit()) || id == "00000000" {
            return Vec::new();
        }
        if !ids.iter().any(|known| known == id) {
            ids.push(id.to_owned());
        }
        if ids.len() > 16 {
            return Vec::new();
        }
    }
    ids
}
pub(crate) fn reason(value: Option<&str>) -> Option<&'static str> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "other" => Some("other"),
        "driver" => Some("driver"),
        "application" | "app" => Some("application"),
        "hardware" => Some("hardware"),
        "firmware" => Some("firmware"),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_or_absent_status_is_not_no_hold() {
        assert_eq!(hold_status(Some(0)), Some(true));
        assert_eq!(hold_status(Some(2)), Some(false));
        assert_eq!(hold_status(None), None);
        assert_eq!(hold_status(Some(1)), None);
    }
    #[test]
    fn target_names_are_discovered_without_assuming_a_future_release_prefix() {
        assert!(target_name("GE24H2"));
        assert!(target_name("GE25H2"));
        assert!(target_name("NEW26H1"));
        assert!(!target_name(r"GE25H2\PRIVATE"));
        assert!(!target_name("PRIVATE"));
    }
    #[test]
    fn only_nonpersonal_block_ids_and_finite_reasons_are_exportable() {
        assert_eq!(
            block_ids(Some("52796844; 12345678")),
            vec!["52796844", "12345678"]
        );
        assert!(block_ids(Some("52796844; PRIVATE")).is_empty());
        assert!(block_ids(None).is_empty());
        assert!(block_ids(Some("00000000")).is_empty());
        assert_eq!(reason(Some("Other")), Some("other"));
        assert_eq!(reason(Some(r"C:\Users\PRIVATE\driver.dll")), None);
    }
}
