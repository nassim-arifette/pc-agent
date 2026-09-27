//! Estimated inference cost: measured token usage times catalog prices. Only a verified, dated
//! price is used (spec 13); without one, or without complete usage, only the usage is reported.
use serde::Serialize;

/// Prices of one model as published in the provider catalog, and when they were read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pricing {
    pub model_id: String,
    /// US dollars per input token, the exact decimal string the catalog published.
    pub prompt_usd_per_token: String,
    /// US dollars per output token (reasoning tokens are output tokens), as published.
    pub completion_usd_per_token: String,
    pub source: String,
    pub read_at_unix_ms: u64,
}
impl Pricing {
    /// Picodollars per input and per output token; `None` unless both prices parse exactly.
    pub fn per_token_pico(&self) -> Option<(u128, u128)> {
        Some((
            parse_usd_pico(&self.prompt_usd_per_token)?,
            parse_usd_pico(&self.completion_usd_per_token)?,
        ))
    }
    /// Input and output prices in US dollars per million tokens, exact, at least two decimals.
    pub fn per_million_usd(&self) -> Option<(String, String)> {
        let (prompt, completion) = self.per_token_pico()?;
        let show = |pico: u128| {
            let exact = format_pico(pico.checked_mul(1_000_000)?);
            Some(match exact.split_once('.') {
                Some((_, frac)) if frac.len() >= 2 => exact,
                Some((int, frac)) => format!("{int}.{frac:0<2}"),
                None => format!("{exact}.00"),
            })
        };
        Some((show(prompt)?, show(completion)?))
    }
}

/// Measured usage priced at the catalog rates. An estimate, not an invoice: the provider's
/// billing (rounding, discounts, minimums) is not visible here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cost {
    /// Exact product in US dollars, as a plain decimal ("0.0223698").
    pub usd: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub pricing: Pricing,
    #[serde(skip)]
    pico: u128,
}
impl Cost {
    /// The amount rounded half-up to `decimals` places, for display ("0.0224").
    pub fn rounded_usd(&self, decimals: u32) -> String {
        round_pico(self.pico, decimals)
    }
}

const PICO_PER_USD: u128 = 1_000_000_000_000;
const PICO_DIGITS: usize = 12;

/// Parses a plain non-negative decimal ("0.0000003", "2") into picodollars. Signs, exponents,
/// separators, more than 12 decimals or overflow are rejected: such a price is not verified.
pub fn parse_usd_pico(text: &str) -> Option<u128> {
    let (int, frac) = match text.split_once('.') {
        Some((int, frac)) if !frac.is_empty() => (int, frac),
        Some(_) => return None,
        None => (text, ""),
    };
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if int.is_empty() || !digits(int) || !digits(frac) || frac.len() > PICO_DIGITS {
        return None;
    }
    let whole = int.parse::<u128>().ok()?.checked_mul(PICO_PER_USD)?;
    let scale = 10u128.pow(u32::try_from(PICO_DIGITS - frac.len()).ok()?);
    let part = if frac.is_empty() {
        0
    } else {
        frac.parse::<u128>().ok()?.checked_mul(scale)?
    };
    whole.checked_add(part)
}

/// `None` when a price does not parse or the product overflows.
pub fn estimate(prompt_tokens: u64, completion_tokens: u64, pricing: &Pricing) -> Option<Cost> {
    let (prompt, completion) = pricing.per_token_pico()?;
    let pico = u128::from(prompt_tokens)
        .checked_mul(prompt)?
        .checked_add(u128::from(completion_tokens).checked_mul(completion)?)?;
    Some(Cost {
        usd: format_pico(pico),
        prompt_tokens,
        completion_tokens,
        pricing: pricing.clone(),
        pico,
    })
}

/// Picodollars as an exact plain decimal without trailing zeros.
pub fn format_pico(pico: u128) -> String {
    let (int, frac) = (pico / PICO_PER_USD, pico % PICO_PER_USD);
    if frac == 0 {
        return int.to_string();
    }
    let digits = format!("{frac:012}");
    format!("{int}.{}", digits.trim_end_matches('0'))
}

/// Rounds half-up to `decimals` (at most 12) places and always prints that many decimals.
pub fn round_pico(pico: u128, decimals: u32) -> String {
    let decimals = decimals.min(12);
    let step = 10u128.pow(12 - decimals);
    let rounded = (pico + step / 2) / step;
    let unit = 10u128.pow(decimals);
    if decimals == 0 {
        return rounded.to_string();
    }
    format!(
        "{}.{:0width$}",
        rounded / unit,
        rounded % unit,
        width = decimals as usize
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pricing(prompt: &str, completion: &str) -> Pricing {
        Pricing {
            model_id: "nvidia/nemotron-fixture".into(),
            prompt_usd_per_token: prompt.into(),
            completion_usd_per_token: completion.into(),
            source: "fixture".into(),
            read_at_unix_ms: 0,
        }
    }
    #[test]
    fn published_decimal_prices_parse_exactly() {
        assert_eq!(parse_usd_pico("0.0000003"), Some(300_000));
        assert_eq!(parse_usd_pico("0.000000658"), Some(658_000));
        assert_eq!(parse_usd_pico("2"), Some(2 * PICO_PER_USD));
        assert_eq!(parse_usd_pico("0.000000000001"), Some(1));
        for bad in [
            "",
            ".3",
            "3.",
            "-0.1",
            "+0.1",
            "3e-7",
            "0,3",
            " 0.3",
            "0.0000000000001",
            "NaN",
            "340282366920938463463374607431768211456",
        ] {
            assert_eq!(parse_usd_pico(bad), None, "{bad:?}");
        }
    }
    #[test]
    fn cost_is_the_exact_product_of_tokens_and_prices() {
        // 74,566 input and 876 output tokens at $0.30 / $0.90 per million tokens.
        let cost = estimate(74_566, 876, &pricing("0.0000003", "0.0000009")).unwrap();
        assert_eq!(cost.usd, "0.0231582");
        assert_eq!(cost.rounded_usd(4), "0.0232");
        assert_eq!(
            estimate(0, 0, &pricing("0.0000003", "0.0000009"))
                .unwrap()
                .usd,
            "0"
        );
        assert!(estimate(1, 1, &pricing("3e-7", "0.0000009")).is_none());
        assert!(estimate(u64::MAX, u64::MAX, &pricing("340282366", "1")).is_none());
    }
    #[test]
    fn prices_per_million_keep_their_precision() {
        let show = |p: &str, c: &str| pricing(p, c).per_million_usd().unwrap();
        assert_eq!(
            show("0.0000003", "0.0000009"),
            ("0.30".into(), "0.90".into())
        );
        assert_eq!(
            show("0.000000658", "0.000001"),
            ("0.658".into(), "1.00".into())
        );
        assert_eq!(
            show("0.00000006", "0.00000024"),
            ("0.06".into(), "0.24".into())
        );
        assert!(pricing("x", "1").per_million_usd().is_none());
    }
    #[test]
    fn rounding_is_half_up_with_fixed_decimals() {
        assert_eq!(round_pico(22_369_800_000, 4), "0.0224");
        assert_eq!(round_pico(50_000_000, 4), "0.0001");
        assert_eq!(round_pico(49_999_999, 4), "0.0000");
        assert_eq!(round_pico(300_000 * 1_000_000, 2), "0.30");
        assert_eq!(round_pico(PICO_PER_USD * 3, 0), "3");
        assert_eq!(format_pico(22_369_800_000), "0.0223698");
        assert_eq!(format_pico(PICO_PER_USD), "1");
    }
}
