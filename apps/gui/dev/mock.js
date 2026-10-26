// Design-time stand-in for the Rust side, used when ui/index.html is opened in a plain browser
// (python -m http.server in apps/gui, then /ui/index.html). Every value is synthetic. The app
// never loads this file: it is outside the embedded ui folder.
//
// URL switches: ?cloud=none|ready|problem, ?connection=key|network|timeout|model|limit|empty,
// ?gpu=none, ?disk=ok, ?privacy=complete|anonymized|local
// (a confirmed choice; without it the first cloud question asks for one), ?elevation=fail|slow
// (fail: Windows' administrator prompt is declined), ?recording=short (3-second recordings),
// ?battery=none|ac|charging|low|relative|two|unavailable
// (the Live PC battery read: by default one discharging laptop battery; none is a desktop PC, also
// without a battery in recordings), ?sources=short (selection sources expire after 5 s instead of
// five minutes, to exercise a scan page read again on Explain or Investigate). Questions containing
// "extend" ask for a budget extension; "fail" makes the model's answer unusable; "slow"
// stretches the steps; "space" or "clean" propose cleanups and "startup" a start-up entry to turn
// off. Both event families are emitted, like the Rust side: turn:* and inv:*.
// Live captures, history reads and scan pages publish selection sources; Explain and Investigate
// use selection_preview/send.
// Failures have the Rust shape { code, message } (src/failure.rs), translated by the page. Error
// switches: ?scan=denied (the folder cannot be read) or ?scan=fail (the scan stops midway),
// ?action=refuse (Run now is refused: the target changed), ?preview=expired (a question's preview
// ticket expired, nothing is sent), ?history=unavailable and ?live=fail.

const params = new URLSearchParams(window.location.search);
const listeners = new Map();
const wait = (ms) => new Promise((resolve) => window.setTimeout(resolve, ms));
const GiB = 1024 ** 3;
const MiB = 1024 ** 2;

let orphanTurn = params.get("turn") === "orphan" ? { turnId: "turn-orphan", question: "Why is my PC slow?", startedAt: Date.now() - 42000 } : null;

function emit(name, payload) {
  for (const cb of listeners.get(name) || []) cb(payload);
}
/** Rejects like a Rust command returning a `Failure`: a stable code and the English detail. */
function fail(code, message, reason = null) {
  throw reason ? { code, reason, message } : { code, message };
}
function fmt(bytes) {
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let v = bytes / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u += 1;
  }
  return `${v.toFixed(u >= 2 ? 2 : 1)} ${units[u]}`;
}
const m = (value, text) => ({ value, text, status: "ok", note: null });
const missing = (status, note) => ({ value: null, text: null, status, note });
const absent = () => ({ value: null, text: null, status: null, note: null });
const jitter = (base, spread) => base + (Math.random() - 0.5) * spread;

let cloudMode = params.get("cloud") || "ready";
let cloudKeyConfigured = cloudMode !== "none";
async function checkConnection() {
  await wait(800);
  if (!cloudKeyConfigured) throw new Error("NEBIUS_API_KEY is missing or empty");
  const errors = {
    key: "Provider returned HTTP 401; response body was withheld",
    network: "Provider request failed; details and response body were withheld",
    timeout: "Provider did not answer within the time limit",
    model: "The selected model is absent from the account catalog",
    limit: "Provider returned HTTP 429; response body was withheld",
  };
  const error = errors[params.get("connection")];
  if (error) throw new Error(error);
}
// Web lookups (C6). ?web=on: a Tavily key is saved and web search is allowed, so an AI
// investigation also reads the graphics driver's notes and its answer cites web pages [W1], [W2];
// ?web=key: a key is saved but web search is off. Like the Rust side, the page only ever sees
// whether a key exists and its last four characters.
const webParam = params.get("web");
let searchKey = webParam === "on" || webParam === "key" ? "tvly-synthetic-mock-key-x9Qa" : null;
function searchKeyState() {
  return { search_key_configured: Boolean(searchKey), search_key_hint: searchKey && searchKey.length >= 12 ? searchKey.slice(-4) : null, search_key_source: searchKey ? "file" : null };
}
function cloud() {
  if (cloudMode === "none") {
    return { key_configured: false, key_hint: null, key_source: null, model_id: cloudModel, model_source: null, ready: false, problem: "NEBIUS_API_KEY is missing or empty", file: "C:\\Users\\Alex\\AppData\\Roaming\\pcagent\\.env", file_problem: null, ...searchKeyState() };
  }
  if (cloudMode === "problem") {
    return { key_configured: true, key_hint: "Q7fz", key_source: "file", model_id: "meta/llama", model_source: "file", ready: false, problem: "Select an explicit nvidia/ Nemotron model ID; no fallback model is allowed", file: "C:\\Users\\Alex\\AppData\\Roaming\\pcagent\\.env", file_problem: null, ...searchKeyState() };
  }
  return { key_configured: true, key_hint: "Q7fz", key_source: "file", model_id: cloudModel, model_source: "file", ready: true, reasoning: cloudReasoning, problem: null, file: "C:\\Users\\Alex\\AppData\\Roaming\\pcagent\\.env", file_problem: null, ...searchKeyState() };
}
let cloudModel = cloudMode === "none" ? null : "nvidia/nemotron-3-super-120b-a12b";
let cloudReasoning = "on";
let routingSettings = { mode: "configured", super_model_id: null, ultra_model_id: null };
function routingView() {
  const calibrated = params.get("routing") === "calibrated";
  return { settings: { ...routingSettings }, revision: 0, automatic_available: calibrated, calibration_reviewed_at_unix_ms: calibrated ? Date.now() - 86400000 : null, calibrated_rules: [], limitation: null };
}

function volume(root, totalGiB, freeGiB) {
  return { root, total_bytes: totalGiB * GiB, free_bytes: freeGiB * GiB, free_ratio: freeGiB / totalGiB };
}

// ---------- Selection sources ----------
// Each live capture publishes a host-issued source reference, as SelectionRegistry does. Sources
// expire after five minutes; previews are single-use tickets bound to their selection.
const SELECTION_TTL = params.get("sources") === "short" ? 5000 : 5 * 60 * 1000;
const selectionSources = new Map();
const selectionTickets = new Map();
let selectionSequence = 0;
function publishSource(targets, measuredAt, extra = {}) {
  const id = `selection-source-${++selectionSequence}`;
  selectionSources.set(id, { targets, measuredAt, expires: Date.now() + SELECTION_TTL, ...extra });
  return id;
}
function sameTarget(a, b) {
  return a.kind === b.kind && a.resource === b.resource && a.entity_ref === b.entity_ref && a.event_id === b.event_id && a.recording_id === b.recording_id && a.scope_id === b.scope_id && a.item_ref === b.item_ref;
}

let tick = 0;
function dashboard() {
  tick += 1;
  const total = 15.73 * GiB;
  const available = jitter(3.1, 0.4) * GiB;
  const used = total - available;
  const cpu = Math.round(jitter(1850, 900));
  const idle = Math.round(jitter(9350, 300));
  const procs = [
    ["chrome.exe", 2.31, 1.9],
    ["Code.exe", 1.12, 0.8],
    ["Teams.exe", 0.84, 0.6],
    ["explorer.exe", 0.31, 0.2],
    ["Discord.exe", 0.29, 0.25],
    ["MsMpEng.exe", 0.21, 0.19],
    ["steamwebhelper.exe", 0.18, 0.11],
  ];
  const gpus =
    params.get("gpu") === "none"
      ? []
      : [
          {
            name: "NVIDIA GeForce RTX 4060 Laptop GPU",
            temperature: m(61 + (tick % 3), `${61 + (tick % 3)} °C`),
            memory_used: m(1.4 * GiB, "1.40 GiB"),
            memory_total: m(8 * GiB, "8.00 GiB"),
            memory_ratio: 0.175,
            utilization: m(700, "7.0%"),
            sm_clock: m(1350, "1350 MHz"),
            power: m(14500, "14.5 W"),
            slowdown: m(0, "none"),
            slowdown_reasons: [],
          },
        ];
  const resources = ["memory", "cpu", "disk", ...(gpus.length ? ["gpu"] : [])].map((resource) => ({ kind: "resource", resource }));
  return {
    selection_source_id: publishSource(resources, Date.now()),
    collected_at_unix_ms: Date.now(),
    memory: {
      total: m(total, fmt(total)),
      available: m(available, fmt(available)),
      commit: m(18.2 * GiB, "18.20 GiB"),
      commit_limit: m(24 * GiB, "24.00 GiB"),
      used_ratio: used / total,
      used_text: fmt(used),
    },
    cpu: {
      busy: tick === 1 ? missing("partial", "First observation; a rate needs two samples.") : m(cpu, `${(cpu / 100).toFixed(1)}%`),
      busiest_core: tick === 1 ? absent() : m(cpu + 2100, `${((cpu + 2100) / 100).toFixed(1)}%`),
    },
    disk: {
      read_latency: m(1300, "1.3 ms"),
      write_latency: m(2100, "2.1 ms"),
      idle: m(idle, `${(idle / 100).toFixed(1)}%`),
      queue: m(0, "0"),
      hard_faults: m(1240, "12.4 pages/s"),
      busy_ratio: (10000 - idle) / 10000,
    },
    gpus,
    gpu_engines: [{ kind: "3d", usage: m(820, "8.2%") }, { kind: "videodecode", usage: m(120, "1.2%") }, { kind: "copy", usage: m(0, "0.0%") }],
    gpu_busiest_engine: m(820, "8.2%"),
    processes: procs.map(([name, ws, pc], i) => ({
      name,
      pid: 4000 + i * 713,
      working_set: m(ws * GiB, fmt(ws * GiB)),
      private_commit: m(pc * GiB, fmt(pc * GiB)),
      cpu: tick === 1 ? missing("partial", "First observation.") : m(Math.round(jitter(300, 300)), "3.0%"),
    })),
    process_coverage: { status: "partial", enumerated: 231, readable: 214, permission_denied: 17, unavailable: 0, returned: 7, enumeration_truncated: false },
    // ?disk=ok gives C: room to breathe; by default it is almost full, like the development PC.
    volumes: [
      volume("C:\\", 255, params.get("disk") === "ok" ? 61 : 0.3),
      volume("D:\\", 476, 135),
    ],
    ...liveBatteries(),
    limitations: ["Working sets can share pages; they do not add up to physical memory in use."],
  };
}

// ---------- Live battery ----------
// The Live PC battery read (view::BatteryView), chosen by ?battery=. Like the Rust side it is
// read again about every 10 s (every second dashboard), off the dashboard itself, and each read
// publishes its own selection source for the resource `battery`. Setting
// window.__pcagentMock.holdBattery keeps the last read, like a stalled driver, so its age shows.
const BATTERY_MODE = params.get("battery") || "discharging";
const BATTERY_REF = "battery-00000000-0000-4000-8000-00000000b001";
const BATTERY_REF_2 = "battery-00000000-0000-4000-8000-00000000b002";
let batteryRead = null;

/** Every BatteryView field, unknown (null) unless given. */
function batteryView(ref, at, values) {
  return {
    battery_ref: ref, kind: "system", capacity_units: "milliwatt_hours", measured_at_unix_ms: at,
    charge_percent: null, charge_state: null, on_ac: null, critical: null, power_rate_mw: null,
    estimated_runtime_windows_seconds: null, estimated_runtime_at_current_rate_seconds: null,
    remaining_capacity_mwh: null, full_charge_capacity_mwh: null, design_capacity_mwh: null,
    capacity_retention_percent: null, estimated_wear_percent: null, cycle_count: null, inconsistent: false,
    ...values,
  };
}

/** An absolute-unit battery: remaining energy, retention and wear follow from its capacities. */
function absoluteBattery(ref, at, { charge, design, full, ...values }) {
  const remaining = Math.round((charge / 100) * full);
  return batteryView(ref, at, {
    charge_percent: charge, remaining_capacity_mwh: remaining, full_charge_capacity_mwh: full, design_capacity_mwh: design,
    capacity_retention_percent: (full / design) * 100, estimated_wear_percent: 100 - (full / design) * 100,
    ...values,
  });
}

/** One discharging laptop battery, the default; its charge drops 0.02 % per dashboard. */
function dischargingBattery(ref, at) {
  const power = -Math.round(jitter(8400, 600));
  const battery = absoluteBattery(ref, at, { charge: 78.1 - 0.02 * tick, design: 57000, full: 52800, charge_state: "discharging", on_ac: false, critical: false, power_rate_mw: power, estimated_runtime_windows_seconds: 17400 - 5 * tick, cycle_count: 212 });
  battery.estimated_runtime_at_current_rate_seconds = (battery.remaining_capacity_mwh * 3600) / -power;
  return battery;
}

const BATTERY_FIXTURES = {
  discharging: (at) => [dischargingBattery(BATTERY_REF, at)],
  // The development PC's readings (docs/battery.md): no power, cycles or time left reported.
  ac: (at) => [absoluteBattery(BATTERY_REF, at, { charge: 100, design: 47310, full: 39900, charge_state: "idle", on_ac: true, critical: false, capacity_retention_percent: 84.3373, estimated_wear_percent: 15.6627 })],
  charging: (at) => [absoluteBattery(BATTERY_REF, at, { charge: 46, design: 57000, full: 52800, charge_state: "charging", on_ac: true, critical: false, power_rate_mw: Math.round(jitter(23500, 800)), cycle_count: 212 })],
  low: (at) => {
    const power = -Math.round(jitter(11200, 600));
    const battery = absoluteBattery(BATTERY_REF, at, { charge: 6, design: 57000, full: 52800, charge_state: "discharging", on_ac: false, critical: true, power_rate_mw: power, estimated_runtime_windows_seconds: 1260, cycle_count: 212 });
    battery.estimated_runtime_at_current_rate_seconds = (battery.remaining_capacity_mwh * 3600) / -power;
    return [battery];
  },
  // A percentage only: no energy, power, calculated time left or wear (docs/battery.md).
  relative: (at) => [batteryView(BATTERY_REF, at, { capacity_units: "relative", charge_percent: 64, charge_state: "discharging", on_ac: false, critical: false, estimated_runtime_windows_seconds: 9000 })],
  two: (at) => [
    dischargingBattery(BATTERY_REF, at),
    absoluteBattery(BATTERY_REF_2, at, { charge: 91.4, design: 24000, full: 23200, charge_state: "idle", on_ac: false, critical: false, power_rate_mw: 0, cycle_count: 87 }),
  ],
  unavailable: () => [],
  none: () => [],
};

/** The dashboard's battery fields: the last read, and a new read on every second dashboard. */
function liveBatteries() {
  const fixture = BATTERY_FIXTURES[BATTERY_MODE] || BATTERY_FIXTURES.discharging;
  if (!batteryRead || (tick % 2 === 1 && !window.__pcagentMock?.holdBattery)) {
    const at = Date.now() - 40;
    const batteries = fixture(at);
    batteryRead = { batteries, sourceId: batteries.length ? publishSource([{ kind: "resource", resource: "battery" }], at, { battery: batteries }) : null };
  }
  return { batteries: batteryRead.batteries, battery_source_id: batteryRead.sourceId, battery_unavailable: BATTERY_MODE === "unavailable" };
}

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
    ...extra,
  };
}

// The query lookup_driver_notes composes from the measured driver, as external_tools.rs does.
const DRIVER_QUERY = "NVIDIA GeForce RTX 4060 Laptop GPU driver 32.0.15.6094 release notes known issues";

/** The installed driver (O3) and two pages its lookup found (W1, W2), one without a date. */
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

// A written answer as the model returns it, in the language of the question. With web lookups it
// adds a lead from the driver's notes, cited as web pages, never as measurements.
function mockAnswer(question, web = false) {
  const french = /pourquoi|mon |est-ce|[éèàç]/i.test(question);
  const lead = french
    ? "\n\nUne piste en plus : votre pilote graphique est en version 32.0.15.6094 [O3], et les notes de NVIDIA pour la version 566.36 corrigent une consommation de mémoire trop élevée dans les navigateurs avec l'accélération matérielle [W1] ; NVIDIA la décrit aussi comme un problème connu [W2]. C'est un indice venu du web, pas une mesure : mettez à jour le pilote seulement si le ralentissement continue après avoir fermé des onglets."
    : "\n\nOne more lead: your graphics driver is 32.0.15.6094 [O3], and NVIDIA's notes for version 566.36 fix higher than expected memory use in browsers with hardware acceleration [W1]; NVIDIA also lists it as a known issue [W2]. This is a hint from the web, not a measurement: update the driver only if the slowdown continues after closing tabs.";
  const text = (french
    ? "Votre PC rame surtout parce que la mémoire vive est presque pleine : il ne restait que 1,54 Gio de libre sur 15,73 Gio [O1], et Windows relisait 12,4 pages par seconde depuis le disque pour compenser [F2]. J'en suis moyennement sûr : c'est une mesure d'une seconde.\n\nCe que vous pouvez faire :\n1. Fermez les onglets de **chrome.exe (PID 4000)** dont vous n'avez pas besoin : il occupe 2,31 Gio [O1].\n2. Gardez PC Agent ouvert pendant le prochain ralentissement et reposez la question, pour confirmer.\n\nLe processeur n'était pas saturé (18,5 % [O1]) et le disque répondait en moins de 3 ms [O1] : ils ne semblent pas en cause."
    : "Your PC is slow mainly because memory is almost full: only 1.54 GiB of 15.73 GiB was available [O1], and Windows read 12.4 pages per second back from disk to compensate [F2]. I'm moderately sure: this is a one-second measurement.\n\nWhat you can do:\n1. Close the **chrome.exe (PID 4000)** tabs you don't need: it holds 2.31 GiB [O1].\n2. Keep PC Agent open during the next slowdown and ask again to confirm.\n\nThe processor was not saturated (18.5% [O1]) and the disk answered in under 3 ms [O1], so they don't look involved.") + (web ? lead : "");
  return {
    text,
    confidence: "medium",
    category: "memory",
    citations: [
      { reference: "O1", kind: "observation", title: "get_system_snapshot", facts: ["Available physical memory: 1.54 GiB of 15.73 GiB (9.8%).", "chrome.exe (PID 4000) used 2.31 GiB of RAM (working set).", "CPU busy: 18.5%.", "Disk read response time: 1.3 ms."] },
      { reference: "F2", kind: "finding", title: "Heavy hard-fault paging", facts: ["12.4 pages per second were read from disk to resolve hard faults."] },
      ...(web ? webCitations() : []),
    ],
    follow_ups: french
      ? ["Quels onglets ou extensions utilisent le plus de mémoire ?", "Ai-je besoin de plus de RAM ?"]
      : ["Which tabs or extensions use the most memory?", "Do I need more RAM?"],
    unverified: [],
    repairs: 0,
  };
}

function report(question, cloudRun, error = null, { web = false, toolEvents = [] } = {}) {
  const selected = error
    ? finding("insufficient_evidence", "Insufficient evidence", "insufficient", "The available measurements do not support a conclusion yet.", {
        missing: ["An observation taken while the slowdown happens."],
        next_step: "Run the investigation again while the problem is happening.",
        limitations: ["No cause is excluded because some resources were not measured."],
      })
    : finding("low_available_memory", "Low available memory", "plausible", "Available physical memory was 9.8% of 15.73 GiB during the one-second sample, and chrome.exe (PID 4000) was the largest contributor.", {
        facts: [
          "Available physical memory: 1.54 GiB of 15.73 GiB (9.8%).",
          "System commit: 18.20 GiB of its 24.00 GiB limit (75.8%).",
          "chrome.exe (PID 4000) used 2.31 GiB of RAM (working set).",
          "Hard-fault page reads: 12.4 pages/s.",
        ],
        hypothesis: "Low available memory can make Windows page memory to disk, which slows programs down.",
        counter_evidence: ["The processor was 18.5% busy: it was not saturated.", "Disk response times stayed below 3 ms."],
        missing: ["Memory use over several minutes while the slowdown happens.", "Which browser tabs or extensions use the most memory."],
        next_step: "Keep PC Agent open while the slowdown happens and ask again, or close unused browser tabs and compare.",
        limitations: ["One short sample cannot prove a cause or a memory leak.", "Working sets can share pages; their sum is not the memory in use."],
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
          finding("largest_memory_contributor", "Largest memory contributor", "supported", "chrome.exe (PID 4000) held 2.31 GiB of RAM, the most of any readable program."),
          finding("hard_fault_pressure", "Heavy hard-fault paging", "plausible", "12.4 pages per second were read from disk to resolve hard faults."),
        ],
    answer: cloudRun && !error ? mockAnswer(question, web) : null,
    observations: cloudRun && !error
      ? [{ tool: "get_metric_history", status: "ok", facts: ["CPU busy: 12 usable samples, observed range 9.5% to 31.0%.", "Observed interval: 60000 ms."], limitations: [] }]
      : [],
    tool_events: toolEvents,
    limitations: ["Short observations cannot prove a cause, a leak or a hardware purchase need."],
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


// ---------- Persistent history (Sampler) ----------
// Synthetic five-second samples over the last week with a night-time gap, three anomalies and an
// optional recording. Shapes follow pcagent_store: points, gaps, events and process names.
const historyStart = Date.now() - 7 * 24 * 3600 * 1000;
const historyEvents = [
  { id: "ev-1", kind: "cpu_busy", hoursAgo: 30, minutes: 3, peak: 8700, metric: "cpu_busy_basis_points", unit: "basis_points_of_total_cpu", contributor: "proc-11aa22bb", name: "firefox.exe" },
  { id: "ev-2", kind: "disk_latency", hoursAgo: 52, minutes: 12, peak: 64000, metric: "disk_latency_microseconds", unit: "microseconds", contributor: "proc-33cc44dd", name: "TiWorker.exe" },
  { id: "ev-3", kind: "memory_pressure", hoursAgo: 3, minutes: 6, peak: 9700, metric: "physical_used_basis_points", unit: "basis_points", contributor: "proc-11aa22bb", name: "firefox.exe" },
];
let historyCleared = false;
let recording = null;
let startupEnabled = false;
function historyValue(metric, at) {
  const hours = (Date.now() - at) / 3600000;
  const wave = Math.sin(at / 900000) * 0.5 + Math.sin(at / 170000) * 0.3;
  const near = (e) => Math.abs(hours - e.hoursAgo) < e.minutes / 60;
  const ev = (kind) => historyEvents.find((e) => e.kind === kind && near(e));
  switch (metric) {
    case "cpu_busy_basis_points":
      return ev("cpu_busy") ? 8500 : Math.round(1800 + wave * 900);
    case "physical_total_bytes":
      return Math.round(15.73 * GiB);
    case "physical_available_bytes":
      return ev("memory_pressure") ? Math.round(0.45 * GiB) : Math.round((3.2 + wave * 0.9) * GiB);
    case "disk_idle_basis_points":
      return ev("disk_latency") ? 2500 : Math.round(9300 + wave * 300);
    case "gpu_busiest_engine_basis_points":
      return Math.round(700 + wave * 500);
    default:
      return Math.round(1000 + wave * 500);
  }
}
function historySeries(metric, from, to) {
  // At most 120 points, like the agent's projection; one night without measurements.
  const count = 120;
  const step = Math.max(5000, Math.floor((to - from) / count));
  const gapFrom = Date.now() - 20 * 3600 * 1000;
  const gapTo = gapFrom + 7 * 3600 * 1000;
  const points = [];
  for (let at = from; at <= to; at += step) {
    const inGap = (at >= gapFrom && at < gapTo) || at < historyStart || historyCleared;
    points.push({ unix_ms: at, value: inGap ? null : historyValue(metric, at), min: null, max: null, status: inGap ? "unsupported" : "ok", measured_samples: inGap ? 0 : 1, expected_samples: 1, resolution_ms: step, observed_duration_ms: inGap ? 0 : step, origin: "synthetic_fixture" });
  }
  const gaps = historyCleared ? [{ from_unix_ms: from, to_unix_ms: to, reason: "not_sampled" }] : gapTo > from && gapFrom < to ? [{ from_unix_ms: Math.max(from, gapFrom), to_unix_ms: Math.min(to, gapTo), reason: "not_sampled" }] : [];
  const events = historyCleared
    ? []
    : historyEvents
        .map((e) => {
          const end = Date.now() - e.hoursAgo * 3600000;
          return { id: e.id, kind: e.kind, entity_ref: "system", started_at_unix_ms: end - e.minutes * 60000, ended_at_unix_ms: end, peak: e.peak, metric: e.metric, unit: e.unit, active: false, contributor_ref: e.contributor, contributor_value: null, contributor_metric: null, contributor_unit: null, origin: "synthetic_fixture" };
        })
        .filter((e) => e.ended_at_unix_ms >= from && e.started_at_unix_ms <= to);
  const names = Object.fromEntries(historyEvents.map((e) => [e.contributor, e.name]));
  return { metric, entity_ref: "system", unit: "", status: gaps.length ? "partial" : "ok", origin: "synthetic_fixture", from_unix_ms: from, to_unix_ms: to, points, gaps, events, process_names: names, sources: ["synthetic"], limitations: [] };
}
function samplerStatus() {
  if (recording && recording.ended_at_unix_ms == null) {
    const elapsed = Date.now() - recording.started_at_unix_ms;
    recording.samples = Math.floor(elapsed / 1000);
    if (elapsed >= recording.duration_seconds * 1000) {
      recording.ended_at_unix_ms = recording.started_at_unix_ms + recording.duration_seconds * 1000;
      recording.status = "completed";
    }
  }
  const active = recording && recording.ended_at_unix_ms == null ? recording : null;
  return { running: true, samples: historyCleared ? 12 : 98213, last_sample_unix_ms: Date.now() - 2000, recording: active, error: null, thread_cpu_basis_points: 160, total_cpu_basis_points: 20, logical_processors: 8, resource_window_ms: 60000, host_working_set_bytes: 96 * MiB, cpu_target_basis_points: 50, memory_target_bytes: 50 * MiB };
}


// One opaque battery discharging steadily, one point per recorded second, with the derived loss
// of its continuous discharge segment (pcagent_store::BatteryRecording).
function batteryRecording(rec) {
  const end = rec.ended_at_unix_ms ?? Date.now();
  const ticks = Math.max(1, Math.min(128, Math.floor((end - rec.started_at_unix_ms) / 1000)));
  const ref = "battery-00000000-0000-4000-8000-00000000b001";
  const window = (at) => ({ started_at_unix_ms: at - 40, finished_at_unix_ms: at, duration_ms: 40 });
  const source = "DeviceIoControl/IOCTL_BATTERY_QUERY_STATUS";
  const o = (at, metric, value, unit) => ({ observation_id: `${ref}-measurement-${at}-${metric}`, measured_at_unix_ms: at, window: window(at), entity_ref: ref, metric, value, unit, status: "ok", source, origin: "synthetic_fixture" });
  const startCapacity = 41250;
  const rate = -8400;
  const points = params.get("battery") === "none" ? [] : Array.from({ length: ticks }, (_, i) => {
    const at = rec.started_at_unix_ms + (i + 1) * 1000;
    const capacity = Math.round(startCapacity + (rate * (i + 1)) / 3600);
    return {
      recorded_at_unix_ms: at, recording_elapsed_ms: (i + 1) * 1000, clock_consistent: true, status: "ok",
      observations: [
        o(at, "battery_kind", "system", "category"), o(at, "battery_capacity_units", "milliwatt_hours", "category"), o(at, "battery_charge_state", "discharging", "category"), o(at, "battery_on_ac", false, "boolean"),
        o(at, "battery_remaining_capacity_mwh", capacity, "milliwatt_hours"), o(at, "battery_charge_percent", Math.round((capacity / 52800) * 100), "percent"),
        o(at, "battery_power_rate_mw", Math.round(rate + (Math.random() - 0.5) * 600), "milliwatts"), o(at, "battery_estimated_runtime_windows_seconds", 17400 - i, "seconds"),
      ],
    };
  });
  const first = points[0];
  const last = points[points.length - 1];
  const capacityOf = (p) => p.observations.find((x) => x.metric === "battery_remaining_capacity_mwh");
  const lost = points.length > 1 ? capacityOf(first).value - capacityOf(last).value : null;
  const elapsed = points.length > 1 ? last.recording_elapsed_ms - first.recording_elapsed_ms : 0;
  const derived = (metric, value, unit) => ({ ...o(last.recorded_at_unix_ms, metric, value, unit), source: "derived/recording_battery_capacity_endpoints", derivation: "synthetic", inputs: [capacityOf(first), capacityOf(last)] });
  const evolution = lost > 0 && elapsed > 0
    ? [{ from_unix_ms: first.recorded_at_unix_ms, to_unix_ms: last.recorded_at_unix_ms, measured_points: points.length, observations: [
        derived("battery_measured_capacity_decrease_mwh", lost, "milliwatt_hours"),
        derived("battery_average_power_rate_mw", (-lost * 3600000) / elapsed, "milliwatts"),
        derived("battery_estimated_runtime_from_recording_seconds", (capacityOf(last).value * elapsed) / lost / 1000, "seconds"),
      ] }]
    : [];
  return {
    status: points.length ? "ok" : "partial", started_at_unix_ms: rec.started_at_unix_ms, ended_at_unix_ms: rec.ended_at_unix_ms, interval_ms: 1000,
    coverage: { expected_ticks: rec.duration_seconds, recorded_ticks: ticks, ticks_without_battery: points.length ? 0 : ticks, unavailable_observations: 0, rejected_observations: 0, points_returned: points.length, points_omitted_from_response: 0, batteries_omitted_from_response: 0, battery_tick_points_omitted_during_collection: 0 },
    gaps: [], batteries: points.length ? [{ battery_ref: ref, status: "ok", points, gaps: [], evolution }] : [], limitations: [],
  };
}

// ---------- Action proposals ----------
// A question about freeing space proposes two plans; running one reports space freed. A question
// about start-up proposes turning one program off; once done, it can be restored from the journal.
const turnPlans = new Map();
function proposePlans(turnId, question) {
  if (/startup|start-up|boot|démarr/i.test(question)) {
    turnPlans.set(turnId, [
      { plan_id: `plan-${turnId}-startup`, kind: "disable_user_startup", targets: [{ kind: "startup", label: "Spotify", files: null, bytes: null }], recovery: "restore_previous_startup_approval", checks: ["user_startup_entry_unchanged", "known_startup_approval_format"], decision: null },
    ]);
    return;
  }
  if (!/supprim|delete|clean|nettoy|place|space|free/i.test(question)) return;
  turnPlans.set(turnId, [
    { plan_id: `plan-${turnId}-1`, kind: "run_cleanup_adapter", targets: [{ kind: "cleanup", label: "C:\\Users\\Alex\\AppData\\Local\\Temp", files: 1840, bytes: 18.7 * GiB }], recovery: "none_synthetic_regenerable_data", checks: ["identity_revalidated", "not_protected"], decision: null },
    { plan_id: `plan-${turnId}-2`, kind: "recycle_selected_files", targets: [{ kind: "file", label: "C:\\Users\\Alex\\Downloads\\ubuntu-24.04.iso", files: 1, bytes: null }, { kind: "file", label: "C:\\Users\\Alex\\Downloads\\setup-old.exe", files: 1, bytes: null }], recovery: "recycle_bin_if_operation_supported", checks: ["identity_revalidated", "recycle_bin_available"], decision: null },
  ]);
}

let nextId = 1;
// Saved conversations, as the Rust side keeps them in SQLite.
const conversations = [];
let nextConversation = 1;
// ?conversation=empty: a saved question whose turn never finished (window closed mid-run).
if (params.get("conversation") === "empty") {
  const at = Date.now() - 3600000;
  conversations.push({ id: `conv-${nextConversation++}`, title: "Why is my battery stuck at 6%?", created_at_unix_ms: at, updated_at_unix_ms: at, turns: [] });
}
function saveTurn(conversationId, turnId, question, report) {
  const c = conversations.find((x) => x.id === conversationId);
  if (!c) return null;
  c.turns.push({ id: turnId, question, asked_at_unix_ms: Date.now(), report, tool_events: report.tool_events || [] });
  c.updated_at_unix_ms = Date.now();
  return c.id;
}
function done(id, report) {
  const own = running && running.id === id;
  const conversationId = own ? saveTurn(running.conversationId, running.turnId, running.question, report) : null;
  if (own) {
    const turn = { id: running.turnId, question: running.question, asked_at_unix_ms: running.askedAt, answer: report.answer, tool_events: report.tool_events || [], report };
    if (report.error) emit("turn:failed", { turn_id: running.turnId, conversation_id: conversationId, code: report.error.code, message: report.error.message, turn });
    else emit("turn:done", { turn_id: running.turnId, conversation_id: conversationId, turn });
  }
  return emit("inv:done", { id, report, conversation_id: conversationId });
}
let running = null;
let nextTurn = 1;
const privacyParam = params.get("privacy");
// The composer's toggles are on by default (C5, 9 October); ?web=key saves a key with web search off.
let privacy = ["complete", "anonymized", "local"].includes(privacyParam)
  ? { mode: privacyParam, confirmed: true, web_search: webParam !== "key", file_names: true }
  : { mode: "complete", confirmed: false, web_search: webParam !== "key", file_names: true };
/** Lookups run only in a cloud mode, with the separate consent and a saved key, as in app.rs. */
const webActive = () => privacy.mode !== "local" && privacy.web_search && Boolean(searchKey);
// Web pages "opened" through open_web_source: recorded for review scripts, never navigated to.
const openedWebSources = [];
window.__pcagentMock = { openedWebSources, holdBattery: false };
/** What PrivacyContext removes from free text: the profile path, the user and machine names. */
function sanitize(text) {
  return text
    .replace(/C:\\Users\\Alex/gi, "%USERPROFILE%")
    .replace(/\bAlex\b/g, "[user]")
    .replace(/DESKTOP-ALEX/gi, "[computer]");
}
function fileNameSharing(root) {
  if (!root) return { names: [], scope: null };
  return {
    scope: "%SCOPE%",
    names: [
      { item_ref: "scan-1:1", name: "Games", relative_path: "%SCOPE%\\Games" },
      { item_ref: "scan-1:2", name: "Videos", relative_path: "%SCOPE%\\Videos" },
      { item_ref: "scan-1:3", name: "windows-11-24h2.iso", relative_path: "%SCOPE%\\Installers\\windows-11-24h2.iso" },
    ],
  };
}
let pendingPreview = null;
let previewSequence = 0;

async function investigate(id, question, cloudRun) {
  const slow = /slow|lent/i.test(question) ? 1.6 : 1;
  const step = (s) => {
    const at = Date.now();
    emit("turn:step", { turn_id: running.turnId, at_unix_ms: at, step: s });
    if (s.kind === "answer_draft") emit("turn:delta", { turn_id: running.turnId, request: s.request, text: s.text, replace: true, unverified: true });
    emit("inv:step", { id, at_unix_ms: at, step: s });
  };
  step({ kind: "snapshot_started" });
  await wait(900 * slow);
  step({ kind: "snapshot_finished", duration_ms: 1012, measured: 38 });
  if (!cloudRun) {
    await wait(250);
    return done(id, report(question, false));
  }
  let request = 0;
  const model = async (ms, prompt, completion, outcome = "ok") => {
    request += 1;
    step({ kind: "model_started", request });
    await wait(ms * slow);
    step({ kind: "model_finished", request, duration_ms: ms, prompt_tokens: prompt, completion_tokens: completion, outcome });
  };
  const toolEvents = [];
  const tool = async (name, ms, status = "ok", query = null) => {
    step({ kind: "tool_started", tool: name });
    await wait(ms * slow);
    // A web lookup reports the exact query it sent; other tools have none (skipped like serde).
    const event = { tool: name, status, duration_ms: ms, ...(query ? { query } : {}) };
    toolEvents.push(event);
    step({ kind: "tool_finished", ...event });
  };
  if (running.stop) return done(id, report(question, true, "cancelled"));
  await model(1400, 4627, 83);
  if (/fail|échec/i.test(question)) {
    await model(1600, 9880, 8192, "output_limit");
    return done(id, report(question, true, "output_limit"));
  }
  if (running.stop) return done(id, report(question, true, "cancelled"));
  await tool("get_processes", 420);
  await tool("get_metric_history", 180);
  const web = webActive();
  if (web) {
    await tool("get_drivers", 260);
    await tool("lookup_driver_notes", 1100, "ok", DRIVER_QUERY);
  }
  if (/extend|prolong/i.test(question)) {
    const answer = new Promise((resolve) => {
      running.answer = resolve;
    });
    const extension = {
      request: { reason: "rounds", available: true, additional_rounds: 6, additional_tool_calls: 12, additional_seconds: 90 },
      usage: { requests: 6, tool_calls: 6, extensions: 0, prompt_tokens: 30210, completion_tokens: 812, reasoning_tokens: 600, cost: { usd: "0.0098", exact_usd: "0.0097938", price: null } },
    };
    emit("turn:extension", { turn_id: running.turnId, ...extension });
    emit("inv:extension", { id, ...extension });
    const accepted = await answer;
    if (!accepted) return done(id, { ...report(question, true, "budget"), execution: "budget_exhausted" });
  }
  if (running.stop) return done(id, report(question, true, "cancelled"));
  // The last request writes the answer: its text arrives in pieces, like Nebius's stream.
  const final = report(question, true, null, { web, toolEvents });
  request += 1;
  step({ kind: "model_started", request });
  await wait(500 * slow);
  const text = final.answer?.text || "";
  for (let end = 12; end < text.length + 12; end += 12) {
    step({ kind: "answer_draft", request, text: text.slice(0, end) });
    await wait(70 * slow);
  }
  step({ kind: "model_finished", request, duration_ms: 1300, prompt_tokens: 8969, completion_tokens: 271, outcome: "ok" });
  done(id, final);
}

let scanId = 0;
let scanCancel = false;
function items(kind, root, count, offset = 0) {
  const dirNames = ["Games", "Videos", "node_modules", "Photos 2025", "Installers", "target", "Backups", "Music", "Old projects", "Courses"];
  const fileNames = ["windows-11-24h2.iso", "holiday-2025.mp4", "dataset.zip", "vm-disk.vhdx", "setup-game.exe", "backup-march.7z", "lecture-04.mkv", "photos-raw.tar", "driver-pack.zip", "podcast-ep12.wav"];
  const names = kind === "dirs" ? dirNames : fileNames;
  return Array.from({ length: count }, (_, i) => {
    const n = offset + i;
    const size = Math.round((kind === "dirs" ? 48 : 9) * GiB * Math.pow(0.72, n));
    const name = n < names.length ? names[n] : `${kind === "dirs" ? "Folder" : "file"}-${n}${kind === "dirs" ? "" : ".bin"}`;
    const parent = kind === "dirs" ? "" : ["", "Videos", "Installers", "Backups"][n % 4];
    return {
      item_ref: `scan-${scanId}:${n + 1}`,
      name,
      parent,
      path: `${root}\\${parent ? `${parent}\\` : ""}${name}`,
      logical_bytes: size,
      allocated_bytes: size,
      size,
      size_text: fmt(size),
      files: kind === "dirs" ? 1200 - n * 90 : null,
      modified_unix_ms: Date.now() - (n + 1) * 86400000 * 9,
      status: n === 3 && kind === "dirs" ? "partial" : "ok",
    };
  });
}
function page(kind, root, offset) {
  const total = kind === "dirs" ? 64 : 180;
  const count = Math.min(50, total - offset);
  return { kind, items: items(kind, root, count, offset), total, next_offset: offset + count < total ? offset + count : null };
}
// Like retain_storage_page: each page of a completed scan is published as a selection source
// whose targets are its items; the page carries its selection_source_id.
function publishPage(scopeId, shown, measuredAt) {
  const targets = shown.items.map((item) => ({ kind: "storage", scope_id: scopeId, item_ref: item.item_ref }));
  const storage = Object.fromEntries(shown.items.map((item, i) => [item.item_ref, { kind: shown.kind, alias: `${shown.kind === "dirs" ? "dir" : "file"}-${i + 1}`, allocated_bytes: item.allocated_bytes, files: item.files }]));
  return { ...shown, selection_source_id: targets.length ? publishSource(targets, measuredAt, { storage }) : null };
}
// Replacing a scan revokes every source of the previous one, as invalidate_storage does.
function invalidateStorage() {
  for (const [id, source] of selectionSources) {
    if (source.targets.some((target) => target.kind === "storage")) selectionSources.delete(id);
  }
}
let scanFinishedAt = 0;
async function scan(scopeId, root) {
  const started = Date.now();
  scanFinishedAt = 0;
  let entries = 0;
  while (Date.now() - started < 2600) {
    if (scanCancel) break;
    entries += Math.round(jitter(4200, 800));
    emit("scan:progress", { scope_id: scopeId, entries, dirs: Math.round(entries / 9), bytes: entries * 1.9 * MiB, bytes_text: fmt(entries * 1.9 * MiB), elapsed_ms: Date.now() - started });
    await wait(150);
    // ?scan=fail: the scan stops midway with scan:failed, which carries a code like a command.
    if (params.get("scan") === "fail" && Date.now() - started > 900) {
      emit("scan:failed", { scope_id: scopeId, code: "scan_failed", message: `Cannot scan ${root}: Scope root cannot be accessed: scan worker panicked.` });
      return;
    }
  }
  emit("scan:done", {
    scope_id: scopeId,
    root,
    status: scanCancel ? "partial" : "partial",
    duration_ms: Date.now() - started,
    scanned: { logical_bytes: 212 * GiB, allocated_bytes: 208 * GiB, allocation_unknown_files: 0, files: entries },
    scanned_text: "208.00 GiB",
    volume: { total_bytes: 476 * GiB, free_bytes: 61 * GiB, total_text: "476.00 GiB", free_text: "61.00 GiB", filesystem: "NTFS" },
    coverage: { files: entries, dirs: Math.round(entries / 9), denied_dirs: 3, denied_files: 0, reparse_skipped: 12, placeholders: 41, placeholder_logical_bytes: 3 * GiB, other_volume_skipped: 0, special_skipped: 0, vanished: 0, changed_during_scan: 5, errors: scanCancel ? 0 : 2, hard_link_aliases: 0, allocation_unknown_files: 0, truncated: null, cancelled: scanCancel },
    denied: [`${root}\\Private`],
    dirs: publishPage(scopeId, page("dirs", root, 0), (scanFinishedAt = Date.now())),
    files: publishPage(scopeId, page("files", root, 0), scanFinishedAt),
    limitations: ["Modification times are not last-use times; last use is unknown."],
  });
}

// ---------- Elevated quick drive analysis ----------
let elevation = null;
function mftResult(volume) {
  const now = Date.now();
  const o = (metric, value, unit = "count") => ({ observation_id: `obs-${metric}`, measured_at_unix_ms: now, window: { started_at_unix_ms: now - 9000, finished_at_unix_ms: now, duration_ms: 9000 }, entity_ref: "ntfs-volume-1", metric, value, unit, status: "partial", source: "synthetic_fixture", origin: "synthetic_fixture" });
  const item = (path, files, size, unknown = 0) => ({ path, files, logical_bytes_known: Math.round(size * 0.97), data_cluster_bytes_known: size, unknown_logical_files: unknown, unknown_allocation_files: unknown, hard_linked_files: 0 });
  return {
    status: "partial", measured_at_unix_ms: now, window: { started_at_unix_ms: now - 9000, finished_at_unix_ms: now, duration_ms: 9000 }, origin: "synthetic_fixture",
    observations: [
      o("ntfs_files_examined", 1284311), o("ntfs_supported_data_allocation_bytes_known", 187.4 * GiB, "bytes"), o("ntfs_supported_logical_bytes_known", 179.2 * GiB, "bytes"),
      o("ntfs_record_slots_walked", 1000000), o("ntfs_record_slots_not_walked", 312480),
    ],
    coverage: [
      { scope: "ntfs_inventory", status: "partial", source: "FSCTL_GET_NTFS_VOLUME_DATA, $MFT", detail: "Record limit reached: 312,480 record slots were not walked. Live volume, not an atomic snapshot." },
      { scope: "ntfs_tree", status: "partial", source: "FILE_NAME metadata", detail: "Largest 16 attributed folders and files; parents and children overlap." },
    ],
    local_mft_tree: {
      directories: [
        item("Users", 412004, 96.3 * GiB, 12), item("Users\\Alex", 398110, 91.8 * GiB, 12), item("Users\\Alex\\AppData", 301442, 54.1 * GiB, 9),
        item("Windows", 512881, 31.6 * GiB, 40), item("Program Files", 98102, 22.4 * GiB), item("Users\\Alex\\Downloads", 1204, 18.7 * GiB),
      ],
      files: [
        item("Users\\Alex\\Downloads\\windows-11-24h2.iso", 1, 6.1 * GiB), item("hiberfil.sys", 1, 4.7 * GiB), item("pagefile.sys", 1, 4.0 * GiB),
        item("Users\\Alex\\Videos\\vacances.mp4", 1, 2.2 * GiB),
      ],
      indexed_nodes: 250000, indexed_name_links: 262114, retained_bytes: 31 * MiB, attributed_files: 1190220, unattributed_files: 94091,
      attributed_logical_bytes_known: 170.1 * GiB, unattributed_logical_bytes_known: 9.1 * GiB, attributed_data_cluster_bytes_known: 177.9 * GiB, unattributed_data_cluster_bytes_known: 9.5 * GiB,
      limitations: ["index_node_limit", "top_directories_truncated", "top_files_truncated"],
    },
  };
}

// ---------- Elevated deeper checks ----------
// Same shapes as pcagent_inspect::elevation::to_inspection: opaque entity references
// ("disk-reliability-<uuid>"), one observation per metric with its unit, null for a value the
// source did not report, and coverage notes in the core's English.
let entitySequence = 0;
function entityRef(prefix) {
  entitySequence += 1;
  return `${prefix}-00000000-0000-4000-8000-${String(entitySequence).padStart(12, "0")}`;
}
function inspection(rows, coverage, durationMs) {
  const now = Date.now();
  const window = { started_at_unix_ms: now - durationMs, finished_at_unix_ms: now, duration_ms: durationMs };
  const observations = rows.map(([entity_ref, metric, value, unit, status = "ok"], i) => ({
    observation_id: `measurement-${now}-${i}`, measured_at_unix_ms: now, window, entity_ref, metric, value, unit,
    status: value == null && status === "ok" ? "unsupported" : status, source: "synthetic_fixture", origin: "synthetic_fixture",
  }));
  const status = coverage.every((c) => c.status === "ok") && observations.every((o) => o.status === "ok") ? "ok" : "partial";
  return { status, measured_at_unix_ms: now, window, origin: "synthetic_fixture", observations, coverage };
}
function systemCachesResult() {
  const source = "pcagent-elevated / pcagent-storage ClassicBackend metadata";
  const cache = (category, exists, logical, allocated, files, unknown, status = "ok") => {
    const e = entityRef("system-cache");
    return [[e, "system_cache_category", category, "category", status], [e, "system_cache_exists", exists, "boolean", status], [e, "system_cache_logical_bytes", logical, "bytes", status], [e, "system_cache_allocated_known_bytes", allocated, "bytes", status], [e, "system_cache_file_count", files, "count", status], [e, "system_cache_allocation_unknown_files", unknown, "count", status]];
  };
  return inspection(
    [...cache("previous_windows", false, 0, 0, 0, 0), ...cache("windows_update_download", true, 2.31 * GiB, 2.33 * GiB, 1204, 0), ...cache("delivery_optimization", true, 4.12 * GiB, 4.15 * GiB, 389, 2, "partial")],
    [
      { scope: "previous_windows", status: "ok", source, detail: "Metadata snapshot only; observed allocation is not reclaimable space. Missing values remain unknown. Limits: [MetadataSnapshot]; coverage: None." },
      { scope: "windows_update_download", status: "ok", source, detail: "Metadata snapshot only; observed allocation is not reclaimable space. Missing values remain unknown. Limits: [MetadataSnapshot]; coverage: Some(CacheCoverage { directories: 212, denied_directories: 0 })." },
      { scope: "delivery_optimization", status: "partial", source, detail: "Metadata snapshot only; observed allocation is not reclaimable space. Missing values remain unknown. Limits: [MetadataSnapshot, AllocationLowerBound, DefaultCacheLocationOnly]; coverage: Some(CacheCoverage { directories: 41, denied_directories: 0 })." },
    ],
    4200,
  );
}
function shadowStorageResult() {
  const e = entityRef("shadow-storage");
  return inspection(
    [[e, "shadow_storage_used_bytes", 6.2 * GiB, "bytes"], [e, "shadow_storage_allocated_bytes", 6.5 * GiB, "bytes"], [e, "shadow_storage_maximum_bytes", 12.75 * GiB, "bytes"], [e, "shadow_storage_maximum_unlimited", false, "boolean"]],
    [{ scope: "shadow_storage", status: "partial", source: "WMI Win32_ShadowStorage", detail: "Includes other shadow copies; unavailable per-restore-point attribution. Not reclaimable bytes. Limits: [RestorePointAttributionUnavailable]." }],
    1800,
  );
}
function diskReliabilityResult() {
  const disk = (temperature, limit, wear, hours, errors, cycles, latencies, status = "ok") => {
    const e = entityRef("disk-reliability");
    return [
      [e, "disk_reliability_temperature", temperature, "celsius", status], [e, "disk_reliability_temperature_limit", limit, "celsius", status], [e, "disk_reliability_wear", wear, "percent", status],
      [e, "disk_reliability_power_on_hours", hours, "hours", status], [e, "disk_reliability_read_errors_total", errors[0], "count", status], [e, "disk_reliability_read_errors_uncorrected", errors[1], "count", status],
      [e, "disk_reliability_write_errors_total", errors[2], "count", status], [e, "disk_reliability_write_errors_uncorrected", errors[3], "count", status], [e, "disk_reliability_start_stop_cycles", cycles, "count", status],
      [e, "disk_reliability_maximum_read_latency", latencies[0], "ms", status], [e, "disk_reliability_maximum_write_latency", latencies[1], "ms", status], [e, "disk_reliability_maximum_flush_latency", latencies[2], "ms", status],
    ];
  };
  return inspection(
    [...disk(41, 84, 3, 4211, [0, 0, 0, 0], 1532, [48, 112, 9]), ...disk(35, null, null, 12890, [null, null, null, null], 4410, [null, null, null])],
    [{ scope: "disk_reliability", status: "partial", source: "WMI root/Microsoft/Windows/Storage MSFT_StorageReliabilityCounter", detail: "Device-reported historical counters; absent fields are unknown. Limits: [ProviderValueMissing]." }],
    2600,
  );
}
function batteryEnergyResult() {
  const history = entityRef("battery-energy-history");
  const rows = [[history, "battery_energy_estimated_total", 48.2e6, "mJ"], [history, "battery_energy_committed_rows", 1284, "count"], [history, "battery_energy_distinct_apps", 7, "count"]];
  const apps = [[16.4e6, 14.1e6, 12.2e6, 412, 9.8], [9.7e6, 8.9e6, 7.4e6, 233, 6.1], [6.1e6, 5.2e6, 0.4e6, 198, 7.9], [5.3e6, 4.8e6, 4.1e6, 140, 3.2], [4.2e6, 3.1e6, 0, 151, 7.2], [3.6e6, 3.3e6, 2.9e6, 88, 2.4], [2.9e6, 1.1e6, 0, 62, 5.5]];
  apps.forEach(([energy, screen, foreground, records, hours], i) => {
    const e = entityRef("battery-energy-app");
    rows.push(
      [e, "battery_app_energy_estimated", energy, "mJ"], [e, "battery_app_screen_on_energy_estimated", screen, "mJ"], [e, "battery_app_foreground_energy_estimated", foreground, "mJ"],
      [e, "battery_app_committed_rows", records, "count"], [e, "battery_app_reported_row_durations", Math.round(hours * 3600000), "ms"],
      [e, "battery_app_historical_first_civil_stamp", `2026-09-${21 + (i % 3)} 08:${10 + i}:00.000000000 (provider civil time; zone unknown)`, "provider_civil_time"],
      [e, "battery_app_historical_last_civil_stamp", `2026-10-0${4 + (i % 2)} 2${i % 4}:4${i}:00.000000000 (provider civil time; zone unknown)`, "provider_civil_time"],
    );
  });
  return inspection(
    rows,
    [{ scope: "battery_energy_history", status: "partial", source: "Windows E3 historical estimates / powercfg srumutil CSV", detail: "Historical E3 estimates for accepted committed on-battery rows of this user only. Civil timestamps have unknown timezone; summed row durations may overlap and are not a measurement window. Export time is the read time. Not physical power, exhaustive app attribution or battery discharge. Limits: [ExportPartial]." }],
    7400,
  );
}
const CHECK_RESULTS = { system_caches: systemCachesResult, shadow_storage: shadowStorageResult, disk_reliability: diskReliabilityResult, battery_energy: batteryEnergyResult };

export function createMock() {
  const commands = {
    app_info: () => ({ version: "0.1.0", cloud: cloud(), models: [
      { id: "nvidia/nemotron-3-super-120b-a12b", note: "recommended" },
      { id: "nvidia/Nemotron-3-Ultra-550b-a55b", note: "most_capable" },
      { id: "nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B", note: "cheaper_slower" },
      { id: "nvidia/Nemotron-3_5-Lightning", note: "not_recommended" },
    ] }),
    dashboard: async () => {
      await wait(tick === 0 ? 700 : 120);
      if (params.get("live") === "fail") fail("live_unavailable", "Measurements took too long.");
      return dashboard();
    },
    process_activity: async () => {
      await wait(600);
      const now = Date.now();
      const window = { started_at_unix_ms: now - 2000, finished_at_unix_ms: now, duration_ms: 2000 };
      const app = (ref, name, count, cpu, ws, members, services = []) => ({
        entity_ref: ref, name, process_count: count, members_truncated: false,
        cpu: m(cpu, `${(cpu / 100).toFixed(1)}%`), working_set: m(ws * GiB, fmt(ws * GiB)), private_commit: m(ws * 0.8 * GiB, fmt(ws * 0.8 * GiB)),
        working_set_increase: ref === "app-1" ? m(0.2 * GiB, fmt(0.2 * GiB)) : m(0, "0"), io_read: m(120000, "117 KiB/s"), io_write: m(40000, "39 KiB/s"),
        gpu: ref === "app-1" ? m(310, "3.1%") : missing("unsupported", "No GPU counter instance for these processes."),
        services,
        members: members.map(([pid, mcpu, mws], i) => ({ entity_ref: `proc-${ref.slice(4)}p${i}`, name, pid, cpu: m(mcpu, ""), working_set: m(mws * GiB, ""), services: [] })),
      });
      const applications = [
          app("app-1", "chrome.exe", 14, Math.round(jitter(940, 200)), 2.9, [[4000, 420, 0.92], [4012, 210, 0.61], [4044, 120, 0.44], [4071, 60, 0.31]]),
          app("app-2", "Code.exe", 9, Math.round(jitter(210, 80)), 1.2, [[5100, 120, 0.51], [5133, 40, 0.28]]),
          app("app-3", "Teams.exe", 6, Math.round(jitter(140, 60)), 0.84, [[6200, 90, 0.42]]),
          app("app-4", "svchost.exe", 31, Math.round(jitter(180, 60)), 0.62, [[1108, 80, 0.06]], [
            { name: "wuauserv", display_name: "Windows Update", state: "running" },
            { name: "Spooler", display_name: "Print Spooler", state: "running" },
          ]),
          app("app-5", "MsMpEng.exe", 1, Math.round(jitter(90, 40)), 0.21, [[3200, 90, 0.21]], [{ name: "WinDefend", display_name: "Microsoft Defender Antivirus Service", state: "running" }]),
          app("app-6", "explorer.exe", 2, 30, 0.31, [[2900, 30, 0.31]]),
      ];
      const processes = applications.flatMap((a) => a.members.map((member) => ({ kind: "process", entity_ref: member.entity_ref })));
      return {
        selection_source_id: publishSource(processes, now),
        collected_at_unix_ms: now, window_ms: 2000, status: "partial",
        applications,
        applications_total: 74,
        coverage: { status: "partial", enumerated: 231, readable: 214, permission_denied: 17, unavailable: 0, returned: 50, enumeration_truncated: false },
        services_status: "ok", gpu_status: "partial", limitations: [], window,
      };
    },
    action_plans: ({ turnId }) => (turnPlans.get(turnId) || []).map((p) => ({ ...p })),
    // Journal-based restoration of a start-up entry turned off by a completed plan: a fresh plan
    // in its own local action group, still run only through action_run.
    startup_restore_prepare: async ({ sourcePlanId, itemIndex }) => {
      await wait(300);
      const source = [...turnPlans.values()].flat().find((p) => p.plan_id === sourcePlanId);
      if (!source || source.kind !== "disable_user_startup" || source.decision !== "completed" || itemIndex !== 0) fail("action_refused", "startup disable is not durably completed", "restore_unavailable");
      const turnId = `local-restore-${sourcePlanId}`;
      const plan = { plan_id: `plan-restore-${Date.now()}`, kind: "restore_user_startup", targets: source.targets.map((x) => ({ ...x })), recovery: "restore_previous_startup_approval", checks: ["user_startup_entry_unchanged", "known_startup_approval_format"], decision: null };
      turnPlans.set(turnId, [plan]);
      return { turn_id: turnId, plan: { ...plan } };
    },
    action_decline: ({ turnId, planId }) => {
      const plan = (turnPlans.get(turnId) || []).find((p) => p.plan_id === planId);
      if (!plan) fail("action_refused", "These proposals are no longer current.", "stale");
      plan.decision = plan.decision || "declined";
    },
    action_run: async ({ turnId, planId }) => {
      const plan = (turnPlans.get(turnId) || []).find((p) => p.plan_id === planId);
      if (!plan || plan.decision) fail("action_refused", "This proposal was already decided.", "already_decided");
      await wait(1200);
      // ?action=refuse: the broker's last check finds the target changed since it was proposed.
      if (params.get("action") === "refuse") {
        plan.decision = "refused";
        fail("action_refused", "target t1 changed since it was resolved", "changed");
      }
      plan.decision = "completed";
      const gained = plan.kind === "run_cleanup_adapter" ? 18.6 * GiB : null;
      return { plan_id: planId, overall: "completed", items: [], verification: { volume: gained ? { gained_bytes: gained, status: "ok" } : null, comparisons: [] } };
    },
    history_status: () => samplerStatus(),
    metric_history: async ({ metric, fromUnixMs, toUnixMs }) => {
      await wait(150);
      if (params.get("history") === "unavailable") fail("history_unavailable", "Local history is unavailable.");
      const series = historySeries(metric, fromUnixMs, toUnixMs);
      // Reading history publishes its events (and a finished recording) as selection sources.
      const targets = series.events.map((e) => ({ kind: "event", event_id: e.id }));
      if (recording && recording.ended_at_unix_ms != null) targets.push({ kind: "recording", recording_id: recording.id });
      if (targets.length) publishSource(targets, toUnixMs);
      return series;
    },
    record_activity_start: ({ durationSeconds }) => {
      if (params.get("history") === "unavailable") fail("history_unavailable", "Local history is unavailable.");
      if (recording && recording.ended_at_unix_ms == null) throw new Error("A recording is already running.");
      // ?recording=short makes the test recordings last 3 seconds.
      const seconds = params.get("recording") === "short" ? 3 : durationSeconds;
      recording = { id: `rec-${Date.now()}`, started_at_unix_ms: Date.now(), ended_at_unix_ms: null, duration_seconds: seconds, status: "recording", samples: 0, markers: [{ at_unix_ms: Date.now(), kind: "start" }] };
      return { ...recording };
    },
    // The battery points of a recording, as Store::battery_recording returns them; ?battery=none
    // is a desktop PC without a battery.
    battery_recording: async ({ recordingId }) => {
      await wait(150);
      if (!recording || recording.id !== recordingId) return null;
      return batteryRecording(recording);
    },
    record_activity_cancel: ({ recordingId }) => {
      if (!recording || recording.id !== recordingId || recording.ended_at_unix_ms != null) throw new Error("This recording is over.");
      recording.ended_at_unix_ms = Date.now();
      recording.status = "cancelled";
    },
    history_clear: () => {
      historyCleared = true;
      return 98213;
    },
    startup_get: () => startupEnabled,
    startup_set: ({ enabled }) => {
      startupEnabled = Boolean(enabled);
      return startupEnabled;
    },
    privacy_get: () => ({ ...privacy }),
    // Same shape as PrivacySettings: unknown fields are refused, a missing toggle is on.
    privacy_set: ({ settings }) => {
      const unknown = Object.keys(settings).filter((key) => !["mode", "confirmed", "web_search", "file_names"].includes(key));
      if (unknown.length) throw new Error(`unknown field ${unknown[0]}`);
      if (settings.mode !== privacy.mode || !settings.confirmed || settings.web_search !== privacy.web_search) pendingPreview = null;
      privacy = { mode: settings.mode, confirmed: Boolean(settings.confirmed), web_search: settings.web_search ?? true, file_names: settings.file_names ?? true };
      return { ...privacy };
    },
    cloud_preview: async ({ question, names, conversationId = null, fileNames = false }) => {
      const sequence = ++previewSequence;
      await wait(300);
      if (fileNames && !names) throw new Error("File names require complete sharing.");
      if (fileNames && !lastScanRoot) fail("scan_unavailable", "Scan and authorize a folder before previewing its names.");
      const message = { question: sanitize(question), initial_observation: { schema_version: 1, origin: "synthetic_fixture", measurements: { values: { system: { physical_total_bytes: 16888365056, physical_available_bytes: 1653562000, cpu_busy_basis_points: 1850 }, "proc-1a2b3c4d": { process_working_set_bytes: 2480000000 } }, units: { physical_total_bytes: "bytes" } } }, findings: [{ id: "F1", classification: "low_available_memory" }], current_finding_ids: ["F1", "F2", "F3"] };
      if (names) message.process_names = { "proc-1a2b3c4d": "chrome.exe" };
      const conversation = conversations.find((c) => c.id === conversationId);
      if (conversation?.turns.length) message.previous_turns = conversation.turns.map((turn) => ({ question: turn.question, answer: "Earlier synthetic observation.", observations: [] }));
      if (sequence !== previewSequence) fail("preview_changed", "A newer sharing preview was requested.");
      const id = `preview-${sequence}`;
      pendingPreview = { id, question, names, conversationId, fileNames };
      return { id, message, bytes: JSON.stringify(message).length, measured: 38, findings: 3, names: names ? 1 : 0, storage_folder: lastScanRoot, file_name_sharing: fileNames ? fileNameSharing(lastScanRoot) : { names: [], scope: null } };
    },
    selection_sources: () => {
      const now = Date.now();
      return [...selectionSources.entries()]
        .filter(([, source]) => source.expires > now)
        .reverse()
        .flatMap(([id, source]) => source.targets.map((target) => ({ reference: { source_id: id, target }, measured_at_unix_ms: source.measuredAt, historical: false })));
    },
    selection_preview: async ({ reference, intent, question = null, conversationId = null, names, fileNames = false }) => {
      await wait(250);
      if (!["explain", "investigate"].includes(intent)) throw new Error("Invalid local selection reference.");
      const source = selectionSources.get(reference?.source_id);
      if (!source || source.expires <= Date.now()) fail("selection_expired", "The selected capture expired or was invalidated. Select the measured item again.");
      if (!source.targets.some((target) => sameTarget(target, reference.target))) throw new Error("The target does not belong to this source type.");
      const verb = intent === "explain" ? "Explain" : "Investigate";
      const noun = reference.target.kind === "resource" ? `the selected ${reference.target.resource} measurements` : reference.target.kind === "storage" ? "the selected scan item" : `the selected ${reference.target.kind}`;
      // A scan item leaves under its alias: never its name or path without the file-name opt-in.
      const stored = reference.target.kind === "storage" ? source.storage?.[reference.target.item_ref] : null;
      const asked = question || `${verb} ${noun}, stating what is measured and what remains uncertain.`;
      const age = Date.now() - source.measuredAt;
      const evidence = {
        status: "ok",
        // Like the host's projection: a scan item is named by its alias, never by its scope or scan.
        selection: { target: stored ? { kind: "storage", alias: stored.alias } : reference.target, intent, fresh_collection: false },
        measured_at_unix_ms: source.measuredAt,
        age_ms: age,
        freshness: "already_measured",
        observations: stored
          ? [{ observation_id: "O0", entity_ref: stored.alias, metric: "allocated_bytes", value: stored.allocated_bytes, unit: "bytes", status: "ok" }]
          : reference.target.kind === "resource" && reference.target.resource === "battery"
            ? (source.battery || []).flatMap((b) =>
                [["battery_charge_percent", b.charge_percent, "percent"], ["battery_power_rate_mw", b.power_rate_mw, "milliwatts"], ["battery_charge_state", b.charge_state, "category"]].map(([metric, value, unit]) => ({
                  observation_id: `${b.battery_ref}-measurement-${b.measured_at_unix_ms}-${metric}`,
                  measured_at_unix_ms: b.measured_at_unix_ms,
                  entity_ref: b.battery_ref,
                  metric,
                  value,
                  unit,
                  status: value == null ? "unsupported" : "ok",
                })),
              )
          : reference.target.kind === "resource" && reference.target.resource === "memory"
            ? [{ observation_id: "O0", metric: "physical_available_bytes", value: 3326083072, unit: "bytes", status: "ok" }, { observation_id: "O0", metric: "physical_total_bytes", value: 16888365056, unit: "bytes", status: "ok" }]
            : [{ observation_id: "O0", metric: "cpu_busy_basis_points", value: 1850, unit: "basis_points", status: "ok" }],
        storage_items: stored ? [{ item_ref: stored.alias, alias_path: `<scope>/${stored.alias}`, kind: stored.kind, allocated_bytes: stored.allocated_bytes, files: stored.files, last_used: "unknown" }] : undefined,
        limitations: ["Measurements were taken before this turn; they cannot verify the current state."],
      };
      if (names && reference.target.kind === "process") evidence.process_names = { [reference.target.entity_ref]: "chrome.exe" };
      const message = { question: sanitize(asked), initial_observation: { selected_evidence: evidence } };
      const id = `selection-preview-${++previewSequence}`;
      selectionTickets.set(id, { reference, intent, question, conversationId, names, fileNames, asked });
      return { preview_id: id, message, selected_evidence: evidence, intent, measured_at_unix_ms: source.measuredAt, age_ms: age, context_revision: 0, file_name_preview: { names: [], scope: null }, model_routing: { mode: "configured" }, configured_model_id: cloud().model_id };
    },
    selection_send: async ({ reference, intent, question = null, conversationId = null, previewId }) => {
      const cloudRun = privacy.mode !== "local" && cloud().ready;
      const ticket = selectionTickets.get(previewId);
      if (!ticket) fail("preview_required", "Open the selected-evidence sharing preview before sending.");
      if (JSON.stringify(ticket.reference) !== JSON.stringify(reference) || ticket.intent !== intent || ticket.question !== question || ticket.conversationId !== conversationId) {
        fail("preview_changed", "The selection, question, conversation, sharing or model settings changed. Open the preview again.");
      }
      selectionTickets.delete(previewId);
      pendingPreview = { id: previewId, question: ticket.asked, names: privacy.mode === "complete", conversationId };
      return commands.start_investigation({ request: { question: ticket.asked, cloud: cloudRun, names: privacy.mode === "complete", conversationId, previewId } });
    },
    send_message: async ({ conversationId = null, text, scopeIds = [], previewId = null }) => {
      const cloudRun = privacy.mode !== "local";
      if (scopeIds.length > 1) throw new Error("Attach one scanned folder to a message.");
      return commands.start_investigation({ request: { question: text, cloud: cloudRun, names: privacy.mode === "complete", conversationId, previewId } });
    },
    // ?turn=orphan: a turn the engine still runs after the window reloaded (current_turn).
    current_turn: () =>
      orphanTurn ? { turn_id: orphanTurn.turnId, conversation_id: null, question: orphanTurn.question, started_at_unix_ms: orphanTurn.startedAt, stopping: false, extension: null } : null,
    stop_turn: ({ turnId }) => {
      if (orphanTurn && orphanTurn.turnId === turnId) {
        orphanTurn = null;
        window.setTimeout(() => emit("turn:failed", { turn_id: turnId, conversation_id: null, code: "cancelled", message: "Stopped.", turn: null }), 300);
        return;
      }
      if (!running || running.turnId !== turnId || running.done) throw new Error("This turn is over.");
      running.stop = true;
      running.answer?.(false);
    },
    start_investigation: async ({ request: { question, cloud: cloudRun, names, conversationId = null, previewId } }) => {
      if (running && !running.done) fail("turn_running", "An investigation is already running.");
      if (cloudRun) {
        if (!pendingPreview || pendingPreview.id !== previewId || pendingPreview.question !== question || pendingPreview.names !== names || pendingPreview.conversationId !== conversationId) fail("preview_changed", "The question, conversation or sharing choice changed. Open the preview again.");
        pendingPreview = null;
        // ?preview=expired: the ticket outlived its five minutes, so nothing leaves.
        if (params.get("preview") === "expired") fail("preview_expired", "The preview expired. Open it again for a fresh measurement.");
      }
      const id = nextId++;
      let conversation = conversations.find((c) => c.id === conversationId);
      if (!conversation) {
        const now = Date.now();
        conversation = { id: `conv-${nextConversation++}`, title: question.split("\n")[0].slice(0, 80), created_at_unix_ms: now, updated_at_unix_ms: now, turns: [] };
        conversations.unshift(conversation);
      }
      running = { id, turnId: `turn-${nextTurn++}`, askedAt: Date.now(), stop: false, answer: null, done: false, conversationId: conversation.id, question };
      if (cloudRun) proposePlans(running.turnId, question);
      investigate(id, question, cloudRun).finally(() => {
        running.done = true;
      });
      return { id, conversation_id: conversation.id, turn_id: running.turnId };
    },
    conversations_list: async () =>
      conversations
        .slice()
        .sort((a, b) => b.updated_at_unix_ms - a.updated_at_unix_ms)
        .map(({ turns, ...c }) => ({ ...c, turn_count: turns.length, last_turn_status: turns.length && turns[turns.length - 1].report?.error ? "failed" : "completed" })),
    conversation_get: async ({ id }) => {
      const c = conversations.find((x) => x.id === id);
      return c ? { ...c, turn_count: c.turns.length, turns: c.turns.slice() } : null;
    },
    conversation_delete: async ({ id }) => {
      const i = conversations.findIndex((x) => x.id === id);
      if (i < 0) return false;
      conversations.splice(i, 1);
      return true;
    },
    conversations_clear: async () => {
      const n = conversations.length;
      conversations.length = 0;
      return n;
    },
    answer_extension: ({ accept }) => {
      running?.answer?.(accept);
    },
    stop_investigation: () => {
      if (running) {
        running.stop = true;
        running.answer?.(false);
      }
    },
    quick_folders: () => [
      { id: "downloads", path: "C:\\Users\\Alex\\Downloads" },
      { id: "desktop", path: "C:\\Users\\Alex\\Desktop" },
      { id: "documents", path: "C:\\Users\\Alex\\Documents" },
      { id: "temp", path: "C:\\Users\\Alex\\AppData\\Local\\Temp" },
      { id: "profile", path: "C:\\Users\\Alex" },
      { id: "system_drive", path: "C:\\" },
    ],
    pick_folder: async () => {
      await wait(200);
      return "C:\\Users\\Alex\\Downloads";
    },
    start_scan: ({ path }) => {
      // ?scan=denied: Windows refuses the folder before the scan starts.
      if (params.get("scan") === "denied") fail("folder_unreadable", `Cannot scan ${path}: Scope root cannot be accessed: PermissionDenied.`);
      scanId += 1;
      scanCancel = false;
      invalidateStorage();
      const scopeId = `scope-${scanId}`;
      lastScanRoot = path;
      scan(scopeId, path);
      return { scope_id: scopeId, root: path };
    },
    cancel_scan: () => {
      scanCancel = true;
    },
    scan_page: ({ scopeId, kind, offset }) => {
      if (scopeId !== `scope-${scanId}` || !scanFinishedAt) fail("scan_unavailable", "This scan is no longer available. Scan the folder again.");
      return publishPage(scopeId, page(kind, lastScanRoot, offset), scanFinishedAt);
    },
    reveal: () => null,
    // ?routing=calibrated makes Auto available, as a reviewed local calibration would.
    model_routing_get: () => routingView(),
    model_routing_set: ({ settings }) => {
      if (settings.mode === "automatic" && params.get("routing") !== "calibrated") fail("routing_unavailable", "Automatic routing requires a reviewed local Nemotron calibration.");
      routingSettings = { mode: settings.mode, super_model_id: settings.super_model_id ?? null, ultra_model_id: settings.ultra_model_id ?? null };
      return routingView();
    },
    // Elevated helper: one closed task at a time. ?elevation=fail declines Windows' administrator
    // prompt; ?elevation=slow keeps a deeper check running for 20 s (to see the running state and
    // Stop). Errors carry the codes of src/failure.rs, their Rust names only in the detail.
    elevation_status: () => (elevation ? { request_id: elevation.id, task: elevation.task, budget_ms: elevation.budget, state: elevation.state } : null),
    elevation_prepare: async ({ task, budgetMs }) => {
      if (elevation) fail("helper_busy", "An elevated inspection is already prepared or running.");
      const mft = task?.kind === "mft_inventory" && /^[A-Z]$/.test(task.volume) && Object.keys(task).length === 2;
      const check = Boolean(CHECK_RESULTS[task?.kind]) && Object.keys(task).length === 1;
      if (!mft && !check) fail("unsupported", "This elevated task is unsupported here (Unsupported).");
      if (!(budgetMs >= 1 && budgetMs <= 120000)) fail("unknown", "The elevated request was refused (InvalidRequest).");
      await wait(300);
      elevation = { id: `elevation-${Date.now()}`, task, budget: budgetMs, state: "awaiting_local_approval", cancelled: false };
      return { request_id: elevation.id, task, budget_ms: budgetMs, state: elevation.state };
    },
    elevation_run: async ({ requestId }) => {
      if (!elevation || elevation.id !== requestId || elevation.state !== "awaiting_local_approval") fail("request_expired", "This elevated inspection is no longer awaiting approval.");
      const job = elevation;
      job.state = "running";
      const slow = params.get("elevation") === "slow" && job.task.kind !== "mft_inventory";
      for (let waited = 0; waited < (slow ? 20000 : 2500) && !job.cancelled; waited += 100) await wait(100);
      const finish = () => {
        if (elevation === job) elevation = null;
      };
      if (job.cancelled) {
        finish();
        fail("cancelled", "The elevated task was cancelled (Cancelled).");
      }
      if (params.get("elevation") === "fail") {
        finish();
        fail("uac_declined", "The Windows administrator prompt was declined (UacDeclined).");
      }
      finish();
      return job.task.kind === "mft_inventory" ? mftResult(job.task.volume) : CHECK_RESULTS[job.task.kind]();
    },
    elevation_cancel: ({ requestId }) => {
      if (!elevation || elevation.id !== requestId) fail("request_expired", "This elevated inspection is no longer available.");
      if (elevation.state === "awaiting_local_approval") elevation = null;
      else {
        elevation.cancelled = true;
        elevation.state = "cancelling";
      }
    },
    save_cloud_settings: ({ apiKey = null, modelId, reasoning = null }) => {
      if (typeof modelId !== "string" || modelId.length > 200
        || !modelId.toLowerCase().startsWith("nvidia/") || !modelId.toLowerCase().includes("nemotron")
        || !/^[a-z0-9/._-]+$/i.test(modelId)) throw new Error("Select an explicit nvidia/ Nemotron model ID; no fallback model is allowed");
      if (apiKey?.trim()) cloudKeyConfigured = true;
      cloudMode = cloudKeyConfigured ? "ready" : "none";
      cloudModel = modelId;
      if (reasoning) {
        if (!["on", "low", "off"].includes(reasoning)) throw new Error("unknown reasoning");
        cloudReasoning = reasoning;
      }
      return cloud();
    },
    test_cloud: async () => {
      await checkConnection();
      if (!cloudModel) throw new Error("NEBIUS_MODEL_ID is missing or empty");
      if (params.get("connection") === "empty" || cloudMode === "problem") throw new Error("The selected model is absent from the account catalog");
      return { price: { model_id: cloudModel, input_per_million: "0.30", output_per_million: "0.90", read_at_unix_ms: Date.now() } };
    },
    cloud_models: async () => {
      await checkConnection();
      return {
        source: "synthetic-mock", read_at_unix_ms: Date.now(),
        models: params.get("connection") === "empty" ? [] : [
          { id: "nvidia/nemotron-3-super-120b-a12b", supports_tools: true },
          { id: "nvidia/mock-nemotron-account-model", supports_tools: null },
          { id: "nvidia/mock-nemotron-no-tools", supports_tools: false },
        ],
      };
    },
    open_nebius_console: () => null,
    save_search_key: ({ apiKey = null }) => {
      const key = (apiKey || "").trim();
      if (key && !key.startsWith("tvly-")) fail("search_key_invalid", "A Tavily key starts with tvly-.");
      searchKey = key || null;
      return cloud();
    },
    // The Rust side reads the address from the stored turn and opens it in the browser; the mock
    // only records the request.
    open_web_source: ({ conversationId, turnId, reference }) => {
      const conversation = conversations.find((c) => c.id === conversationId);
      if (!conversation) fail("web_source_unavailable", "This conversation no longer exists.");
      const turn = conversation.turns.find((x) => x.id === turnId);
      if (!turn) fail("web_source_unavailable", "This answer no longer exists.");
      const cited = (turn.report?.answer?.citations || []).find((c) => c.reference === reference && c.kind === "web");
      if (!cited?.web?.url) fail("web_source_unavailable", "This answer has no such web source.");
      openedWebSources.push({ conversationId, turnId, reference, at: Date.now() });
      return null;
    },
  };
  let lastScanRoot = null;
  return {
    mock: true,
    invoke: async (command, args = {}) => {
      const fn = commands[command];
      if (!fn) throw new Error(`Unknown command ${command}`);
      return fn(args);
    },
    listen: (name, cb) => {
      if (!listeners.has(name)) listeners.set(name, new Set());
      listeners.get(name).add(cb);
      return Promise.resolve(() => listeners.get(name).delete(cb));
    },
    onDrop: () => () => {},
  };
}
