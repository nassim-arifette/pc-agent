// Synthetic findings and written answers, never measurements of the current PC.
import { language } from "../../ui/js/i18n.js";
import { DICTIONARIES } from "../../ui/js/phrases.js";

const q = (unit, value) => ({ unit, value });
const p = (key, values = {}) => ({ key, values });
const GiB = 1024 ** 3;
const availableFact = p("fact.available_of_total", {
  available: q("bytes", Math.round(1.54 * GiB)),
  total: q("bytes", Math.round(15.73 * GiB)),
  share: q("basis_points", 980),
});
const cpuFact = p("fact.cpu_busy", { share: q("basis_points", 1850) });
const faultFact = p("fact.hard_faults", { rate: q("pages_per_second_x100", 1240) });
const processFact = p("observation.metric_value", {
  metric: q("metric", "process_working_set_bytes"),
  value: q("bytes", Math.round(2.31 * GiB)),
  entity: q("process", { name: "Google Chrome", pid: 4000 }),
});

function finding(classification, label, support, summary, extra = {}) {
  return {
    classification,
    label,
    support,
    support_reason: "Derived from one short sample.",
    summary,
    facts: [],
    hypothesis: null,
    counter_evidence: [],
    missing: [],
    next_step: "",
    limitations: [],
    summary_phrase: p(`summary.${classification}`),
    support_reason_phrase: p("support.short_sample"),
    ...extra,
  };
}

export const DRIVER_QUERY = "NVIDIA GeForce RTX 4060 Laptop GPU driver 32.0.15.6094 release notes known issues";

function webCitations() {
  const now = Date.now();
  const web = (domain, path, publishedDate) => ({ domain, url: `https://www.${domain}/${path}`, published_date: publishedDate, searched_at_unix_ms: now - 6000, query: DRIVER_QUERY });
  return [
    { reference: "O3", kind: "observation", title: "get_drivers", facts: ["NVIDIA GeForce RTX 4060 Laptop GPU: driver 32.0.15.6094 (NVIDIA 560.94), dated 2024-08-14."], earlier: false, measured_at_unix_ms: now - 7000 },
    {
      reference: "W1",
      kind: "web",
      title: "GeForce Game Ready Driver 566.36 | Release Notes",
      facts: ["Fixed issues in version 566.36: higher than expected memory usage in some web browsers when hardware acceleration is enabled. Open issues: none related to memory."],
      earlier: false,
      measured_at_unix_ms: null,
      web: web("nvidia.com", "synthetic/566-36-release-notes", "2024-12-05"),
    },
    {
      reference: "W2",
      kind: "web",
      title: "Known issues with GeForce drivers and web browsers",
      facts: ["Some browsers may use more memory than expected with hardware acceleration on drivers older than 566.36. Updating the driver or turning hardware acceleration off avoids it."],
      earlier: false,
      measured_at_unix_ms: null,
      web: web("nvidia.com", "synthetic/known-issues", null),
    },
  ];
}

export const PRICE_QUERY = "DDR5 SO-DIMM 5600 MT/s 16 GB price";
export const RAM_QUESTION = /more ram|plus de ram|upgrade|mémoire en plus|memory upgrade/i;

// A synthetic upgrade answer: the module the PC takes, measured, then a dated web price.
function upgradeAnswer() {
  const french = language() === "fr";
  const now = Date.now();
  const text = french
    ? "Oui, un ajout de mémoire aiderait : votre PC n'avait plus que 1,54 Gio de libre sur 15,73 Gio [O1]. Il utilise de la DDR5 SO-DIMM à 5600 MT/s, deux barrettes de 8 Gio, et accepte jusqu'à 64 Gio [O4].\n\nLe plus simple : remplacer les deux barrettes par 2 × 16 Gio du même type. Une barrette de 16 Gio DDR5-5600 SO-DIMM se trouve aujourd'hui entre 39,90 € et 54,90 € [W3]. C'est un prix relevé sur le web aujourd'hui, pas une mesure : vérifiez-le avant d'acheter.\n\nAvant de dépenser quoi que ce soit, fermer les onglets de **Google Chrome** inutiles libère déjà 2,31 Gio [O1]."
    : "Yes, more memory would help: only 1.54 GiB of 15.73 GiB was free [O1]. Your PC takes DDR5 SO-DIMM at 5600 MT/s, has two 8 GiB modules, and supports up to 64 GiB [O4].\n\nThe simple upgrade: replace both with 2 × 16 GiB of the same type. A 16 GiB DDR5-5600 SO-DIMM module costs between €39.90 and €54.90 today [W3]. That price comes from the web today, not a measurement: check it before you buy.\n\nBefore spending anything, closing the **Google Chrome** tabs you don't need already frees 2.31 GiB [O1].";
  return {
    text,
    confidence: "medium",
    category: "memory",
    citations: [
      { reference: "O1", kind: "observation", title: "get_system_snapshot",
        facts: ["Available physical memory: 1.54 GiB of 15.73 GiB (9.8%).", "Google Chrome used 2.31 GiB of RAM (working set)."],
        fact_phrases: [availableFact, processFact] },
      { reference: "O4", kind: "observation", title: "get_hardware_inventory",
        facts: ["Memory: DDR5 SO-DIMM, 5600 MT/s, 2 modules of 8 GiB, 2 of 2 slots used, up to 64 GiB supported (firmware)."],
        earlier: false, measured_at_unix_ms: now - 5000 },
      { reference: "W3", kind: "web", title: "16 GB DDR5-5600 SO-DIMM laptop memory: price comparison",
        facts: ["16 GB DDR5-5600 SO-DIMM modules listed from €39.90 to €54.90 across 9 shops."],
        earlier: false, measured_at_unix_ms: null,
        web: { domain: "prices.example", url: "https://prices.example/synthetic/ddr5-5600-sodimm-16gb", published_date: null, searched_at_unix_ms: now - 4000, query: PRICE_QUERY } },
    ],
    follow_ups: french ? ["Comment installer la mémoire ?", "Quelles applications utilisent le plus de mémoire ?"] : ["How do I install the memory?", "Which apps use the most memory?"],
    unverified: [],
    repairs: 0,
  };
}

function mockAnswer(question, web = false) {
  if (web && RAM_QUESTION.test(question)) return upgradeAnswer();
  const french = language() === "fr";
  const lead = french
    ? "\n\nUne piste en plus : votre pilote graphique est en version 32.0.15.6094 [O3], et les notes de NVIDIA pour la version 566.36 corrigent une consommation de mémoire trop élevée dans les navigateurs avec l'accélération matérielle [W1] ; NVIDIA la décrit aussi comme un problème connu [W2]. C'est un indice venu du web, pas une mesure : mettez à jour le pilote seulement si le ralentissement continue après avoir fermé des onglets."
    : "\n\nOne more lead: your graphics driver is 32.0.15.6094 [O3], and NVIDIA's notes for version 566.36 fix higher than expected memory use in browsers with hardware acceleration [W1]; NVIDIA also lists it as a known issue [W2]. This is a hint from the web, not a measurement: update the driver only if the slowdown continues after closing tabs.";
  const text = (french
    ? "Votre PC rame surtout parce que la mémoire vive est presque pleine : il ne restait que 1,54 Gio de libre sur 15,73 Gio [O1], et Windows relisait 12,4 pages par seconde depuis le disque pour compenser [F2]. J'en suis moyennement sûr : c'est une mesure d'une seconde.\n\nCe que vous pouvez faire :\n1. Fermez les onglets de **Google Chrome** dont vous n'avez pas besoin : il occupe 2,31 Gio [O1].\n2. Gardez PC Agent ouvert pendant le prochain ralentissement et reposez la question, pour confirmer.\n\nLe processeur n'était pas saturé (18,5 % [O1]) et le disque répondait en moins de 3 ms [O1] : ils ne semblent pas en cause."
    : "Your PC is slow mainly because memory is almost full: only 1.54 GiB of 15.73 GiB was available [O1], and Windows read 12.4 pages per second back from disk to compensate [F2]. I'm moderately sure: this is a one-second measurement.\n\nWhat you can do:\n1. Close the **Google Chrome** tabs you don't need: it holds 2.31 GiB [O1].\n2. Keep PC Agent open during the next slowdown and ask again to confirm.\n\nThe processor was not saturated (18.5% [O1]) and the disk answered in under 3 ms [O1], so they don't look involved.") + (web ? lead : "");
  return {
    text,
    confidence: "medium",
    category: "memory",
    citations: [
      { reference: "O1", kind: "observation", title: "get_system_snapshot",
        facts: ["Available physical memory: 1.54 GiB of 15.73 GiB (9.8%).", "Google Chrome used 2.31 GiB of RAM (working set).", "CPU busy: 18.5%.", "Disk read response time: 1.3 ms."],
        fact_phrases: [availableFact, processFact, cpuFact, p("fact.disk_read_latency", { latency: q("microseconds", 1300) })] },
      { reference: "F2", kind: "finding", classification: "hard_fault_pressure", title: "Heavy hard-fault paging",
        facts: ["12.4 pages per second were read from disk to resolve hard faults."], fact_phrases: [faultFact] },
      ...(web ? webCitations() : []),
    ],
    follow_ups: french
      ? ["Quels onglets ou extensions utilisent le plus de mémoire ?", "Ai-je besoin de plus de RAM ?"]
      : ["Which tabs or extensions use the most memory?", "Do I need more RAM?"],
    unverified: [],
    repairs: 0,
  };
}

export function report(question, cloudRun, error = null, { web = false, toolEvents = [] } = {}) {
  const selected = error
    ? finding("insufficient_evidence", "Insufficient evidence", "insufficient", "The available measurements do not support a conclusion yet.", {
        missing: ["An observation taken while the slowdown happens."],
        next_step: "Run the investigation again while the problem is happening.",
        limitations: ["No cause is excluded because some resources were not measured."],
        next_step_phrase: p("next.observe_longer"),
        limitation_phrases: [p("support.abstention")],
      })
    : finding("low_available_memory", "Low available memory", "plausible", "Available physical memory was 9.8% of 15.73 GiB during the one-second sample, and Google Chrome was the largest contributor.", {
        facts: [
          "Available physical memory: 1.54 GiB of 15.73 GiB (9.8%).",
          "System commit: 18.20 GiB of its 24.00 GiB limit (75.8%).",
          "Google Chrome used 2.31 GiB of RAM (working set).",
          "Hard-fault page reads: 12.4 pages/s.",
        ],
        fact_phrases: [
          availableFact,
          p("fact.commit_of_limit", { used: q("bytes", Math.round(18.2 * GiB)), limit: q("bytes", 24 * GiB), share: q("basis_points", 7580) }),
          processFact, faultFact,
        ],
        hypothesis: "Low available memory can make Windows page memory to disk, which slows programs down.",
        counter_evidence: ["The processor was 18.5% busy: it was not saturated.", "Disk response times stayed below 3 ms."],
        missing: ["Memory use over several minutes while the slowdown happens.", "Which browser tabs or extensions use the most memory."],
        next_step: "Keep PC Agent open while the slowdown happens and ask again, or close unused browser tabs and compare.",
        next_step_phrase: p("next.memory_contributors"),
        limitations: [DICTIONARIES.en["note.short_sample"], DICTIONARIES.en["note.working_sets"]],
        limitation_phrases: [p("note.short_sample"), p("note.working_sets")],
      });
  return {
    question,
    mode: cloudRun ? "nebius" : "local_rules",
    execution: error ? (error === "cancelled" ? "failed" : "failed") : "completed",
    coverage: {
      status: "partial",
      domains: [
        { domain: "system", target: null, status: "complete", missing: [] },
        { domain: "processes", target: null, status: "partial", missing: ["17 processes denied"] },
        { domain: "hardware", target: null, status: "not_collected", missing: [] },
        { domain: "storage", target: null, status: "not_collected", missing: [] },
        { domain: "history", target: "cpu_busy_basis_points", status: cloudRun ? "complete" : "not_collected", missing: [] },
      ],
    },
    support: selected.support,
    error: error ? { code: error, message: error } : null,
    selected,
    alternatives: error
      ? []
      : [
          finding("largest_memory_contributor", "Largest memory contributor", "supported", "Google Chrome held 2.31 GiB of RAM, the most of any readable program."),
          finding("hard_fault_pressure", "Heavy hard-fault paging", "plausible", "12.4 pages per second were read from disk to resolve hard faults."),
        ],
    answer: cloudRun && !error ? mockAnswer(question, web) : null,
    observations: cloudRun && !error
      ? [{ tool: "get_metric_history", status: "ok", facts: ["CPU busy: 12 usable samples, observed range 9.5% to 31.0%.", "Observed interval: 60000 ms."], limitations: [] }]
      : [],
    tool_events: toolEvents,
    limitations: [DICTIONARIES.en["note.short_sample"]],
    limitation_phrases: [p("note.short_sample")],
    // view::admin_hints: the checks a turn lacked, when the model met needs_admin.
    admin_hints: cloudRun && !error && /health|santé/i.test(question) ? ["disk_reliability", "battery_energy"] : [],
    elevated_proposals: cloudRun && !error && /administrator|administrateur|deep|approfondie/i.test(question)
      ? [{ plan_id: "plan-mock-deep", tasks: ["system_caches", "shadow_storage", "disk_reliability", "mft_inventory", "process_images"] }] : [],
    usage: cloudRun
      ? {
          requests: error === "output_limit" ? 1 : 3,
          tool_calls: toolEvents.length || 2,
          extensions: 0,
          prompt_tokens: 13596,
          completion_tokens: 354,
          reasoning_tokens: 270,
          cost: { usd: "0.0044", exact_usd: "0.0044106", price: { model_id: "nvidia/nemotron-3-super-120b-a12b", input_per_million: "0.30", output_per_million: "0.90", read_at_unix_ms: Date.now() - 90000 } },
        }
      : null,
    continuation: null,
  };
}
