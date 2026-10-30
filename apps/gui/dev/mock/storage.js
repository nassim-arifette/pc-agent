import { wait, fail, GiB, MiB, fmt, jitter } from "./common.js";

export function createStorageFixture({ params, emit, publishSource, invalidateStorage }) {
  let scanId = 0;

  let scanCancel = false;
  const indexed = new Map();

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
        item_ref: `scan-${scanId}:${kind}:${n + 1}`,
        parent_ref: `scan-${scanId}:root`,
        kind,
        system: kind === "dirs" && n === 0 && /^[A-Za-z]:\\?$/.test(root),
        name,
        parent,
        path: `${root}\\${parent ? `${parent}\\` : ""}${name}`,
        logical_bytes: unallocated ? size : Math.round(size * 0.995),
        allocated_bytes: unallocated ? null : size,
        size,
        size_text: fmt(size),
        files: kind === "dirs" ? Math.max(1, 1200 - n * 90) : null,
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
      scanned_at_unix_ms: Date.now(),
      scanned: { logical_bytes: 212 * GiB, allocated_bytes: 208 * GiB, allocation_unknown_files: 1, files: entries },
      scanned_text: "208.00 GiB",
      // The same C: as Live PC's mock (255 GiB, almost full unless ?disk=ok).
      volume: { root: "C:\\", fixed: true, total_bytes: 255 * GiB, free_bytes: (params.get("disk") === "ok" ? 61 : 0.3) * GiB, total_text: "255.00 GiB", free_text: params.get("disk") === "ok" ? "61.00 GiB" : "307.2 MiB", filesystem: "NTFS" },
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
        indexed.clear();
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
      scan_children: ({ scopeId, itemRef, kind, offset = 0 }) => {
        if (scopeId !== `scope-${scanId}` || !scanFinishedAt) fail("scan_unavailable", "This scan is no longer available.");
        const root = { item_ref: `scan-${scanId}:root`, name: lastScanRoot, path: lastScanRoot,
          kind: "dirs", size: 208 * GiB, allocated_bytes: 208 * GiB, logical_bytes: 212 * GiB, status: "partial" };
        const parent = itemRef ? indexed.get(itemRef) : root;
        if (!parent || parent.kind !== "dirs") fail("item_unavailable", "This folder is unavailable.");
        const depth = parent.depth || 0;
        const total = depth === 0 ? (kind === "dirs" ? 64 : 180) : kind === "dirs" && depth >= 3 ? 0 : 6;
        const count = Math.max(0, Math.min(50, total - offset));
        const children = items(kind, parent.path, count, offset).map((item, i) => {
          const n = offset + i;
          const size = depth ? Math.round(parent.size * 0.22 * 0.6 ** n) : item.size;
          const child = { ...item, item_ref: `${parent.item_ref}/${kind}:${n}`, parent_ref: parent.item_ref,
            parent: parent.name, path: `${parent.path.replace(/\\$/, "")}\\${item.name}`,
            size, allocated_bytes: item.allocated_bytes == null ? null : size, logical_bytes: size,
            depth: depth + 1 };
          indexed.set(child.item_ref, child);
          return child;
        });
        return { kind, items: children, parent, total, selection_source_id: null,
          next_offset: offset + count < total ? offset + count : null,
          parent_partial: parent.status !== "ok",
          other_bytes: Math.max(0, parent.size - children.reduce((n, item) => n + item.size, 0)) };
      },
      scan_item_facts: async ({ scopeId, itemRef }) => {
        if (scopeId !== `scope-${scanId}` || !scanFinishedAt) fail("scan_unavailable", "This scan is no longer available.");
        const item = indexed.get(itemRef);
        if (!item) fail("item_unavailable", "This item is unavailable.");
        const count = item.kind === "dirs" ? item.files : 1;
        const measured = (files, fraction) => ({ files_count: files, logical_bytes: Math.round(item.size * fraction),
          allocated_bytes: item.allocated_bytes == null ? null : Math.round(item.size * fraction),
          allocation_unknown_files_count: item.allocated_bytes == null ? 1 : 0 });
        const shown = publishPage(scopeId, { kind: item.kind, items: [item], total: 1, next_offset: null }, scanFinishedAt);
        return { item, selection_source_id: shown.selection_source_id, parent_ref: item.parent_ref,
          parent_alias: "dir-parent", rank_scope: "siblings_of_parent", volume_total_bytes: 255 * GiB,
          item_facts: { scanned_at_unix_ms: scanFinishedAt, untouched_before_unix_ms: scanFinishedAt - 182 * 86400000,
            files_count: count, untouched: measured(Math.floor(count / 2), count > 1 ? 0.45 : 0), undated_files_count: item.kind === "dirs" ? Math.min(2, count) : 0,
            largest_file: item.kind === "files" ? { ...item, alias: "file-largest" }
              : { alias: "file-largest", path: `${item.path}\\archive.zip`, allocated_bytes: Math.round(item.size / 3), logical_bytes: Math.round(item.size / 3) },
            file_types: count === 1 ? [{ file_type: "archives", ...measured(1, 1) }]
              : [{ file_type: "archives", ...measured(Math.floor(count / 2), 0.6) },
                { file_type: "documents", ...measured(count - Math.floor(count / 2), 0.4) }],
            status: item.status, limitations: [] } };
      },
      reveal: () => null
    };
    return { commands, getRoot: () => lastScanRoot };
  }

  return { createCommands };
}
