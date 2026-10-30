import { wait, fail, GiB, fmt, m, missing, absent, jitter, volume } from "./common.js";
import { createBatteryFixture } from "./battery.js";
import { browserFixture, localBrowserFixture } from "./browser.js";

export function createLiveFixture({ params, publishSource, cloud }) {
  const DESKTOP = params.get("pc") === "desktop";

  const NO_NTFS = params.get("ntfs") === "none";

  let probed = false;

  let tick = 0;
  const battery = createBatteryFixture({ params, getTick: () => tick, publishSource });
  const { liveBatteries, mode: BATTERY_MODE } = battery;
  function capabilities() {
    const known = (value) => (probed ? value : null);
    return {
      windows: true,
      history_sampler: true,
      elevated_helper: params.get("helper") !== "missing",
      nvidia_gpu: known(params.get("gpu") !== "none" && !DESKTOP),
      battery: known(BATTERY_MODE !== "none"),
      ntfs_volumes: known(!NO_NTFS),
    };
  }

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
      params.get("gpu") === "none" || DESKTOP
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
        volume("C:\\", 255, params.get("disk") === "ok" ? 61 : 0.3, NO_NTFS ? "ReFS" : "NTFS"),
        volume("D:\\", 476, 135, NO_NTFS || DESKTOP ? "exFAT" : "NTFS"),
      ],
      ...liveBatteries(),
      limitations: ["Working sets can share pages; they do not add up to physical memory in use."],
    };
  }

  const commands = {
    local_browser_windows: async ({ applicationRef, includeTabs }) => {
      await wait(300);
      if (!["app-1", "app-7"].includes(applicationRef)) return { status: "stale", windows: [] };
      return localBrowserFixture(applicationRef, includeTabs, params.get("browser") === "partial");
    },
    app_info: () => ({ version: "0.1.2", cloud: cloud(), models: [
      { id: "nvidia/nemotron-3-super-120b-a12b", note: "recommended" },
      { id: "nvidia/Nemotron-3-Ultra-550b-a55b", note: "most_capable" },
      { id: "nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B", note: "cheaper_slower" },
      { id: "nvidia/Nemotron-3_5-Lightning", note: "not_recommended" },
    ], capabilities: capabilities() }),
    dashboard: async () => {
      await wait(tick === 0 ? 700 : 120);
      if (params.get("live") === "fail") fail("live_unavailable", "Measurements took too long.");
      probed = true;
      return dashboard();
    },
    process_activity: async () => {
      await wait(600);
      const now = Date.now();
      const window = { started_at_unix_ms: now - 2000, finished_at_unix_ms: now, duration_ms: 2000 };
      // Readable names as the executables' version resources give them; svchost.exe has none.
      const READABLE = { "chrome.exe": "Google Chrome", "Code.exe": "Visual Studio Code", "Teams.exe": "Microsoft Teams" };
      const app = (ref, name, count, cpu, ws, members, services = []) => ({
        entity_ref: ref, name, readable_name: READABLE[name] ?? null, process_count: count, members_truncated: false,
        cpu: m(cpu, `${(cpu / 100).toFixed(1)}%`), working_set: m(ws * GiB, fmt(ws * GiB)), private_commit: m(ws * 0.8 * GiB, fmt(ws * 0.8 * GiB)),
        working_set_increase: ref === "app-1" ? m(0.2 * GiB, fmt(0.2 * GiB)) : m(0, "0"), io_read: m(120000, "117 KiB/s"), io_write: m(40000, "39 KiB/s"),
        gpu: ref === "app-1" ? m(310, "3.1%") : missing("unsupported", "No GPU counter instance for these processes."),
        services,
        members: members.map(([pid, mcpu, mws], i) => ({ entity_ref: `proc-${ref.slice(4)}p${i}`, name, readable_name: READABLE[name] ?? null, pid, cpu: m(mcpu, ""), working_set: m(mws * GiB, ""), services: [] })),
      });
      const applications = [
          browserFixture("app-1", "chrome.exe", "chrome"),
          browserFixture("app-7", "firefox.exe", "firefox"),
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
    }
  };
  return { commands, batteryMode: BATTERY_MODE };
}
