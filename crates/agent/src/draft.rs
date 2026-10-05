//! The answer text while `submit_answer` is still being written. A streamed tool call arrives as
//! growing, incomplete JSON; this reads its top-level `answer` string as far as it has arrived so
//! an interface can show the draft. A draft is never evidence and is never checked: the answer
//! that counts is the complete call, reviewed by the controller.

/// The `answer` field received so far, or `None` before its value starts. Escapes cut in half at
/// the end are left out until the rest arrives.
/// The answer field of submit_answer (answer.rs). It was once `text`, which left the live draft
/// empty for every real answer.
const ANSWER_KEY: &str = "answer";

pub fn draft_text(arguments: &str) -> Option<String> {
    let bytes = arguments.as_bytes();
    let mut i = 0;
    let mut depth = 0usize;
    // Walks the object: strings are skipped as units so braces inside them do not count.
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'"' => {
                let (key, end) = read_string(arguments, i + 1);
                i = end?;
                if depth != 1 || key != ANSWER_KEY {
                    continue;
                }
                // A key is followed by a colon; a string value is not.
                let mut j = i;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j >= bytes.len() {
                    return None;
                }
                if bytes[j] != b':' {
                    continue;
                }
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j >= bytes.len() || bytes[j] != b'"' {
                    return None;
                }
                return Some(read_string(arguments, j + 1).0);
            }
            _ => i += 1,
        }
    }
    None
}

/// Decodes a JSON string starting after its opening quote. Returns the text and the index after
/// the closing quote, or no index when the string has not finished arriving.
fn read_string(source: &str, start: usize) -> (String, Option<usize>) {
    let mut out = String::new();
    let mut chars = source[start..].char_indices().peekable();
    while let Some((offset, c)) = chars.next() {
        match c {
            '"' => return (out, Some(start + offset + 1)),
            '\\' => {
                let Some((_, escape)) = chars.next() else {
                    return (out, None);
                };
                match escape {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' | 'f' => {}
                    'u' => {
                        let hex: String = (0..4)
                            .filter_map(|_| chars.next().map(|(_, c)| c))
                            .collect();
                        if hex.len() < 4 {
                            return (out, None);
                        }
                        let Ok(code) = u16::from_str_radix(&hex, 16) else {
                            continue;
                        };
                        if (0xD800..0xDC00).contains(&code) {
                            // A surrogate pair: the low half follows as another \u escape.
                            let rest: String = (0..6)
                                .filter_map(|_| chars.next().map(|(_, c)| c))
                                .collect();
                            if rest.len() < 6 {
                                return (out, None);
                            }
                            if let Some(low) = rest
                                .strip_prefix("\\u")
                                .and_then(|h| u16::from_str_radix(h, 16).ok())
                            {
                                if let Some(c) =
                                    char::decode_utf16([code, low]).next().and_then(Result::ok)
                                {
                                    out.push(c);
                                }
                            }
                        } else if let Some(c) = char::from_u32(code.into()) {
                            out.push(c);
                        }
                    }
                    other => out.push(other),
                }
            }
            c => out.push(c),
        }
    }
    (out, None)
}

#[cfg(test)]
mod tests {
    use super::draft_text;

    #[test]
    fn reads_the_text_as_far_as_it_arrived() {
        assert_eq!(draft_text(""), None);
        assert_eq!(draft_text("{\"te"), None);
        assert_eq!(draft_text("{\"answer\": "), None);
        assert_eq!(
            draft_text("{\"answer\": \"Votre PC"),
            Some("Votre PC".into())
        );
        assert_eq!(
            draft_text("{\"confidence\":\"high\",\"answer\":\"Ligne 1\\nLigne 2 [O1] \\\"cité\\\"\",\"follow_ups\":[]}"),
            Some("Ligne 1\nLigne 2 [O1] \"cité\"".into())
        );
    }

    #[test]
    fn half_escapes_wait_for_the_rest() {
        assert_eq!(draft_text("{\"answer\":\"a\\"), Some("a".into()));
        assert_eq!(draft_text("{\"answer\":\"a\\u00"), Some("a".into()));
        assert_eq!(draft_text("{\"answer\":\"a\\u00e9b"), Some("aéb".into()));
        assert_eq!(
            draft_text("{\"answer\":\"\\ud83d\\ude00!"),
            Some("😀!".into())
        );
    }

    #[test]
    fn only_the_top_level_answer_counts() {
        // A nested "answer" or a value equal to "answer" is not the answer, and the old "text"
        // key is not read.
        let nested = "{\"meta\":{\"answer\":\"no\"},\"category\":\"answer\",\"answer\":\"yes";
        assert_eq!(draft_text(nested), Some("yes".into()));
        assert_eq!(draft_text("{\"category\":\"answer\"}"), None);
        assert_eq!(draft_text("{\"text\":\"old key\"}"), None);
        assert_eq!(
            draft_text("{\"note\":\"a { brace\",\"answer\":\"ok\"}"),
            Some("ok".into())
        );
    }
}
