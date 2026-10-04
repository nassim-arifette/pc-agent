//! Numeric driver versions and dates; unknown formats are never ordered lexicographically.
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DriverVersion(pub [u16; 4]);
impl DriverVersion {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let parts = text.trim().split('.').collect::<Vec<_>>();
        if parts.len() != 4 {
            return None;
        }
        let mut value = [0; 4];
        for (index, part) in parts.into_iter().enumerate() {
            if part.is_empty() || part.len() > 5 || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            value[index] = part.parse().ok()?;
        }
        Some(Self(value))
    }
    pub(crate) fn text(self) -> String {
        format!("{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
    /// NVIDIA Windows numbering embeds its public version in the last five decimal digits.
    /// This conversion is used only after the provider is identified as NVIDIA, then checked
    /// against NVML. It cannot establish that a driver is the newest available release.
    pub(crate) fn nvidia_public(self) -> Option<String> {
        let [_, _, family, suffix] = self.0;
        if !(10..100).contains(&family) || suffix >= 10_000 {
            return None;
        }
        Some(format!(
            "{}.{:02}",
            (family % 10) as u32 * 100 + suffix as u32 / 100,
            suffix % 100
        ))
    }
}

/// WUA exposes no numeric version property. Accept only a complete numeric suffix in a title.
pub(crate) fn title_version(title: &str) -> Option<DriverVersion> {
    if title.len() > 512 || title.chars().any(char::is_control) {
        return None;
    }
    let suffix = title.rsplit_once(" - ")?.1;
    DriverVersion::parse(suffix)
}
pub(crate) fn relation(
    installed: Option<DriverVersion>,
    offered: Option<DriverVersion>,
) -> &'static str {
    match (installed, offered) {
        (Some(installed), Some(offered)) => match offered.cmp(&installed) {
            Ordering::Less => "older",
            Ordering::Equal => "same",
            Ordering::Greater => "newer",
        },
        _ => "unknown",
    }
}
pub(crate) fn ole_date_ms(days: f64) -> Option<u64> {
    let millis = (days - 25_569.0) * 86_400_000.0;
    (millis.is_finite() && millis >= 0.0 && millis <= u64::MAX as f64)
        .then_some(millis.round() as u64)
}
pub(crate) fn filetime_ms(filetime: u64) -> Option<u64> {
    filetime
        .checked_sub(116_444_736_000_000_000)
        .map(|v| v / 10_000)
}
pub(crate) fn driver_class(value: &str) -> Option<&'static str> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "display" => "display",
        "net" | "network" => "network",
        "hdc" | "scsiadapter" | "diskdrive" | "storage" => "storage",
        "system" => "system",
        "media" | "audio" => "audio",
        "bluetooth" => "bluetooth",
        _ => return None,
    })
}
pub(crate) fn display_text(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 256
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | '/' | '@'))
    {
        return None;
    }
    Some(value.to_owned())
}
/// Matching without instance/hardware IDs is deliberately conservative. A package or device
/// with different/missing model text, or multiple exact matches, remains unassociated.
pub(crate) fn unique_match(
    offered: (&str, &str, &str),
    installed: &[(&str, &str, &str)],
) -> Option<usize> {
    if offered.1.is_empty() || offered.2.is_empty() {
        return None;
    }
    let mut matches = installed.iter().enumerate().filter(|(_, key)| {
        key.0.eq_ignore_ascii_case(offered.0)
            && key.1.eq_ignore_ascii_case(offered.1)
            && key.2.eq_ignore_ascii_case(offered.2)
    });
    let index = matches.next()?.0;
    matches.next().is_none().then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_title_versions_are_exact_suffixes_and_missing_is_unknown() {
        let expected = DriverVersion::parse("32.0.15.6094");
        assert_eq!(title_version("NVIDIA - Display - 32.0.15.6094"), expected);
        assert_eq!(
            title_version("NVIDIA - Display - GPU model 32.0.15.6094"),
            None
        );
        assert_eq!(title_version("NVIDIA - Display - 560.94"), None);
        assert_eq!(
            title_version("NVIDIA - Display - 32.0.15.6094 (optional)"),
            None
        );
        assert_eq!(DriverVersion::parse("32.0.15.99999"), None);
        assert_eq!(DriverVersion::parse("32.0..15"), None);
    }
    #[test]
    fn offered_driver_can_be_older_and_numeric_components_are_ordered() {
        let installed = DriverVersion::parse("32.0.15.6094");
        assert_eq!(
            relation(installed, DriverVersion::parse("32.0.15.5764")),
            "older"
        );
        assert_eq!(relation(installed, installed), "same");
        assert_eq!(
            relation(installed, DriverVersion::parse("32.0.15.7628")),
            "newer"
        );
        assert_eq!(relation(None, installed), "unknown");
        assert_eq!(
            relation(
                DriverVersion::parse("1.2.3.9"),
                DriverVersion::parse("1.2.3.10")
            ),
            "newer"
        );
    }
    #[test]
    fn nvidia_conversion_and_native_dates_are_bounded() {
        assert_eq!(
            DriverVersion::parse("32.0.15.6094")
                .unwrap()
                .nvidia_public()
                .as_deref(),
            Some("560.94")
        );
        assert_eq!(
            DriverVersion::parse("23.21.13.9135")
                .unwrap()
                .nvidia_public()
                .as_deref(),
            Some("391.35")
        );
        assert_eq!(
            DriverVersion::parse("1.0.101.5000")
                .unwrap()
                .nvidia_public(),
            None
        );
        assert_eq!(ole_date_ms(25569.5), Some(43_200_000));
        assert_eq!(ole_date_ms(f64::NAN), None);
        assert_eq!(ole_date_ms(1.0), None);
        assert_eq!(filetime_ms(0), None);
        assert_eq!(filetime_ms(116_444_736_000_000_000), Some(0));
    }
    #[test]
    fn package_matching_rejects_ambiguity_and_identifier_like_fields() {
        let devices = [
            ("display", "NVIDIA", "GPU A"),
            ("network", "Intel", "Network Adapter"),
        ];
        assert_eq!(
            unique_match(("display", "nvidia", "GPU A"), &devices),
            Some(0)
        );
        assert_eq!(unique_match(("display", "NVIDIA", "GPU B"), &devices), None);
        assert_eq!(
            unique_match(("display", "NVIDIA", "GPU A"), &[devices[0], devices[0]]),
            None
        );
        assert_eq!(unique_match(("display", "", ""), &devices), None);
        assert_eq!(driver_class("Net"), Some("network"));
        assert_eq!(driver_class("Printer"), None);
        assert_eq!(display_text(r"C:\Users\PRIVATE\device"), None);
    }
}
