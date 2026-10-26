use super::*;

#[test]
fn cost_needs_complete_usage_and_a_verified_catalog_price() {
    let pricing = fixture_pricing();
    let initial = fixture();
    let next = usable_snapshot();
    let id = analyze(&next)[0].id.clone();
    let run = |replies: Vec<Reply>, pricing: Option<Pricing>| {
        let mut m = script(replies);
        m.pricing = pricing;
        investigate(&mut m, "test", &initial, &mut tb(|| next.clone()), |_| {})
    };
    let snapshot_call = || tools(vec![call("a", "get_system_snapshot", "{}")]);
    let chosen = alias_after(&initial, 0);
    let r = run(vec![snapshot_call(), done(&chosen)], Some(pricing.clone()));
    assert_eq!(r.selected.id, id);
    assert_eq!(r.stats.prompt_tokens, Some(18));
    assert_eq!(r.stats.completion_tokens, Some(2));
    assert_eq!(r.stats.requests.len(), 2);
    // 18 input tokens at $0.30 and 2 output tokens at $0.90 per million.
    assert_eq!(
        r.stats.cost.as_ref().map(|c| c.usd.as_str()),
        Some("0.0000072")
    );
    assert_eq!(r.stats.cost.unwrap().pricing, pricing);

    assert!(run(vec![snapshot_call(), done(&chosen)], None)
        .stats
        .cost
        .is_none());
    let unreported = Reply {
        calls: vec![call(
            "answer",
            SUBMIT_ANSWER,
            &answer_args(&format!("See [{chosen}].")),
        )],
        ..Reply::default()
    };
    let r = run(vec![snapshot_call(), unreported], Some(pricing.clone()));
    assert_eq!(r.stats.prompt_tokens, None);
    assert!(r.stats.cost.is_none());

    // A reply cut off at the output limit is billed: it ends the investigation, but its
    // tokens are counted and priced.
    let cut_off = Reply {
        prompt_tokens: Some(9),
        completion_tokens: Some(u64::from(MAX_OUTPUT_TOKENS)),
        rejected: Some(AgentError::OutputLimit),
        ..Reply::default()
    };
    // A reply cut off at the output limit is billed: its tokens are counted and priced, and the
    // request is asked again a bounded number of times.
    let r = run(
        vec![cut_off.clone(), snapshot_call(), done(&chosen)],
        Some(pricing.clone()),
    );
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(r.stats.inference_calls, 3);
    assert_eq!(
        r.stats.completion_tokens,
        Some(u64::from(MAX_OUTPUT_TOKENS) + 2)
    );
    // A second cut-off in a row ends the turn.
    let r = run(vec![cut_off; MAX_REPAIRS + 1], Some(pricing));
    assert_eq!(r.status, Status::Partial);
    assert_eq!(r.error, Some(AgentError::OutputLimit));
    assert_eq!(r.stats.inference_calls, 2);
    assert_eq!(r.stats.tool_calls, 0);
    assert_eq!(
        r.stats.completion_tokens,
        Some(u64::from(MAX_OUTPUT_TOKENS) * 2)
    );
    assert!(r.stats.cost.is_some());
}

#[test]
fn a_cost_lower_bound_needs_a_verified_price_for_every_counted_request() {
    let price = fixture_pricing();
    for earlier_model in [price.model_id.as_str(), "nvidia/unpriced-fixture"] {
        let mut stats = Stats::none();
        stats.record(&Reply {
            prompt_tokens: Some(100),
            completion_tokens: Some(10),
            ..Reply::default()
        });
        stats.price_for(None, Some(earlier_model), 0);
        stats.record_failure();
        stats.price_for(Some(price.clone()), Some(&price.model_id), 0);
        assert_eq!(stats.known_prompt_tokens, 100);
        assert_eq!(stats.known_completion_tokens, 10);
        assert_eq!(stats.unknown_requests, 1);
        assert!(stats.cost.is_none());
        assert!(
            stats.cost_lower_bound.is_none(),
            "an unpriced request must not inherit a later request's price"
        );
    }
}

#[test]
fn a_cost_lower_bound_uses_only_one_verified_price_and_complete_known_usage() {
    let price = fixture_pricing();
    let mut stats = Stats::none();
    stats.record(&Reply {
        prompt_tokens: Some(100),
        completion_tokens: Some(10),
        ..Reply::default()
    });
    stats.price_for(Some(price.clone()), Some(&price.model_id), 0);
    stats.record_failure();
    stats.price_for(Some(price.clone()), Some(&price.model_id), 0);
    assert!(stats.cost.is_none());
    assert!(stats.total_cost_usd.is_none());
    let bound = stats.cost_lower_bound.as_ref().unwrap();
    assert_eq!((bound.prompt_tokens, bound.completion_tokens), (100, 10));
    assert_eq!(bound.usd, "0.000039");

    let mut other = price.clone();
    other.model_id = "nvidia/other-fixture".into();
    other.prompt_usd_per_token = "0.000003".into();
    stats.record(&Reply {
        prompt_tokens: Some(100),
        completion_tokens: Some(10),
        ..Reply::default()
    });
    stats.price_for(Some(other.clone()), Some(&other.model_id), 0);
    assert_eq!(stats.known_prompt_tokens, 200);
    assert!(stats.cost_lower_bound.is_none());
}
