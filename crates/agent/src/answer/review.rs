//! Assemble the checks and retain the best displayable answer after bounded repairs.
use super::{
    grounding,
    quantities::claims,
    submission::{malformed, Submission},
    text::{ascii_reference_hyphens, check_citations, check_markup, check_names},
    Answer, Context, Issue, IssueKind, Rejection, MAX_ACTION_REFS, MAX_ANSWER_CHARS,
    MAX_FOLLOW_UPS, MAX_FOLLOW_UP_CHARS,
};

/// How much of a shown answer is unverified: one per number without a source, one per other
/// problem in the text. Used to keep the best of several attempts.
pub(crate) fn unverified_claims(answer: &Answer) -> usize {
    answer
        .unverified
        .iter()
        .map(|issue| match issue.kind {
            IssueKind::MissingCitation => claims(&issue.excerpt).len().max(1),
            _ => 1,
        })
        .sum()
}

/// Parses and checks one submission. `Ok` is an answer without problems.
pub(crate) fn review(arguments: &str, ctx: &Context<'_>) -> Result<Answer, Rejection> {
    let submission = match serde_json::from_str::<Submission>(arguments) {
        Ok(submission) => submission,
        Err(error) => {
            return Err((
                vec![Issue::new(IssueKind::Malformed, malformed(&error))],
                None,
            ));
        }
    };
    let mut issues = Vec::new();
    let mut text = ascii_reference_hyphens(submission.answer.trim());
    if text.is_empty() {
        return Err((vec![Issue::new(IssueKind::Empty, "")], None));
    }
    if text.chars().count() > MAX_ANSWER_CHARS {
        issues.push(Issue::new(IssueKind::TooLong, ""));
        text = text.chars().take(MAX_ANSWER_CHARS).collect::<String>() + "…";
    }
    let citations = check_citations(&text, ctx, &mut issues);
    grounding::check_numbers(&text, ctx, &mut issues);
    grounding::check_web(&text, ctx, &mut issues);
    check_names(&text, ctx, &mut issues);
    check_markup(&text, &mut issues);
    let mut action_refs: Vec<String> = Vec::new();
    for reference in submission.action_refs {
        if !ctx.plans.contains(&reference) {
            issues.push(Issue::new(IssueKind::UnknownAction, reference));
        } else if !action_refs.contains(&reference) && action_refs.len() < MAX_ACTION_REFS {
            action_refs.push(reference);
        }
    }
    let follow_ups = submission
        .follow_ups
        .iter()
        .map(|f| f.trim())
        .filter(|f| {
            let mut probe = Vec::new();
            check_markup(f, &mut probe);
            !f.is_empty() && probe.is_empty()
        })
        .map(|f| {
            if f.chars().count() > MAX_FOLLOW_UP_CHARS {
                f.chars().take(MAX_FOLLOW_UP_CHARS).collect::<String>() + "…"
            } else {
                f.to_owned()
            }
        })
        .take(MAX_FOLLOW_UPS)
        .collect();
    let answer = Answer {
        text,
        confidence: submission.confidence,
        category: submission.category,
        citations,
        action_refs,
        follow_ups,
        unverified: Vec::new(),
        repairs: 0,
    };
    if issues.iter().all(|i| i.kind.mark_only()) {
        Ok(Answer {
            unverified: issues,
            ..answer
        })
    } else {
        let shown = Answer {
            unverified: issues
                .iter()
                .filter(|i| i.kind.in_text())
                .cloned()
                .collect(),
            ..answer
        };
        Err((issues, Some(Box::new(shown))))
    }
}
