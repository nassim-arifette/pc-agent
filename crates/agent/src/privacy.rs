//! Final C5 projection shared by consent previews and every model data message.
//!
//! Collectors still use positive field allowlists. This guard removes local identifiers from
//! otherwise permitted text; it is not permission to serialize arbitrary native records.
use serde_json::Value;
use std::collections::BTreeMap;

const OMITTED: &str = "[private content omitted]";
const NAME_FIELDS: &[&str] = &[
    "process_names",
    "shared_names",
    "service_names",
    "application_names",
    "startup_names",
    "hardware_names",
    "shared_name",
    "shared_path",
    "shared_ancestors",
];

/// Frozen local identifiers for one consent preview and investigation. Never serialize this
/// object, put it in an HTTP header, or print its contents.
#[derive(Clone, Default)]
pub struct PrivacyContext {
    roots: Vec<(String, String)>,
    identities: Vec<String>,
    secrets: Vec<String>,
}

impl std::fmt::Debug for PrivacyContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivacyContext { identifiers: [withheld] }")
    }
}

impl PrivacyContext {
    /// Read process-local identity metadata, never a file or its contents. Capturing once keeps
    /// the reviewed projection stable if the environment later changes.
    pub fn local() -> Self {
        let env = |key| std::env::var(key).unwrap_or_default();
        let profile = env("USERPROFILE");
        let user = env("USERNAME");
        let machine = env("COMPUTERNAME");
        let secrets: Vec<String> = ["NEBIUS_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY"]
            .iter()
            .map(|key| env(key))
            .filter(|value| !value.is_empty())
            .collect();
        let mut context = Self::from_identifiers(
            &profile,
            &user,
            &machine,
            &secrets.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        for key in ["USER", "HOSTNAME"] {
            let value = env(key);
            if !value.is_empty() {
                context.identities.push(value);
            }
        }
        for (key, alias) in [
            ("HOME", "%USERPROFILE%"),
            ("LOCALAPPDATA", "%LOCALAPPDATA%"),
            ("APPDATA", "%APPDATA%"),
            ("ProgramFiles", "%PROGRAMFILES%"),
            ("ProgramFiles(x86)", "%PROGRAMFILES(X86)%"),
            ("ProgramData", "%PROGRAMDATA%"),
            ("SystemRoot", "%WINDIR%"),
        ] {
            context.add_root(&env(key), alias);
        }
        context
    }

    /// Explicit metadata is useful to hosts with a fixed environment and to synthetic tests.
    pub fn from_identifiers(profile: &str, user: &str, machine: &str, secrets: &[&str]) -> Self {
        let mut context = Self {
            roots: Vec::new(),
            identities: [user, machine]
                .into_iter()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            secrets: secrets
                .iter()
                .filter(|s| !s.is_empty())
                .map(|s| (*s).to_owned())
                .collect(),
        };
        context.add_root(profile, "%USERPROFILE%");
        context
    }

    fn add_root(&mut self, path: &str, alias: &str) {
        let path = path.trim_end_matches(['\\', '/']);
        if path.len() > 2 {
            self.roots.push((path.replace('/', "\\"), alias.to_owned()));
            self.roots
                .sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));
        }
    }

    /// Add a credential loaded by the trusted host (for example from its local settings file).
    /// The secret remains local and cannot be recovered from this type's debug representation.
    pub fn protect_secret(&mut self, secret: &str) {
        if !secret.is_empty() && !self.secrets.iter().any(|known| known == secret) {
            self.secrets.push(secret.to_owned());
        }
    }

    /// Display names are bounded basenames, scrubbed for identity/secret text as well as control
    /// and bidi characters. A rejected command or credential is never exposed as a name.
    pub fn shared_name(&self, name: &str) -> Option<String> {
        let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
        let cleaned = self.sanitize_text(base);
        let cleaned: String = cleaned
            .chars()
            .filter(|c| !c.is_control())
            .take(crate::MAX_SHARED_NAME_CHARS)
            .collect();
        let cleaned = cleaned.trim();
        (!cleaned.is_empty() && cleaned != OMITTED).then(|| cleaned.to_owned())
    }

    /// A path is shareable only under a known local root. No drive path, UNC server, profile
    /// prefix, traversal component or arbitrary folder root leaves the machine.
    pub fn shared_path(&self, path: &str) -> Option<String> {
        let normalized = path.replace('/', "\\");
        for (root, alias) in &self.roots {
            if normalized.len() < root.len() || !normalized.is_char_boundary(root.len()) {
                continue;
            }
            if normalized[..root.len()].to_lowercase() != root.to_lowercase() {
                continue;
            }
            let rest = &normalized[root.len()..];
            if !rest.is_empty() && !rest.starts_with('\\') {
                continue;
            }
            if rest.split('\\').any(|part| matches!(part, "." | "..")) {
                return None;
            }
            let relative = rest
                .split('\\')
                .map(|part| self.sanitize_text(part))
                .collect::<Vec<_>>()
                .join("\\");
            return Some(format!("{alias}{relative}"));
        }
        None
    }

    /// Sanitize free text without interpreting it as instructions. Obvious pasted commands,
    /// credentials and content blocks are omitted; ordinary diagnostic prose remains usable.
    pub fn sanitize_text(&self, text: &str) -> String {
        let text: String = text.chars().filter(|c| {
            (!c.is_control() || matches!(c, '\n' | '\r' | '\t'))
                && !matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        }).collect();
        let mut fenced = false;
        let mut private_key = false;
        let mut content = false;
        let mut lines = Vec::new();
        for line in text.split('\n') {
            let trim = line.trim();
            let lower = trim.to_ascii_lowercase();
            if trim.starts_with("```") || trim.starts_with("~~~") {
                if !fenced {
                    lines.push(OMITTED.to_owned());
                }
                fenced = !fenced;
                continue;
            }
            if lower.contains("-----begin ") && lower.contains("private key") {
                private_key = true;
                lines.push(OMITTED.to_owned());
                continue;
            }
            if private_key {
                if lower.contains("-----end ") {
                    private_key = false;
                }
                continue;
            }
            if fenced {
                continue;
            }
            if content {
                if trim.is_empty() {
                    content = false;
                } else {
                    continue;
                }
            }
            if [
                "file contents:",
                "file content:",
                "contenu du fichier",
                "contenu de fichier",
            ]
            .iter()
            .any(|label| lower.contains(label))
            {
                content = true;
                lines.push(OMITTED.to_owned());
            } else if sensitive_line(&lower) {
                lines.push(OMITTED.to_owned());
            } else {
                lines.push(self.scrub_paths_and_tokens(line));
            }
        }
        lines.join("\n")
    }

    fn scrub_paths_and_tokens(&self, text: &str) -> String {
        let mut out = String::new();
        let mut at = 0;
        while at < text.len() {
            let tail = &text[at..];
            let bytes = tail.as_bytes();
            let absolute = (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'\\' | b'/'))
                || tail.starts_with("\\\\")
                || tail.starts_with("/home/")
                || tail.starts_with("/Users/");
            if absolute {
                let end = tail
                    .find(['\n', '\r', '\t', '"', '\'', '<', '>', '`'])
                    .unwrap_or(tail.len());
                // Keep a complete known root even when the account name contains spaces. A
                // quoted path also includes spaces; unquoted suffixes stop before prose.
                let normalized = tail.replace('/', "\\");
                let prefix = self
                    .roots
                    .iter()
                    .find_map(|(root, _)| {
                        normalized
                            .get(..root.len())
                            .filter(|value| value.to_lowercase() == root.to_lowercase())
                            .map(|_| root.len())
                    })
                    .unwrap_or(0)
                    .min(end);
                let quoted = text[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| matches!(c, '"' | '\''));
                let end = if quoted {
                    end
                } else {
                    tail[prefix..end].find(' ').map_or(end, |i| prefix + i)
                };
                let path = &tail[..end];
                out.push_str(&self.shared_path(path).unwrap_or_else(|| "[path]".into()));
                at += end;
            } else {
                let c = tail.chars().next().expect("nonempty text");
                out.push(c);
                at += c.len_utf8();
            }
        }
        self.scrub_tokens(&out)
    }

    fn scrub_tokens(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for secret in &self.secrets {
            out = out.replace(secret, "[secret]");
        }
        for identity in &self.identities {
            out = replace_word_ci(&out, identity, "[local identity]");
        }
        let mut cleaned = String::new();
        for token in out.split_inclusive(char::is_whitespace) {
            let core = token.trim_end_matches(char::is_whitespace);
            let trimmed = core.trim_matches([
                '"', '\'', '(', ')', '[', ']', '{', '}', ',', ';', '.', ':', '!', '?',
            ]);
            let sensitive = trimmed.contains('@')
                && trimmed
                    .split_once('@')
                    .is_some_and(|(a, b)| !a.is_empty() && b.contains('.'))
                || is_guid(trimmed)
                || trimmed.starts_with("S-1-5-")
                || is_token(trimmed);
            if sensitive {
                cleaned.push_str("[private identifier]");
            } else {
                cleaned.push_str(core);
            }
            cleaned.push_str(&token[core.len()..]);
        }
        cleaned
    }

    /// Apply after a collector's field allowlist and before recording what the model received.
    /// Sensitive object keys are discarded rather than leaving an apparently usable value.
    pub fn sanitize_value(&self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.sanitize_text(text),
            Value::Array(items) => items.iter_mut().for_each(|v| self.sanitize_value(v)),
            Value::Object(map) => {
                let old = std::mem::take(map);
                for (key, mut child) in old {
                    if forbidden_field(&key) {
                        continue;
                    }
                    let cleaned_key = self.sanitize_text(&key);
                    if cleaned_key != key {
                        continue;
                    }
                    self.sanitize_value(&mut child);
                    map.insert(key, child);
                }
            }
            _ => {}
        }
    }
}

fn forbidden_field(key: &str) -> bool {
    let key: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        key.as_str(),
        "username"
            | "userprofile"
            | "computername"
            | "machinename"
            | "machineid"
            | "hostname"
            | "serialnumber"
            | "serial"
            | "productid"
            | "deviceid"
            | "hardwareid"
            | "commandline"
            | "command"
            | "arguments"
            | "filecontent"
            | "filecontents"
            | "contents"
            | "apikey"
            | "password"
            | "secret"
            | "accesstoken"
            | "authorization"
            | "email"
            | "bootid"
            | "collectorsessionid"
            | "creationtimefiletime"
            | "sid"
    )
}

fn sensitive_line(lower: &str) -> bool {
    [
        "command_line",
        "commandline",
        "command line:",
        "ligne de commande",
        "authorization:",
        "api_key=",
        "api_key:",
        "api key:",
        "apikey=",
        "password=",
        "password:",
        "mot de passe:",
        "secret=",
        "secret:",
        "token=",
        "token:",
        "bearer ",
        "machine_id",
        "machine id:",
        "serial_number",
        "serial number:",
        "device_id",
        "sid:",
    ]
    .iter()
    .any(|label| lower.contains(label))
        || lower.starts_with("$ ")
        || lower.starts_with("ps>")
        || [
            "powershell ",
            "powershell.exe ",
            "pwsh ",
            "cmd /",
            "cmd.exe /",
            "bash -",
            "sh -",
            "get-content ",
            "get-childitem ",
            "get-process ",
            "get-ciminstance ",
            "wmic ",
            "reg query ",
            "reg add ",
            "remove-item ",
            "invoke-webrequest ",
            "curl ",
            "taskkill ",
            "diskpart",
            "sfc /",
            "dism /",
            "netsh ",
            "bcdedit ",
        ]
        .iter()
        .any(|command| lower.contains(command))
}

fn is_guid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

fn is_token(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    ["sk-", "ghp_", "github_pat_", "hf_", "akia"]
        .iter()
        .any(|prefix| lower.starts_with(prefix) && value.len() > prefix.len() + 8)
        || (value.len() > 60 && value.starts_with("eyJ") && value.matches('.').count() == 2)
}

pub(crate) fn replace_word_ci(text: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
    let folded = needle.to_lowercase();
    let length = needle.chars().count();
    let mut out = String::new();
    let mut at = 0;
    while at < text.len() {
        let tail = &text[at..];
        let end = tail
            .char_indices()
            .nth(length)
            .map_or(tail.len(), |(i, _)| i);
        let word = |c: char| c.is_alphanumeric();
        let boundary = text[..at].chars().next_back().is_none_or(|c| !word(c))
            && tail[end..].chars().next().is_none_or(|c| !word(c));
        if boundary && tail[..end].to_lowercase() == folded {
            out.push_str(replacement);
            at += end;
        } else {
            let c = tail.chars().next().expect("nonempty text");
            out.push(c);
            at += c.len_utf8();
        }
    }
    out
}

/// Names permitted in structured observations, retained locally to reapply later choices.
pub(crate) fn collect_shared_names(value: &Value, out: &mut BTreeMap<String, String>) {
    match value {
        Value::Array(items) => items.iter().for_each(|v| collect_shared_names(v, out)),
        Value::Object(map) => {
            for (key, child) in map {
                if NAME_FIELDS.contains(&key.as_str()) {
                    if let Some(names) = child.as_object() {
                        for (reference, name) in names {
                            if let Some(name) = name.as_str() {
                                out.insert(reference.clone(), name.to_owned());
                            }
                        }
                    } else if let Some(name) = child.as_str() {
                        out.insert(name.to_owned(), name.to_owned());
                    } else if let Some(ancestors) = child.as_array() {
                        for ancestor in ancestors {
                            for field in ["name", "relative_path"] {
                                if let Some(name) = ancestor[field].as_str() {
                                    out.insert(name.to_owned(), name.to_owned());
                                }
                            }
                        }
                    } else {
                        collect_shared_names(child, out);
                    }
                } else {
                    collect_shared_names(child, out);
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn anonymize(value: &mut Value, names: &BTreeMap<String, String>) {
    match value {
        Value::String(text) => *text = anonymize_text(text, names),
        Value::Array(items) => items.iter_mut().for_each(|v| anonymize(v, names)),
        Value::Object(map) => {
            map.retain(|key, _| !NAME_FIELDS.contains(&key.as_str()));
            for child in map.values_mut() {
                anonymize(child, names);
            }
        }
        _ => {}
    }
}

pub(crate) fn anonymize_text(text: &str, names: &BTreeMap<String, String>) -> String {
    let mut forms: Vec<String> = names
        .values()
        .flat_map(|name| {
            let mut forms = vec![name.clone()];
            if name.to_ascii_lowercase().ends_with(".exe") && name.len() > 6 {
                forms.push(name[..name.len() - 4].to_owned());
            }
            forms
        })
        .collect();
    forms.sort_by_key(|name| std::cmp::Reverse(name.len()));
    forms.dedup();
    forms.into_iter().fold(text.to_owned(), |text, name| {
        replace_word_ci(&text, &name, "[name]")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> PrivacyContext {
        PrivacyContext::from_identifiers(
            r"C:\Users\Alice",
            "Alice",
            "ALICE-LAPTOP",
            &["known-example-secret"],
        )
    }

    #[test]
    fn identity_paths_secrets_and_contact_details_are_removed_without_losing_measurements() {
        let text = fixture().sanitize_text("Alice, ALICE-LAPTOP: C:\\Users\\Alice\\Downloads\\report.pdf 81 %; alice@example.test known-example-secret 01234567-89ab-cdef-0123-456789abcdef");
        for forbidden in [
            "Alice",
            "ALICE-LAPTOP",
            "C:",
            "example.test",
            "known-example-secret",
            "01234567",
        ] {
            assert!(!text.contains(forbidden), "unexpected identity marker");
        }
        assert!(text.contains("%USERPROFILE%\\Downloads\\report.pdf"));
        assert!(text.contains("81 %"));
        assert!(!format!("{:?}", fixture()).contains("Alice"));
    }

    #[test]
    fn structured_sensitive_fields_and_personal_keys_are_dropped_recursively() {
        let mut value = json!({"question":"Why is it slow?", "nested":[{"command_line":"SECRET-COMMAND", "username":"SECRET-USER", "file_contents":"SECRET-CONTENT", "serial_number":"SECRET-SERIAL", "memory_bytes":42}], "Alice":{"value":3}});
        fixture().sanitize_value(&mut value);
        assert!(!value.to_string().contains("SECRET"));
        assert!(value.get("Alice").is_none());
        assert_eq!(value["nested"][0]["memory_bytes"], 42);
    }

    #[test]
    fn pasted_commands_and_contents_stay_local() {
        for text in [
            "powershell -Command SECRET-PAYLOAD",
            "API_KEY=SECRET-PAYLOAD",
            "```text\nSECRET-PAYLOAD\n```",
            "File contents:\nSECRET-PAYLOAD",
            "-----BEGIN PRIVATE KEY-----\nSECRET-PAYLOAD\n-----END PRIVATE KEY-----",
        ] {
            assert!(!fixture().sanitize_text(text).contains("SECRET-PAYLOAD"));
        }
    }

    #[test]
    fn paths_require_known_roots_and_preserve_relative_names() {
        assert_eq!(
            fixture()
                .shared_path(r"c:\users\ALICE\Downloads\report 2026.pdf")
                .as_deref(),
            Some(r"%USERPROFILE%\Downloads\report 2026.pdf")
        );
        for path in [
            r"C:\Users\Alice-other\secret.txt",
            r"C:\Users\Alice\..\Bob\secret.txt",
            r"\\PRIVATE-SERVER\secret",
            r"D:\private\file.txt",
        ] {
            assert!(fixture().shared_path(path).is_none());
        }
        assert_eq!(
            fixture()
                .shared_name("C:\\Users\\Alice\\Chrome\u{202e}.exe")
                .as_deref(),
            Some("Chrome.exe")
        );
        let spaced =
            PrivacyContext::from_identifiers(r"C:\Users\Alice Smith", "Alice Smith", "", &[]);
        let text = spaced.sanitize_text(
            r#"Question about "C:\Users\Alice Smith\Downloads\report 2026.pdf" now"#,
        );
        assert!(!text.contains("Alice"));
        assert!(!text.contains("Smith"));
        assert!(text.contains(r"%USERPROFILE%\Downloads\report 2026.pdf"));
    }

    #[test]
    fn anonymization_removes_all_name_types_and_short_executable_names() {
        let mut value = json!({"shared_names":{"service-1":"Private Service"}, "process_names":{"proc-1":"SecretBrowser.exe"}, "items":[{"shared_name":"private.pdf", "shared_path":"%USERPROFILE%\\Downloads\\private.pdf"}], "question":"SecretBrowser and Private Service use private.pdf"});
        let mut names = BTreeMap::new();
        collect_shared_names(&value, &mut names);
        anonymize(&mut value, &names);
        let wire = value.to_string().to_lowercase();
        for name in [
            "private service",
            "secretbrowser",
            "private.pdf",
            "downloads",
            "shared_name",
            "process_names",
        ] {
            assert!(!wire.contains(name));
        }
    }
}
