import { SUPER_ID, ULTRA_ID } from "./models.js";

export function createUsageFixture() {
  const PICO = 10n ** 12n;

  const PRICES = {
    [SUPER_ID]: { prompt: "0.0000003", completion: "0.0000009", input_per_million: "0.30", output_per_million: "0.90" },
    [ULTRA_ID]: { prompt: "0.0000006", completion: "0.0000018", input_per_million: "0.60", output_per_million: "1.80" },
  };

  const pricesReadAt = Date.now() - 90000;

  const toPico = (usd) => {
    const [whole, fraction = ""] = usd.split(".");
    return BigInt(whole) * PICO + BigInt(fraction.padEnd(12, "0"));
  };

  const exactUsd = (pico) => {
    const fraction = (pico % PICO).toString().padStart(12, "0").replace(/0+$/, "");
    return fraction ? `${pico / PICO}.${fraction}` : `${pico / PICO}`;
  };

  function amountView(pico) {
    const step = 10n ** 8n;
    const rounded = pico / step + (pico % step >= step / 2n ? 1n : 0n);
    const shown = `${rounded / 10000n}.${(rounded % 10000n).toString().padStart(4, "0")}`;
    return { usd: shown === "0.0000" && pico !== 0n ? exactUsd(pico) : shown, exact_usd: exactUsd(pico) };
  }

  const priceView = (id) => ({ model_id: id, input_per_million: PRICES[id].input_per_million, output_per_million: PRICES[id].output_per_million, read_at_unix_ms: pricesReadAt });

  function routedUsage(requests, { toolCalls, routing }) {
    const costOf = (r) => (r.prompt == null ? null : toPico(PRICES[r.model].prompt) * BigInt(r.prompt) + toPico(PRICES[r.model].completion) * BigInt(r.completion));
    const sum = (picos) => picos.reduce((total, pico) => (total == null || pico == null ? null : total + pico), 0n);
    const tokens = (items, key) => (items.some((r) => r[key] == null) ? null : items.reduce((total, r) => total + r[key], 0));
    const known = requests.filter((r) => r.prompt != null);
    const ids = [...new Set(requests.map((r) => r.model))];
    const total = sum(requests.map(costOf));
    return {
      requests: requests.length,
      tool_calls: toolCalls,
      extensions: 0,
      prompt_tokens: tokens(requests, "prompt"),
      completion_tokens: tokens(requests, "completion"),
      reasoning_tokens: tokens(requests, "reasoning"),
      cost: ids.length === 1 && total != null ? { ...amountView(total), price: priceView(ids[0]) } : null,
      model_requests: requests.map((r, i) => ({
        request: i + 1,
        model_id: r.model,
        usage: { prompt_tokens: r.prompt ?? null, completion_tokens: r.completion ?? null, reasoning_tokens: r.reasoning ?? null },
        cost: r.prompt == null ? null : { usd: exactUsd(costOf(r)), prompt_tokens: r.prompt, completion_tokens: r.completion, pricing: { model_id: r.model, prompt_usd_per_token: PRICES[r.model].prompt, completion_usd_per_token: PRICES[r.model].completion, source: "synthetic mock", read_at_unix_ms: pricesReadAt } },
      })),
      total_cost_usd: total == null ? null : exactUsd(total),
      routing,
      models: ids.map((id) => {
        const own = requests.filter((r) => r.model === id);
        const cost = sum(own.map(costOf));
        const priced = own.filter((r) => r.prompt != null).length;
        return { model_id: id, requests: own.length, prompt_tokens: tokens(own, "prompt"), completion_tokens: tokens(own, "completion"), reasoning_tokens: tokens(own, "reasoning"), unpriced_requests: own.length - priced, cost: cost == null ? null : amountView(cost), price: priced ? priceView(id) : null };
      }),
      total_cost: total == null ? null : amountView(total),
      priced_cost_lower_bound: total == null && known.length ? amountView(sum(known.map(costOf))) : null,
      known_prompt_tokens: known.reduce((n, r) => n + r.prompt, 0),
      known_completion_tokens: known.reduce((n, r) => n + r.completion, 0),
      unknown_requests: requests.length - known.length,
      cost_lower_bound: ids.length === 1 && total == null && known.length ? { ...amountView(sum(known.map(costOf))), price: priceView(ids[0]) } : null,
    };
  }
  return routedUsage;
}
