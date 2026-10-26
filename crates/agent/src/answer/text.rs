//! Citation syntax, disclosed program names and disallowed markup or commands.
use super::{Citation, Context, Issue, IssueKind};

/// Prefixes of the opaque references a model may write (`proc-1a2b3c4d`, `file-3`, `dir-7`).
const REFERENCE_PREFIXES: [&str; 4] = ["proc", "app", "file", "dir"];
/// Typographic dashes models write instead of the ASCII hyphen, e.g. `proc‑1a2b` (U+2011).
const REFERENCE_DASHES: [char; 6] = [
    '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2212}', '\u{fe63}',
];

/// Rewrites a typographic dash inside an opaque reference as the ASCII hyphen, so the reference
/// is recognised by the checks and replaced by the local name in the window. Only a dash between
/// a known prefix word and an ASCII alphanumeric character changes; other text is untouched.
pub(super) fn ascii_reference_hyphens(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        if REFERENCE_DASHES.contains(&c)
            && chars.get(i + 1).is_some_and(char::is_ascii_alphanumeric)
        {
            let word: String = chars[..i]
                .iter()
                .rev()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let boundary = i == word.len() || !chars[i - word.len() - 1].is_alphanumeric();
            if boundary && REFERENCE_PREFIXES.contains(&word.as_str()) {
                out.push('-');
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// `[O2]`, `[F1]` or `[O2, F1]`; any other bracketed text is ordinary text.
pub(super) fn check_citations(
    text: &str,
    ctx: &Context<'_>,
    issues: &mut Vec<Issue>,
) -> Vec<Citation> {
    let mut citations: Vec<Citation> = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else { break };
        let inside = &after[..close];
        rest = &after[close + 1..];
        let tokens: Vec<&str> = inside
            .split([',', ';', ' '])
            .filter(|t| !t.is_empty())
            .collect();
        if tokens.is_empty() || !tokens.iter().all(|t| is_reference(t)) {
            continue;
        }
        for token in tokens {
            match ctx.references.iter().find(|(r, _, _)| r == token) {
                Some((reference, kind, target)) => {
                    if !citations.iter().any(|c| &c.reference == reference) {
                        citations.push(Citation {
                            reference: reference.clone(),
                            kind: *kind,
                            target: target.clone(),
                        });
                    }
                }
                None => issues.push(Issue::new(IssueKind::UnknownCitation, format!("[{token}]"))),
            }
        }
    }
    citations
}

pub(crate) fn is_reference(token: &str) -> bool {
    let mut chars = token.chars();
    matches!(chars.next(), Some('O' | 'F' | 'W'))
        && (2..=6).contains(&token.len())
        && chars.all(|c| c.is_ascii_digit())
}

/// Executable names must appear in what the model received: it cannot know others.
pub(super) fn check_names(text: &str, ctx: &Context<'_>, issues: &mut Vec<Issue>) {
    let mut names = std::collections::BTreeMap::new();
    for payload in ctx.received {
        crate::privacy::collect_shared_names(payload, &mut names);
    }
    let mut from = 0;
    while let Some(found) = find_ci(&text[from..], ".exe") {
        let dot = from + found;
        let end = dot + 4;
        let start = text[..dot]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '+'))
            .last()
            .map_or(dot, |(i, _)| i);
        let boundary = text[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        let name = &text[start..end];
        if start < dot
            && boundary
            && !names.values().any(|known| {
                known.eq_ignore_ascii_case(name)
                    || text[..end].to_lowercase().ends_with(&known.to_lowercase())
            })
        {
            issues.push(Issue::new(IssueKind::UnseenName, name));
        }
        from = end;
    }
}

/// Byte offset of an ASCII needle, ignoring ASCII case. An ASCII match always starts on a
/// character boundary, so the offset can slice the original text.
fn find_ci(text: &str, needle: &str) -> Option<usize> {
    let (hay, needle) = (text.as_bytes(), needle.as_bytes());
    (0..=hay.len().checked_sub(needle.len())?)
        .find(|&i| hay[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// Words that start a command line; an inline code span beginning with one is a command.
const COMMANDS: [&str; 40] = [
    "del",
    "erase",
    "rd",
    "rmdir",
    "rm",
    "format",
    "diskpart",
    "reg",
    "bcdedit",
    "sc",
    "net",
    "netsh",
    "taskkill",
    "wmic",
    "powershell",
    "pwsh",
    "cmd",
    "dism",
    "sfc",
    "chkdsk",
    "cleanmgr",
    "cipher",
    "takeown",
    "icacls",
    "attrib",
    "shutdown",
    "schtasks",
    "winget",
    "choco",
    "curl",
    "wget",
    "iwr",
    "irm",
    "iex",
    "msiexec",
    "vssadmin",
    "powercfg",
    "bootrec",
    "fsutil",
    "mklink",
];
const COMMAND_VERBS: [&str; 12] = [
    "remove-",
    "stop-",
    "set-",
    "new-",
    "invoke-",
    "start-",
    "clear-",
    "disable-",
    "enable-",
    "uninstall-",
    "install-",
    "restart-",
];

pub(super) fn check_markup(text: &str, issues: &mut Vec<Issue>) {
    if let Some(at) = text.find("```") {
        issues.push(Issue::new(IssueKind::Command, excerpt(text, at, 60)));
    } else {
        let mut parts = text.split('`');
        parts.next();
        while let (Some(code), Some(_)) = (parts.next(), parts.clone().next()) {
            parts.next();
            let first = code.split_whitespace().next().unwrap_or("").to_lowercase();
            let first = first.trim_start_matches(".\\").trim_end_matches(".exe");
            if COMMANDS.contains(&first) || COMMAND_VERBS.iter().any(|v| first.starts_with(v)) {
                issues.push(Issue::new(IssueKind::Command, code));
            }
        }
    }
    for marker in ["](", "![", "http://", "https://", "www."] {
        if let Some(at) = find_ci(text, marker) {
            issues.push(Issue::new(IssueKind::Link, excerpt(text, at, 60)));
            break;
        }
    }
    let bytes = text.as_bytes();
    if let Some(at) = (0..bytes.len().saturating_sub(1)).find(|&i| {
        bytes[i] == b'<'
            && (bytes[i + 1].is_ascii_alphabetic() || matches!(bytes[i + 1], b'/' | b'!'))
    }) {
        issues.push(Issue::new(IssueKind::Html, excerpt(text, at, 40)));
    }
}

fn excerpt(text: &str, at: usize, max: usize) -> String {
    text[at..].chars().take(max).collect()
}
