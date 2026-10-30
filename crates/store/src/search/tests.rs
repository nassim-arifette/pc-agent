use super::*;
use serde_json::json;

fn store_with(turns: &[(&str, &str)]) -> (Store, Vec<String>) {
    let mut store = Store::open_in_memory().unwrap();
    let mut ids = Vec::new();
    for (i, (question, answer)) in turns.iter().enumerate() {
        let id = store.create(question, 1_000 + i as u64).unwrap();
        store
            .add_turn(
                &id,
                question,
                2_000 + i as u64,
                &json!({"answer": {"text": answer}, "execution": "completed"}),
            )
            .unwrap();
        ids.push(id);
    }
    (store, ids)
}

#[test]
fn finds_titles_questions_and_answers_most_recent_first() {
    let (store, ids) = store_with(&[
        (
            "Why is my PC slow?",
            "Memory is almost full: close **chrome.exe** tabs.",
        ),
        (
            "What can I delete on C:?",
            "Your Downloads folder holds 212 GiB.",
        ),
    ]);
    let found = store.search_conversations("CHROME").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].summary.id, ids[0]);
    assert!(found[0].snippet.as_deref().unwrap().contains("chrome.exe"));
    assert!(!found[0].snippet.as_deref().unwrap().contains("**"));
    let both = store.search_conversations("e").unwrap();
    assert_eq!(
        both.iter()
            .map(|m| m.summary.id.clone())
            .collect::<Vec<_>>(),
        vec![ids[1].clone(), ids[0].clone()]
    );
    assert!(store.search_conversations("downloads").unwrap()[0]
        .snippet
        .as_deref()
        .unwrap()
        .contains("Downloads"));
}

#[test]
fn wildcards_are_literal_and_blank_queries_return_nothing() {
    let (store, _) = store_with(&[("Disk at 100% busy", "The disk was 100% busy for 3 minutes.")]);
    assert_eq!(store.search_conversations("100%").unwrap().len(), 1);
    assert!(store.search_conversations("10_%").unwrap().is_empty());
    // A literal percent sign matches only text that contains one.
    assert_eq!(store.search_conversations("%").unwrap().len(), 1);
    assert!(store.search_conversations("x%").unwrap().is_empty());
    assert!(store.search_conversations("   ").unwrap().is_empty());
}

#[test]
fn snippets_are_short_one_line_excerpts() {
    let long = format!(
        "{} memory is the cause {}",
        "word ".repeat(60),
        "more ".repeat(60)
    );
    let text = snippet(&long, "memory").unwrap();
    assert!(text.starts_with('…') && text.ends_with('…'));
    assert!(text.contains("memory is the cause"));
    assert!(text.chars().count() <= SNIPPET_CHARS + 2);
    assert_eq!(
        snippet("line one\nline two", "two").unwrap(),
        "line one line two"
    );
}
