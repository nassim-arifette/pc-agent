// Design-time stand-in for the Rust side, used when ui/index.html is opened in a plain browser
// (python -m http.server in apps/gui, then /ui/index.html). Every value is synthetic. The app
// never loads this file: it is outside the embedded ui folder.
//
// URL switches: ?cloud=none|ready|problem, ?gpu=none. Questions containing "extend" ask for a
// budget extension; "fail" makes the model's answer unusable; "slow" stretches the steps.

const params = new URLSearchParams(window.location.search);
const listeners = new Map();
const wait = (ms) => new Promise((resolve) => window.setTimeout(resolve, ms));
const GiB = 1024 ** 3;
const MiB = 1024 ** 2;

function emit(name, payload) {
  for (const cb of listeners.get(name) || []) cb(payload);
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
function cloud() {
  if (cloudMode === "none") {
    return { key_configured: false, key_hint: null, key_source: null, model_id: null, model_source: null, ready: false, problem: "NEBIUS_API_KEY is missing or empty", file: "C:\\Users\\Alex\\AppData\\Roaming\\pcagent\\.env", file_problem: null };
  }
  if (cloudMode === "problem") {
    return { key_configured: true, key_hint: "Q7fz", key_source: "file", model_id: "meta/llama", model_source: "file", ready: false, problem: "Select an explicit nvidia/ Nemotron model ID; no fallback model is allowed", file: "C:\\Users\\Alex\\AppData\\Roaming\\pcagent\\.env", file_problem: null };
  }
  return { key_configured: true, key_hint: "Q7fz", key_source: "file", model_id: "nvidia/nemotron-3-super-120b-a12b", model_source: "file", ready: true, problem: null, file: "C:\\Users\\Alex\\AppData\\Roaming\\pcagent\\.env", file_problem: null };
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
  return {
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
    limitations: ["Working sets can share pages; they do not add up to physical memory in use."],
  };
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

// A written answer as the model returns it, in the language of the question.
function mockAnswer(question) {
  const french = /pourquoi|mon |est-ce|[éèàç]/i.test(question);
  const text = french
    ? "Ton PC rame surtout parce que la mémoire vive est presque pleine : il ne restait que 1,54 Gio de libre sur 15,73 Gio [O1], et Windows relisait 12,4 pages par seconde depuis le disque pour compenser [F2]. J'en suis moyennement sûr : c'est une mesure d'une seconde.\n\nCe que tu peux faire :\n1. Ferme les onglets de **chrome.exe (PID 4000)** dont tu n'as pas besoin : il occupe 2,31 Gio [O1].\n2. Garde PC Agent ouvert pendant le prochain ralentissement et repose la question, pour confirmer.\n\nLe processeur n'était pas saturé (18,5 % [O1]) et le disque répondait en moins de 3 ms [O1] : ils ne semblent pas en cause."
    : "Your PC is slow mainly because memory is almost full: only 1.54 GiB of 15.73 GiB was available [O1], and Windows read 12.4 pages per second back from disk to compensate [F2]. I'm moderately sure: this is a one-second measurement.\n\nWhat you can do:\n1. Close the **chrome.exe (PID 4000)** tabs you don't need: it holds 2.31 GiB [O1].\n2. Keep PC Agent open during the next slowdown and ask again to confirm.\n\nThe processor was not saturated (18.5% [O1]) and the disk answered in under 3 ms [O1], so they don't look involved.";
  return {
    text,
    confidence: "medium",
    category: "memory",
    citations: [
      { reference: "O1", kind: "observation", title: "get_system_snapshot", facts: ["Available physical memory: 1.54 GiB of 15.73 GiB (9.8%).", "chrome.exe (PID 4000) used 2.31 GiB of RAM (working set).", "CPU busy: 18.5%.", "Disk read response time: 1.3 ms."] },
      { reference: "F2", kind: "finding", title: "Heavy hard-fault paging", facts: ["12.4 pages per second were read from disk to resolve hard faults."] },
    ],
    follow_ups: french
      ? ["Quels onglets ou extensions utilisent le plus de mémoire ?", "Ai-je besoin de plus de RAM ?"]
      : ["Which tabs or extensions use the most memory?", "Do I need more RAM?"],
    unverified: [],
    repairs: 0,
  };
}

function report(question, cloudRun, error = null) {
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
    answer: cloudRun && !error ? mockAnswer(question) : null,
    observations: cloudRun && !error
      ? [{ tool: "get_metric_history", status: "ok", facts: ["CPU busy: 12 usable samples, observed range 9.5% to 31.0%.", "Observed interval: 60000 ms."], limitations: [] }]
      : [],
    limitations: ["Short observations cannot prove a cause, a leak or a hardware purchase need."],
    usage: cloudRun
      ? {
          requests: error === "output_limit" ? 1 : 3,
          tool_calls: 2,
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

let nextId = 1;
// Saved conversations, as the Rust side keeps them in SQLite.
const conversations = [];
let nextConversation = 1;
function saveTurn(conversationId, question, report) {
  const c = conversations.find((x) => x.id === conversationId);
  if (!c) return null;
  c.turns.push({ id: `turn-${c.turns.length + 1}`, question, asked_at_unix_ms: Date.now(), report });
  c.updated_at_unix_ms = Date.now();
  return c.id;
}
function done(id, report) {
  const conversationId = running && running.id === id ? saveTurn(running.conversationId, running.question, report) : null;
  return emit("inv:done", { id, report, conversation_id: conversationId });
}
let running = null;
let pendingPreview = null;
let previewSequence = 0;

async function investigate(id, question, cloudRun) {
  const slow = /slow|lent/i.test(question) ? 1.6 : 1;
  const step = (s) => emit("inv:step", { id, at_unix_ms: Date.now(), step: s });
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
  const tool = async (name, ms, status = "ok") => {
    step({ kind: "tool_started", tool: name });
    await wait(ms * slow);
    step({ kind: "tool_finished", tool: name, status, duration_ms: ms });
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
  if (/extend|prolong/i.test(question)) {
    const answer = new Promise((resolve) => {
      running.answer = resolve;
    });
    emit("inv:extension", {
      id,
      request: { reason: "rounds", available: true, additional_rounds: 6, additional_tool_calls: 12, additional_seconds: 90 },
      usage: { requests: 6, tool_calls: 6, extensions: 0, prompt_tokens: 30210, completion_tokens: 812, reasoning_tokens: 600, cost: { usd: "0.0098", exact_usd: "0.0097938", price: null } },
    });
    const accepted = await answer;
    if (!accepted) return done(id, { ...report(question, true, "budget"), execution: "budget_exhausted" });
  }
  if (running.stop) return done(id, report(question, true, "cancelled"));
  await model(1300, 8969, 271);
  done(id, report(question, true));
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
async function scan(scopeId, root) {
  const started = Date.now();
  let entries = 0;
  while (Date.now() - started < 2600) {
    if (scanCancel) break;
    entries += Math.round(jitter(4200, 800));
    emit("scan:progress", { scope_id: scopeId, entries, dirs: Math.round(entries / 9), bytes: entries * 1.9 * MiB, bytes_text: fmt(entries * 1.9 * MiB), elapsed_ms: Date.now() - started });
    await wait(150);
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
    dirs: page("dirs", root, 0),
    files: page("files", root, 0),
    limitations: ["Modification times are not last-use times; last use is unknown."],
  });
}

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
      return dashboard();
    },
    cloud_preview: async ({ question, names, conversationId = null }) => {
      const sequence = ++previewSequence;
      await wait(300);
      const message = { question, initial_observation: { schema_version: 1, origin: "synthetic_fixture", measurements: { values: { system: { physical_total_bytes: 16888365056, physical_available_bytes: 1653562000, cpu_busy_basis_points: 1850 }, "proc-1a2b3c4d": { process_working_set_bytes: 2480000000 } }, units: { physical_total_bytes: "bytes" } } }, findings: [{ id: "F1", classification: "low_available_memory" }], current_finding_ids: ["F1", "F2", "F3"] };
      if (names) message.process_names = { "proc-1a2b3c4d": "chrome.exe" };
      const conversation = conversations.find((c) => c.id === conversationId);
      if (conversation?.turns.length) message.previous_turns = conversation.turns.map((turn) => ({ question: turn.question, answer: "Earlier synthetic observation.", observations: [] }));
      if (sequence !== previewSequence) throw new Error("A newer sharing preview was requested.");
      const id = `preview-${sequence}`;
      pendingPreview = { id, question, names, conversationId };
      return { id, message, bytes: JSON.stringify(message).length, measured: 38, findings: 3, names: names ? 1 : 0, storage_folder: lastScanRoot };
    },
    start_investigation: async ({ request: { question, cloud: cloudRun, names, conversationId = null, previewId } }) => {
      if (running && !running.done) throw new Error("An investigation is already running.");
      if (cloudRun) {
        if (!pendingPreview || pendingPreview.id !== previewId || pendingPreview.question !== question || pendingPreview.names !== names || pendingPreview.conversationId !== conversationId) throw new Error("Open the sharing preview again.");
        pendingPreview = null;
      }
      const id = nextId++;
      let conversation = conversations.find((c) => c.id === conversationId);
      if (!conversation) {
        const now = Date.now();
        conversation = { id: `conv-${nextConversation++}`, title: question.split("\n")[0].slice(0, 80), created_at_unix_ms: now, updated_at_unix_ms: now, turns: [] };
        conversations.unshift(conversation);
      }
      running = { id, stop: false, answer: null, done: false, conversationId: conversation.id, question };
      investigate(id, question, cloudRun).finally(() => {
        running.done = true;
      });
      return { id, conversation_id: conversation.id };
    },
    conversations_list: async () =>
      conversations
        .slice()
        .sort((a, b) => b.updated_at_unix_ms - a.updated_at_unix_ms)
        .map(({ turns, ...c }) => ({ ...c, turn_count: turns.length })),
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
      scanId += 1;
      scanCancel = false;
      const scopeId = `scope-${scanId}`;
      lastScanRoot = path;
      scan(scopeId, path);
      return { scope_id: scopeId, root: path };
    },
    cancel_scan: () => {
      scanCancel = true;
    },
    scan_page: ({ kind, offset }) => page(kind, lastScanRoot, offset),
    reveal: () => null,
    save_cloud_settings: ({ modelId }) => {
      cloudMode = modelId.startsWith("nvidia/") ? "ready" : "problem";
      return cloud();
    },
    test_cloud: async () => {
      await wait(800);
      return { price: { model_id: "nvidia/nemotron-3-super-120b-a12b", input_per_million: "0.30", output_per_million: "0.90", read_at_unix_ms: Date.now() } };
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
