//! Tavily list-price estimate, kept distinct from catalog-priced model inference and invoices.
use crate::cost::{format_pico, parse_usd_pico};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize)]
pub struct WebUsage {
    pub lookups: usize,
    pub cache_hits: usize,
    pub known_credits: f64,
    pub unknown_requests: usize,
}

impl WebUsage {
    pub fn record(&mut self, usage: &Value) {
        if !usage.is_object() {
            return;
        }
        self.lookups += 1;
        if usage["cache_hit"] == true {
            self.cache_hits += 1;
            return;
        }
        match usage["credits"]
            .as_f64()
            .filter(|v| v.is_finite() && *v >= 0.0)
        {
            Some(credits) if (self.known_credits + credits).is_finite() => {
                self.known_credits += credits;
            }
            _ => self.unknown_requests += 1,
        }
    }

    /// $0.008 per credit, outside free allowance/discounts. Never a statement of account billing.
    pub fn estimated_usd(&self) -> Option<String> {
        if self.unknown_requests > 0 {
            return None;
        }
        parse_usd_pico(&self.known_credits.to_string())?
            .checked_mul(8)
            .map(|pico| format_pico(pico / 1000))
    }

    pub fn with_model_usd(&self, model: Option<&str>) -> Option<String> {
        let model = parse_usd_pico(model?)?;
        let web = parse_usd_pico(&self.estimated_usd()?)?;
        model.checked_add(web).map(format_pico)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn cache_is_free_missing_usage_is_not_zero_and_models_remain_separate() {
        let mut usage = WebUsage::default();
        usage.record(&json!({"credits": 1.0, "cache_hit": false}));
        usage.record(&json!({"credits": 2.5, "cache_hit": false}));
        usage.record(&json!({"credits": 1.0, "cache_hit": true}));
        assert_eq!(usage.estimated_usd().as_deref(), Some("0.028"));
        assert_eq!(usage.with_model_usd(Some("0.012")).as_deref(), Some("0.04"));
        assert!(usage.with_model_usd(None).is_none());
        assert_eq!(usage.cache_hits, 1);
        usage.record(&json!({"credits": null, "cache_hit": false}));
        assert!(usage.estimated_usd().is_none());
        assert_eq!(usage.known_credits, 3.5);
    }
}
