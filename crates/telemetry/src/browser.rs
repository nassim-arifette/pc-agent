//! Only closed, non-personal classifications cross the native command-line boundary.
#![cfg_attr(not(windows), allow(dead_code))]
use pcagent_shared::Status;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserProcessType {
    BrowserMain,
    Renderer,
    Extension,
    Gpu,
    Network,
    Storage,
    Audio,
    MediaDecoding,
    Utility,
    CrashHandler,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFamily {
    Firefox,
    Chrome,
    Edge,
    /// Chromium-based applications, including Electron; not a verified product identity.
    ChromiumEmbedded,
    WebView2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserClassification {
    pub family: Option<BrowserFamily>,
    pub process_type: BrowserProcessType,
    pub status: Status,
}

impl Default for BrowserClassification {
    fn default() -> Self {
        Self {
            family: None,
            process_type: BrowserProcessType::Unknown,
            status: Status::Partial,
        }
    }
}

pub(crate) fn family(image: &str) -> Option<BrowserFamily> {
    use BrowserFamily::*;
    match image.to_ascii_lowercase().as_str() {
        "firefox.exe" => Some(Firefox),
        "chrome.exe" => Some(Chrome),
        "msedge.exe" => Some(Edge),
        "msedgewebview2.exe" => Some(WebView2),
        "electron.exe" | "code.exe" | "discord.exe" | "slack.exe" | "teams.exe" | "brave.exe"
        | "opera.exe" | "vivaldi.exe" | "chromium.exe" => Some(ChromiumEmbedded),
        _ => None,
    }
}

pub(crate) fn unavailable(image: &str, status: Status) -> BrowserClassification {
    BrowserClassification {
        family: family(image),
        status,
        ..BrowserClassification::default()
    }
}

/// Consumes already-tokenized arguments; neither arguments nor executable paths are returned.
/// Callers must discard their native buffer and tokens immediately after this function returns.
pub(crate) fn classify(image: &str, args: &[String]) -> BrowserClassification {
    use BrowserProcessType::*;
    let mut browser = family(image);
    let flag = |key: &str| args.iter().skip(1).any(|a| a == key);
    let value = |key: &str| {
        let prefix = format!("{key}=");
        args.iter().enumerate().skip(1).find_map(|(i, a)| {
            a.strip_prefix(&prefix).or_else(|| {
                (a == key)
                    .then(|| args.get(i + 1).map(String::as_str))
                    .flatten()
            })
        })
    };
    let kind = if browser == Some(BrowserFamily::Firefox) {
        if flag("-contentproc") {
            // Firefox's contentproc ABI ends with the process type (not the earlier IDs,
            // channel names, preference handles or profile arguments).
            match args.last().map(String::as_str) {
                Some("tab") => Renderer,
                Some("gpu") => Gpu,
                Some("socket") => Network,
                Some("rdd") | Some("gmplugin") => MediaDecoding,
                Some("utility") => Utility,
                _ => Unknown,
            }
        } else {
            BrowserMain
        }
    } else if let Some(kind) = value("--type").filter(|_| {
        browser.is_some()
            || value("--mojo-platform-channel-handle").is_some()
            || value("--field-trial-handle").is_some()
    }) {
        // Unknown image names need a Chromium IPC signature, not just a generic --type flag.
        // This is a runtime heuristic, never a verified product identity or action authority.
        browser = browser.or(Some(BrowserFamily::ChromiumEmbedded));
        match kind {
            "renderer" if flag("--extension-process") => Extension,
            "renderer" => Renderer,
            "gpu-process" => Gpu,
            "crashpad-handler" => CrashHandler,
            "utility" => match value("--utility-sub-type") {
                Some("network.mojom.NetworkService") => Network,
                Some("storage.mojom.StorageService") => Storage,
                Some("audio.mojom.AudioService") => Audio,
                Some("media.mojom.MediaService")
                | Some("media.mojom.VideoDecoderFactory")
                | Some("media.mojom.MediaFoundationService") => MediaDecoding,
                _ => Utility,
            },
            _ => Unknown,
        }
    } else if image.eq_ignore_ascii_case("crashpad_handler.exe") {
        browser = Some(BrowserFamily::ChromiumEmbedded);
        CrashHandler
    } else if browser.is_some() {
        BrowserMain
    } else {
        Unknown
    };
    BrowserClassification {
        family: browser,
        process_type: kind,
        status: if kind == Unknown && browser.is_some() {
            Status::Partial
        } else {
            Status::Ok
        },
    }
}

pub(crate) fn attribute_embedded_main(rows: &mut [crate::activity::ProcessReading]) {
    let mains: Vec<_> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, parent)| {
            (parent.browser.family.is_none()
                && parent.browser.status == Status::Ok
                && parent.created != 0
                && rows.iter().any(|child| {
                    child.parent_pid == parent.pid
                        && child.pid != parent.pid
                        && child.created >= parent.created
                        && child.session == parent.session
                        && child.browser.family == Some(BrowserFamily::ChromiumEmbedded)
                        && child.browser.status == Status::Ok
                        && child
                            .name
                            .as_deref()
                            .zip(parent.name.as_deref())
                            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
                }))
            .then_some(index)
        })
        .collect();
    for index in mains {
        rows[index].browser = BrowserClassification {
            family: Some(BrowserFamily::ChromiumEmbedded),
            process_type: BrowserProcessType::BrowserMain,
            status: Status::Ok,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classified(image: &str, flags: &[&str]) -> BrowserClassification {
        let args = std::iter::once(image)
            .chain(flags.iter().copied())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        classify(image, &args)
    }

    #[test]
    fn firefox_uses_the_final_contentproc_type_not_arbitrary_arguments() {
        use BrowserProcessType::*;
        for (arg, expected) in [
            ("tab", Renderer),
            ("gpu", Gpu),
            ("socket", Network),
            ("rdd", MediaDecoding),
            ("gmplugin", MediaDecoding),
            ("utility", Utility),
            ("new-kind", Unknown),
        ] {
            assert_eq!(
                classified("firefox.exe", &["-contentproc", "gpu", "123", arg]).process_type,
                expected
            );
        }
        assert_eq!(
            classified("firefox.exe", &["https://example.test/gpu"]).process_type,
            BrowserMain
        );
    }

    #[test]
    fn chromium_electron_and_webview_share_the_closed_subtype_map() {
        use BrowserProcessType::*;
        for image in ["chrome.exe", "msedge.exe", "Code.exe", "msedgewebview2.exe"] {
            for (args, expected) in [
                (vec!["--type=renderer"], Renderer),
                (vec!["--type", "renderer", "--extension-process"], Extension),
                (vec!["--type=gpu-process"], Gpu),
                (vec!["--type=crashpad-handler"], CrashHandler),
                (
                    vec![
                        "--type=utility",
                        "--utility-sub-type=network.mojom.NetworkService",
                    ],
                    Network,
                ),
                (
                    vec![
                        "--type=utility",
                        "--utility-sub-type=storage.mojom.StorageService",
                    ],
                    Storage,
                ),
                (
                    vec![
                        "--type=utility",
                        "--utility-sub-type=audio.mojom.AudioService",
                    ],
                    Audio,
                ),
                (
                    vec![
                        "--type=utility",
                        "--utility-sub-type=media.mojom.MediaService",
                    ],
                    MediaDecoding,
                ),
                (vec!["--type=utility", "--utility-sub-type=other"], Utility),
                (vec!["--type=future"], Unknown),
            ] {
                assert_eq!(classified(image, &args).process_type, expected);
            }
        }
        assert_eq!(
            classified("msedgewebview2.exe", &[]).process_type,
            BrowserMain
        );
        assert_eq!(
            classified("ordinary.exe", &["https://a/--type=renderer"]).process_type,
            Unknown
        );
        assert_eq!(classified("ordinary.exe", &[]).family, None);
        assert_eq!(
            classified("ordinary.exe", &["--type=renderer"]).family,
            None
        );
        assert_eq!(
            classified(
                "custom-electron.exe",
                &["--type=renderer", "--mojo-platform-channel-handle=123"]
            )
            .process_type,
            Renderer
        );
    }

    #[test]
    fn classification_retains_no_command_line_in_json_or_debug() {
        let secret = "PRIVATE_SENTINEL_91";
        let c = classified("chrome.exe", &["--type=renderer", secret]);
        for text in [serde_json::to_string(&c).unwrap(), format!("{c:?}")] {
            assert!(!text.contains(secret));
            assert!(!text.contains("--type"));
        }
        assert!(serde_json::from_str::<BrowserProcessType>("\"user supplied type\"").is_err());
    }
}
