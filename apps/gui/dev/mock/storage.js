import { wait, fail, GiB, MiB, fmt, jitter } from "./common.js";

export function createStorageFixture({ params, emit, publishSource, invalidateStorage }) {
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
      // The size on disk is a little above the data; the second file's could not be read, so it is
      // ranked by its data and marked partial, as the scanner does (Storage inspector).
      const unallocated = kind === "files" && n === 1;
      return {
        item_ref: `scan-${scanId}:${n + 1}`,
        name,
        parent,
        path: `${root}\\${parent ? `${parent}\\` : ""}${name}`,
        logical_bytes: unallocated ? size : Math.round(size * 0.995),
        allocated_bytes: unallocated ? null : size,
        size,
        size_text: fmt(size),
        files: kind === "dirs" ? 1200 - n * 90 : null,
        modified_unix_ms: Date.now() - (n + 1) * 86400000 * 9,
        status: (n === 3 && kind === "dirs") || unallocated ? "partial" : "ok",
      };
    });
  }

  function page(kind, root, offset) {
    const total = kind === "dirs" ? 64 : 180;
    const count = Math.min(50, total - offset);
    return { kind, items: items(kind, root, count, offset), total, next_offset: offset + count < total ? offset + count : null };
  }

  function publishPage(scopeId, shown, measuredAt) {
    const targets = shown.items.map((item) => ({ kind: "storage", scope_id: scopeId, item_ref: item.item_ref }));
    const storage = Object.fromEntries(shown.items.map((item, i) => [item.item_ref, { kind: shown.kind, alias: `${shown.kind === "dirs" ? "dir" : "file"}-${i + 1}`, allocated_bytes: item.allocated_bytes, files: item.files }]));
    return { ...shown, selection_source_id: targets.length ? publishSource(targets, measuredAt, { storage }) : null };
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
      scanned: { logical_bytes: 212 * GiB, allocated_bytes: 208 * GiB, allocation_unknown_files: 1, files: entries },
      scanned_text: "208.00 GiB",
      volume: { total_bytes: 476 * GiB, free_bytes: 61 * GiB, total_text: "476.00 GiB", free_text: "61.00 GiB", filesystem: "NTFS" },
      coverage: { files: entries, dirs: Math.round(entries / 9), denied_dirs: 3, denied_files: 0, reparse_skipped: 12, placeholders: 41, placeholder_logical_bytes: 3 * GiB, other_volume_skipped: 0, special_skipped: 0, vanished: 0, changed_during_scan: 5, errors: scanCancel ? 0 : 2, hard_link_aliases: 0, allocation_unknown_files: 1, truncated: null, cancelled: scanCancel },
      denied: [`${root}\\Private`],
      dirs: publishPage(scopeId, page("dirs", root, 0), (scanFinishedAt = Date.now())),
      files: publishPage(scopeId, page("files", root, 0), scanFinishedAt),
      limitations: ["Modification times are not last-use times; last use is unknown."],
    });
  }

  function createCommands() {
    let lastScanRoot = null;
    const commands = {
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
      reveal: () => null
    };
    return { commands, getRoot: () => lastScanRoot };
  }

  return { createCommands };
}
