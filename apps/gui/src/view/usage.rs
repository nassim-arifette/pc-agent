//! Exact per-model costs and token-usage display values.
use pcagent_agent::{Cost, ModelRequestUsage, Pricing, RequestUsage, Stats};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PriceView {
    pub model_id: String,
    pub input_per_million: Option<String>,
    pub output_per_million: Option<String>,
    pub read_at_unix_ms: u64,
}
#[derive(Debug, Clone, Serialize)]
pub struct CostView {
    /// Rounded to four decimals, or exact when that would show zero.
    pub usd: String,
    pub exact_usd: String,
    pub price: PriceView,
}
/// An exact sum of request costs, which may span several prices, and its display form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AmountView {
    /// Rounded to four decimals, or exact when that would show zero, like [`CostView::usd`].
    pub usd: String,
    pub exact_usd: String,
}
/// One model's share of a turn (C3 complement of 4 October): after an escalation, Super's and
/// Ultra's requests, tokens and costs stay apart. A sum is given only when all of it is known.
#[derive(Debug, Clone, Serialize)]
pub struct ModelUsageView {
    pub model_id: Option<String>,
    pub requests: usize,
    /// Sums over this model's requests; unknown as soon as one of them did not report the count.
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    /// This model's requests without a cost: no reported usage, or no verified price.
    pub unpriced_requests: usize,
    /// The exact sum of this model's request costs; absent unless every one of them is priced.
    pub cost: Option<AmountView>,
    /// The verified, dated price of its priced requests, when they all used the same one.
    pub price: Option<PriceView>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    pub requests: usize,
    pub tool_calls: usize,
    pub extensions: usize,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cost: Option<CostView>,
    pub model_requests: Vec<pcagent_agent::ModelRequestUsage>,
    pub total_cost_usd: Option<String>,
    pub routing: Option<pcagent_agent::routing::RoutingState>,
    /// `model_requests` grouped by model, in the order the models were first used.
    pub models: Vec<ModelUsageView>,
    /// `total_cost_usd` for display: absent exactly when it is, so never one model's price
    /// applied to another model's tokens.
    pub total_cost: Option<AmountView>,
    /// Without a total, the sum of the requests that have a cost, each at its own model's
    /// price: the turn cost at least this much. Never shown as a total.
    pub priced_cost_lower_bound: Option<AmountView>,
    /// Lower bounds when a request reported no usage (see `Stats`).
    pub known_prompt_tokens: u64,
    pub known_completion_tokens: u64,
    pub unknown_requests: usize,
    pub cost_lower_bound: Option<CostView>,
}
pub fn price(p: &Pricing) -> PriceView {
    let rates = p.per_million_usd();
    PriceView {
        model_id: p.model_id.clone(),
        input_per_million: rates.as_ref().map(|(input, _)| input.clone()),
        output_per_million: rates.map(|(_, output)| output),
        read_at_unix_ms: p.read_at_unix_ms,
    }
}
/// What the window shows of an exact amount: four decimals rounded half-up, or the exact amount
/// when that would read zero.
pub(super) fn shown_usd(rounded: String, exact: &str) -> String {
    if rounded == "0.0000" && exact != "0" {
        exact.to_owned()
    } else {
        rounded
    }
}
pub(super) fn cost(c: &Cost) -> CostView {
    CostView {
        usd: shown_usd(c.rounded_usd(4), &c.usd),
        exact_usd: c.usd.clone(),
        price: price(&c.pricing),
    }
}
pub(super) const PICO_PER_USD: u128 = 1_000_000_000_000;
/// Picodollars of an exact amount from the core ("0.0000432"); `None` for anything else.
pub(super) fn pico(usd: &str) -> Option<u128> {
    let (whole, fraction) = usd.split_once('.').unwrap_or((usd, ""));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || (usd.contains('.') && !digits(fraction)) || fraction.len() > 12 {
        return None;
    }
    let fraction = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<12}").parse::<u128>().ok()?
    };
    whole
        .parse::<u128>()
        .ok()?
        .checked_mul(PICO_PER_USD)?
        .checked_add(fraction)
}
/// The exact amount and its display form, rounded half-up to four decimals like a single cost.
pub(super) fn amount(pico: u128) -> AmountView {
    let (whole, fraction) = (pico / PICO_PER_USD, pico % PICO_PER_USD);
    let exact_usd = if fraction == 0 {
        whole.to_string()
    } else {
        format!(
            "{whole}.{}",
            format!("{fraction:012}").trim_end_matches('0')
        )
    };
    // Picodollars per unit of the fourth decimal.
    const STEP: u128 = 100_000_000;
    let rounded = pico / STEP + u128::from(pico % STEP >= STEP / 2);
    AmountView {
        usd: shown_usd(
            format!("{}.{:04}", rounded / 10_000, rounded % 10_000),
            &exact_usd,
        ),
        exact_usd,
    }
}
/// The exact sum of these costs, each at its own price; `None` if one does not parse.
pub(super) fn sum_costs<'a>(costs: impl IntoIterator<Item = &'a Cost>) -> Option<u128> {
    costs
        .into_iter()
        .try_fold(0u128, |sum, c| sum.checked_add(pico(&c.usd)?))
}
/// Requests grouped by model, in the order the models were first used.
pub(super) fn models(requests: &[ModelRequestUsage]) -> Vec<ModelUsageView> {
    let mut groups: Vec<(Option<&str>, Vec<&ModelRequestUsage>)> = Vec::new();
    for request in requests {
        let id = request.model_id.as_deref();
        match groups.iter_mut().find(|(model, _)| *model == id) {
            Some((_, group)) => group.push(request),
            None => groups.push((id, vec![request])),
        }
    }
    groups
        .into_iter()
        .map(|(model_id, group)| {
            let tokens = |count: fn(&RequestUsage) -> Option<u64>| {
                group
                    .iter()
                    .try_fold(0u64, |sum, r| sum.checked_add(count(&r.usage)?))
            };
            let priced: Vec<&Cost> = group.iter().filter_map(|r| r.cost.as_ref()).collect();
            ModelUsageView {
                model_id: model_id.map(str::to_owned),
                requests: group.len(),
                prompt_tokens: tokens(|u| u.prompt_tokens),
                completion_tokens: tokens(|u| u.completion_tokens),
                reasoning_tokens: tokens(|u| u.reasoning_tokens),
                unpriced_requests: group.len() - priced.len(),
                cost: (priced.len() == group.len())
                    .then(|| sum_costs(priced.iter().copied()))
                    .flatten()
                    .map(amount),
                price: priced
                    .first()
                    .filter(|first| priced.iter().all(|c| c.pricing == first.pricing))
                    .map(|c| price(&c.pricing)),
            }
        })
        .collect()
}
pub fn usage(s: &Stats) -> Usage {
    let priced: Vec<&Cost> = s
        .model_requests
        .iter()
        .filter_map(|r| r.cost.as_ref())
        .collect();
    Usage {
        requests: s.inference_calls,
        tool_calls: s.tool_calls,
        extensions: s.extensions,
        prompt_tokens: s.prompt_tokens,
        completion_tokens: s.completion_tokens,
        reasoning_tokens: s.reasoning_tokens,
        cost: s.cost.as_ref().map(cost),
        model_requests: s.model_requests.clone(),
        total_cost_usd: s.total_cost_usd.clone(),
        routing: s.routing.clone(),
        models: models(&s.model_requests),
        total_cost: s.total_cost_usd.as_deref().and_then(pico).map(amount),
        priced_cost_lower_bound: (s.total_cost_usd.is_none() && !priced.is_empty())
            .then(|| sum_costs(priced.iter().copied()))
            .flatten()
            .map(amount),
        known_prompt_tokens: s.known_prompt_tokens,
        known_completion_tokens: s.known_completion_tokens,
        unknown_requests: s.unknown_requests,
        cost_lower_bound: s.cost_lower_bound.as_ref().map(cost),
    }
}
