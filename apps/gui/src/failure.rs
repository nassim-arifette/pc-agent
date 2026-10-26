//! What the window shows when a command fails. Every error a command returns to the page carries
//! a stable code from a closed set, which the page translates (`failure.<code>` in
//! `ui/js/i18n.js`); the English `message` stays for diagnosis, shown at most as a tooltip and
//! never as the main text, so the French interface shows neither a Rust name nor an English
//! sentence. A code the page cannot translate reads as a generic sentence. `turn:failed` keeps its
//! own codes (C3); a refused action adds the closed `reason` it was refused for.
use pcagent_actions::{ActionError, ApprovalRejection};
use pcagent_agent::MAX_QUESTION_BYTES;
use pcagent_inspect::elevation::ElevationError;
use pcagent_storage::StorageError;
use serde::Serialize;
use std::fmt;

/// Declares a closed set of snake_case values together with the list of all of them, which the
/// tests check against the page's dictionaries.
macro_rules! closed_set {
    ($(#[$meta:meta])* $name:ident { $($(#[$doc:meta])* $variant:ident,)+ }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($(#[$doc])* $variant,)+
        }
        impl $name {
            /// Every value, in declaration order.
            #[cfg(test)]
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];
        }
    };
}

closed_set! {
    /// Why a command failed, in words the page translates.
    Code {
        /// Anything not listed below: the page shows a generic sentence.
        Unknown,
        QuestionEmpty,
        QuestionTooLong,
        /// Another turn is still running.
        TurnRunning,
        /// No single-use preview ticket matched the send.
        PreviewRequired,
        PreviewExpired,
        /// Sharing choices, the model or the conversation changed after the preview.
        PreviewChanged,
        ImageInvalid,
        ImageUnsupported,
        ImageTooLarge,
        ImageExpired,
        ImageSharingRequired,
        ImageModelUnavailable,
        ImagePreparing,
        /// The measured selection source expired or was invalidated.
        SelectionExpired,
        ConversationMissing,
        /// The live measurement thread did not answer.
        LiveUnavailable,
        HistoryUnavailable,
        SettingsSaveFailed,
        /// Automatic model routing lacks a valid reviewed calibration.
        RoutingUnavailable,
        StartupFailed,
        FolderMissing,
        NotAFolder,
        FolderLink,
        /// Network or unmounted drives.
        UnsupportedDrive,
        FolderUnreadable,
        ScanRunning,
        ScanFailed,
        /// The scan was replaced or never completed.
        ScanUnavailable,
        ItemUnavailable,
        /// An approved action was refused; `reason` says why.
        ActionRefused,
        UacDeclined,
        TimedOut,
        Cancelled,
        LaunchFailed,
        /// The elevated helper failed or returned an unusable result.
        HelperFailed,
        /// The elevated request is no longer awaiting or holding its local approval.
        RequestExpired,
        /// The single elevated slot is taken.
        HelperBusy,
        ShuttingDown,
        Unsupported,
        SearchKeyInvalid,
        WebSourceUnavailable,
        /// Explorer or the browser could not be opened.
        OpenFailed,
    }
}

closed_set! {
    /// Why an approved action did not run (`failure.action_refused.<reason>`).
    Refusal {
        /// The plan or its single-use approval expired.
        Expired,
        /// A target changed, vanished or no longer matches the plan.
        Changed,
        /// A target is protected, or is a link.
        Protected,
        /// The action, page or cleanup is not allowed here.
        NotAllowed,
        /// The proposal was already run, declined or approved once.
        AlreadyDecided,
        /// A newer answer replaced these proposals.
        Stale,
        /// The local action journal could not be used.
        Journal,
        /// Another approved action is still running.
        Busy,
        /// The journal holds nothing to restore.
        RestoreUnavailable,
    }
}

/// A command's error as the page receives it: `{code, message}`, plus `reason` for an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub code: Code,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Refusal>,
    /// English detail for diagnosis; never the page's main text.
    pub message: String,
}

impl Failure {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            reason: None,
            message: message.into(),
        }
    }

    /// A refused action; without a reason the page says only that an error occurred.
    pub fn refused(reason: Option<Refusal>, message: impl Into<String>) -> Self {
        Self {
            code: Code::ActionRefused,
            reason,
            message: message.into(),
        }
    }
}

/// An error nobody classified keeps its text under the generic code.
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::new(Code::Unknown, message)
    }
}
impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Self::new(Code::Unknown, message)
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// A local settings file could not be written.
pub fn saving(message: impl Into<String>) -> Failure {
    Failure::new(Code::SettingsSaveFailed, message)
}

/// The local history (conversations or performance samples) could not be used.
pub fn history(error: impl fmt::Display) -> Failure {
    Failure::new(Code::HistoryUnavailable, error.to_string())
}

/// Reading or changing *Start with Windows* failed.
pub fn startup(error: impl fmt::Display) -> Failure {
    Failure::new(Code::StartupFailed, error.to_string())
}

/// The checks every question passes before a turn: not blank, within the byte limit.
pub fn check_question(question: &str) -> Result<(), Failure> {
    if question.trim().is_empty() {
        return Err(Failure::new(Code::QuestionEmpty, "Type a question first."));
    }
    if question.len() > MAX_QUESTION_BYTES {
        return Err(Failure::new(
            Code::QuestionTooLong,
            format!("The question is longer than {MAX_QUESTION_BYTES} bytes; shorten it."),
        ));
    }
    Ok(())
}

/// The elevated helper's errors. Their Rust names stay in the detail only.
pub fn elevation(error: &ElevationError) -> Failure {
    use ElevationError as E;
    let (code, text) = match error {
        E::UacDeclined => (
            Code::UacDeclined,
            "The Windows administrator prompt was declined",
        ),
        E::TimedOut => (
            Code::TimedOut,
            "The elevated task did not finish within its budget",
        ),
        E::Cancelled => (Code::Cancelled, "The elevated task was cancelled"),
        E::LaunchFailed(_) => (
            Code::LaunchFailed,
            "Windows could not start the elevated helper",
        ),
        E::HelperFailed(_) | E::Io(_) | E::InsecureJob | E::DigestMismatch | E::InvalidResponse => {
            (
                Code::HelperFailed,
                "The elevated helper failed or returned an unusable result",
            )
        }
        E::Expired | E::ApprovalRequired | E::ApprovalConsumed => (
            Code::RequestExpired,
            "The elevated request is no longer approved",
        ),
        E::Unsupported => (Code::Unsupported, "This elevated task is unsupported here"),
        E::InvalidArguments | E::InvalidRequest | E::ElevatedHost => {
            (Code::Unknown, "The elevated request was refused")
        }
    };
    Failure::new(code, format!("{text} ({error:?})."))
}

/// A storage error about the folder shown as `shown`, in the Storage screen's English wording.
pub fn storage(error: &StorageError, shown: &str) -> Failure {
    use StorageError as S;
    match error {
        S::NotADirectory => Failure::new(Code::NotAFolder, format!("{shown} is not a folder.")),
        S::ReparseRoot => Failure::new(
            Code::FolderLink,
            format!("{shown} is a link to another place. Choose the folder it points to instead."),
        ),
        S::UnsupportedVolume(what) => Failure::new(
            Code::UnsupportedDrive,
            format!("{shown} cannot be scanned: {what}."),
        ),
        other => {
            let code = match other {
                // The kind of an I/O error, or a fixed note of the storage crate.
                S::Io(kind) if kind == "NotFound" => Code::FolderMissing,
                S::Io(kind) if kind == "scan worker panicked" => Code::ScanFailed,
                S::Io(_) => Code::FolderUnreadable,
                S::ScanInProgress | S::Busy => Code::ScanRunning,
                S::UnknownScope | S::NotScanned => Code::ScanUnavailable,
                S::UnknownItem => Code::ItemUnavailable,
                _ => Code::Unknown,
            };
            Failure::new(code, format!("Cannot scan {shown}: {other}."))
        }
    }
}

/// Why the action broker refused an approved plan.
pub fn action(error: &ActionError) -> Failure {
    use ActionError as A;
    use ApprovalRejection as R;
    let reason = match error {
        A::Expired | A::ApprovalRejected(R::Expired) => Some(Refusal::Expired),
        A::Resolve { .. }
        | A::TargetKindMismatch(_)
        | A::IdentityChanged(_)
        | A::DuplicateTarget(_)
        | A::FingerprintMismatch
        | A::ApprovalRejected(R::PlanMismatch) => Some(Refusal::Changed),
        A::Protected(_) | A::ReparsePoint(_) => Some(Refusal::Protected),
        A::InvalidIntent(_)
        | A::SettingsPageNotAllowed(_)
        | A::UnknownAdapter(_)
        | A::NotExecutable { .. } => Some(Refusal::NotAllowed),
        A::IllegalTransition { .. } | A::ApprovalRejected(R::UnknownOrAlreadyUsed) => {
            Some(Refusal::AlreadyDecided)
        }
        A::ApprovalRejected(R::TooManyOutstanding) => Some(Refusal::Busy),
        A::Journal(_) => Some(Refusal::Journal),
        A::ApprovalRejected(R::WrongSession | R::InvalidTtl) => None,
    };
    Failure::refused(reason, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_actions::{ActionKind, PlanState};
    use pcagent_shared::Status;
    use serde_json::json;

    fn name(value: impl Serialize) -> String {
        serde_json::to_value(value)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn a_failure_reaches_the_page_as_code_and_message_with_a_reason_only_when_refused() {
        let failure = Failure::new(Code::UacDeclined, "declined");
        assert_eq!(
            serde_json::to_value(&failure).unwrap(),
            json!({"code":"uac_declined","message":"declined"})
        );
        let refused = Failure::refused(Some(Refusal::RestoreUnavailable), "no journal record");
        assert_eq!(
            serde_json::to_value(&refused).unwrap(),
            json!({"code":"action_refused","reason":"restore_unavailable","message":"no journal record"})
        );
        // Text nobody classified stays readable for diagnosis under the generic code.
        let unknown: Failure = "Something new.".into();
        assert_eq!(unknown.code, Code::Unknown);
        assert_eq!(unknown.to_string(), "Something new.");
        assert_eq!(Failure::from(String::from("x")).code, Code::Unknown);
        assert_eq!(saving("disk full").code, Code::SettingsSaveFailed);
        assert_eq!(history("locked").code, Code::HistoryUnavailable);
        assert_eq!(startup("denied").code, Code::StartupFailed);
    }

    #[test]
    fn every_code_and_reason_has_an_english_and_a_french_sentence() {
        let dictionaries = concat!(
            include_str!("../ui/js/i18n/en.js"),
            include_str!("../ui/js/i18n/fr.js"),
        );
        let count = |key: &str| dictionaries.matches(&format!("\n  \"{key}\": \"")).count();
        let mut keys: Vec<String> = Code::ALL
            .iter()
            .map(|code| format!("failure.{}", name(code)))
            .collect();
        keys.extend(
            Refusal::ALL
                .iter()
                .map(|reason| format!("failure.action_refused.{}", name(reason))),
        );
        for key in keys {
            assert_eq!(
                count(&key),
                2,
                "{key} needs one English and one French entry"
            );
        }
        let codes: std::collections::BTreeSet<_> = Code::ALL.iter().map(name).collect();
        assert_eq!(codes.len(), Code::ALL.len(), "codes are distinct");
    }

    #[test]
    fn questions_are_refused_when_blank_or_over_the_byte_limit() {
        assert!(check_question("Why is my PC slow?").is_ok());
        for blank in ["", "   ", "\n\t"] {
            assert_eq!(check_question(blank).unwrap_err().code, Code::QuestionEmpty);
        }
        let long = "é".repeat(MAX_QUESTION_BYTES / 2 + 1);
        assert_eq!(
            check_question(&long).unwrap_err().code,
            Code::QuestionTooLong
        );
        assert!(check_question(&"a".repeat(MAX_QUESTION_BYTES)).is_ok());
    }

    #[test]
    fn elevated_helper_errors_never_reach_the_page_as_their_rust_names() {
        use ElevationError as E;
        let cases = [
            (E::UacDeclined, Code::UacDeclined),
            (E::TimedOut, Code::TimedOut),
            (E::Cancelled, Code::Cancelled),
            (E::LaunchFailed(1155), Code::LaunchFailed),
            (E::HelperFailed(3), Code::HelperFailed),
            (
                E::Io(std::io::ErrorKind::PermissionDenied),
                Code::HelperFailed,
            ),
            (E::InsecureJob, Code::HelperFailed),
            (E::DigestMismatch, Code::HelperFailed),
            (E::InvalidResponse, Code::HelperFailed),
            (E::Expired, Code::RequestExpired),
            (E::ApprovalRequired, Code::RequestExpired),
            (E::ApprovalConsumed, Code::RequestExpired),
            (E::Unsupported, Code::Unsupported),
            (E::InvalidArguments, Code::Unknown),
            (E::InvalidRequest, Code::Unknown),
            (E::ElevatedHost, Code::Unknown),
        ];
        for (error, code) in cases {
            let failure = elevation(&error);
            assert_eq!(failure.code, code, "{error:?}");
            assert!(failure.reason.is_none());
            // The Rust name is kept in the detail for diagnosis, after an English sentence.
            assert!(failure.message.ends_with(&format!("({error:?}).")));
            assert!(!failure.message.starts_with(&format!("{error:?}")));
        }
    }

    #[test]
    fn storage_errors_say_what_happened_to_the_folder() {
        use StorageError as S;
        let shown = r"C:\Users\fixture\Downloads";
        let cases = [
            (S::NotADirectory, Code::NotAFolder),
            (S::ReparseRoot, Code::FolderLink),
            (
                S::UnsupportedVolume("network drive"),
                Code::UnsupportedDrive,
            ),
            (S::Io("NotFound".into()), Code::FolderMissing),
            (S::Io("PermissionDenied".into()), Code::FolderUnreadable),
            (S::Io("invalid root name".into()), Code::FolderUnreadable),
            (S::Io("scan worker panicked".into()), Code::ScanFailed),
            (S::ScanInProgress, Code::ScanRunning),
            (S::Busy, Code::ScanRunning),
            (S::UnknownScope, Code::ScanUnavailable),
            (S::NotScanned, Code::ScanUnavailable),
            (S::UnknownItem, Code::ItemUnavailable),
            (S::InvalidPage, Code::Unknown),
        ];
        for (error, code) in cases {
            let failure = storage(&error, shown);
            assert_eq!(failure.code, code, "{error:?}");
            assert!(failure.message.contains(shown), "{}", failure.message);
        }
        assert_eq!(
            storage(&S::NotADirectory, shown).message,
            format!("{shown} is not a folder.")
        );
        assert_eq!(
            storage(&S::Io("PermissionDenied".into()), shown).message,
            format!("Cannot scan {shown}: Scope root cannot be accessed: PermissionDenied.")
        );
    }

    #[test]
    fn a_refused_action_carries_one_closed_reason_or_none() {
        use ActionError as A;
        use ApprovalRejection as R;
        let io = || std::io::Error::other("fixture journal");
        let cases = [
            (A::Expired, Some(Refusal::Expired)),
            (A::ApprovalRejected(R::Expired), Some(Refusal::Expired)),
            (
                A::Resolve {
                    target_ref: "t1".into(),
                    status: Status::Unsupported,
                },
                Some(Refusal::Changed),
            ),
            (A::TargetKindMismatch("t1".into()), Some(Refusal::Changed)),
            (A::IdentityChanged("t1".into()), Some(Refusal::Changed)),
            (A::DuplicateTarget("t1".into()), Some(Refusal::Changed)),
            (A::FingerprintMismatch, Some(Refusal::Changed)),
            (A::ApprovalRejected(R::PlanMismatch), Some(Refusal::Changed)),
            (A::Protected("t1".into()), Some(Refusal::Protected)),
            (A::ReparsePoint("t1".into()), Some(Refusal::Protected)),
            (A::InvalidIntent("fixture"), Some(Refusal::NotAllowed)),
            (
                A::SettingsPageNotAllowed("ms-settings:x".into()),
                Some(Refusal::NotAllowed),
            ),
            (A::UnknownAdapter("x".into()), Some(Refusal::NotAllowed)),
            (
                A::NotExecutable {
                    kind: ActionKind::RequestNormalClose,
                    reason: "fixture",
                },
                Some(Refusal::NotAllowed),
            ),
            (
                A::IllegalTransition {
                    from: PlanState::Completed,
                    to: PlanState::Executing,
                },
                Some(Refusal::AlreadyDecided),
            ),
            (
                A::ApprovalRejected(R::UnknownOrAlreadyUsed),
                Some(Refusal::AlreadyDecided),
            ),
            (
                A::ApprovalRejected(R::TooManyOutstanding),
                Some(Refusal::Busy),
            ),
            (A::Journal(io()), Some(Refusal::Journal)),
            (A::ApprovalRejected(R::WrongSession), None),
            (A::ApprovalRejected(R::InvalidTtl), None),
        ];
        for (error, reason) in cases {
            let failure = action(&error);
            assert_eq!(failure.code, Code::ActionRefused);
            assert_eq!(failure.reason, reason, "{error}");
            assert_eq!(failure.message, error.to_string());
        }
    }
}
