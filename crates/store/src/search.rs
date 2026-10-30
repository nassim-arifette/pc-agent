//! Searching saved conversations by their title, questions and written answers. The query and
//! results stay local; nothing here is sent anywhere.
use crate::{millis, ConversationSummary, Store, StoreError};
use rusqlite::params;
use serde::Serialize;

/// The most conversations one search returns.
pub const MAX_SEARCH_RESULTS: usize = 50;
const SNIPPET_CHARS: usize = 90;

/// A conversation that matches, with a short excerpt around the first match.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConversationMatch {
    #[serde(flatten)]
    pub summary: ConversationSummary,
    /// `None` when only the title matched.
    pub snippet: Option<String>,
}

/// `%` and `_` in a person's query are literal characters, not wildcards.
fn like_pattern(query: &str) -> String {
    let mut out = String::from("%");
    for c in query.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// A one-line excerpt of `text` centred on the first case-insensitive match of `needle`.
fn snippet(text: &str, needle: &str) -> Option<String> {
    let flat: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !matches!(c, '*' | '#' | '`'))
        .collect();
    let lower: Vec<char> = flat.to_lowercase().chars().collect();
    let chars: Vec<char> = flat.chars().collect();
    let needle: Vec<char> = needle.to_lowercase().chars().collect();
    if needle.is_empty() || lower.len() != chars.len() {
        return None;
    }
    let at = (0..=lower.len().checked_sub(needle.len())?)
        .find(|&i| lower[i..i + needle.len()] == needle[..])?;
    let start = at.saturating_sub(SNIPPET_CHARS / 3);
    let end = (start + SNIPPET_CHARS).min(chars.len());
    let mut out: String = chars[start..end]
        .iter()
        .collect::<String>()
        .trim()
        .to_owned();
    if start > 0 {
        out.insert(0, '…');
    }
    if end < chars.len() {
        out.push('…');
    }
    Some(out)
}

impl Store {
    /// Conversations whose title, a question or a written answer contains `query` (ignoring
    /// ASCII case), most recent first. A blank query returns nothing.
    pub fn search_conversations(&self, query: &str) -> Result<Vec<ConversationMatch>, StoreError> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 200 {
            return Ok(Vec::new());
        }
        let pattern = like_pattern(query);
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.title, c.created_at, c.updated_at,
                    (SELECT COUNT(*) FROM turns t WHERE t.conversation_id = c.id),
                    (SELECT json_extract(t.report, '$.execution') FROM turns t
                     WHERE t.conversation_id = c.id ORDER BY t.position DESC LIMIT 1),
                    (SELECT t.question FROM turns t WHERE t.conversation_id = c.id
                       AND t.question LIKE ?1 ESCAPE '\\' ORDER BY t.position LIMIT 1),
                    (SELECT json_extract(t.report, '$.answer.text') FROM turns t
                     WHERE t.conversation_id = c.id
                       AND json_extract(t.report, '$.answer.text') LIKE ?1 ESCAPE '\\'
                     ORDER BY t.position LIMIT 1)
             FROM conversations c
             WHERE c.title LIKE ?1 ESCAPE '\\'
                OR EXISTS (SELECT 1 FROM turns t WHERE t.conversation_id = c.id
                           AND (t.question LIKE ?1 ESCAPE '\\'
                                OR json_extract(t.report, '$.answer.text') LIKE ?1 ESCAPE '\\'))
             ORDER BY c.updated_at DESC, c.created_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, MAX_SEARCH_RESULTS as i64], |row| {
            let question: Option<String> = row.get(6)?;
            let answer: Option<String> = row.get(7)?;
            Ok((
                ConversationSummary {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at_unix_ms: millis(row.get(2)?),
                    updated_at_unix_ms: millis(row.get(3)?),
                    turn_count: usize::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
                    last_turn_status: row.get(5)?,
                },
                question.or(answer),
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (summary, text) = row?;
            let snippet = text.and_then(|text| snippet(&text, query));
            out.push(ConversationMatch { summary, snippet });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
