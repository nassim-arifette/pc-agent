// What the Storage view says about a scanned folder or file, computed only from what the page
// already has (the scan's view and its pages). No DOM and no IPC, so dev/storage.test.mjs runs it
// in Node. An item's size is the list's: its space on disk when known, else the size of its data.

/** The last segment of a path: "Downloads" for C:\Users\Alex\Downloads, "C:" for a drive root. */
export function folderName(path) {
  const trimmed = (path || "").replace(/[\\/]+$/, "");
  return trimmed.split(/[\\/]/).pop() || trimmed;
}

export function isDriveRoot(path) {
  return /^[A-Za-z]:\\?$/.test(path || "");
}

/** The drive letter of a scanned folder ("C:"), or null for another kind of path (a share). */
export function driveLetter(root) {
  const match = /^([A-Za-z]):/.exec(root || "");
  return match ? `${match[1].toUpperCase()}:` : null;
}

/**
 * The item's share of the scanned folder, measured like the item's size in the list: its space on
 * disk over the folder's when the item's is known, else its data over the folder's (a cloud file,
 * for example). Never one measure over the other: a folder of cloud files takes little space for
 * a lot of data. Null when the folder's total is unknown, or when the ratio would pass 100 %
 * (inconsistent totals are not shown as a whole folder).
 */
export function folderShare(item, scanned) {
  if (!item || !scanned) return null;
  const onDisk = item.allocated_bytes != null;
  const part = onDisk ? item.allocated_bytes : item.logical_bytes;
  const whole = onDisk ? scanned.allocated_bytes : scanned.logical_bytes;
  if (!(whole > 0) || !Number.isFinite(part) || part < 0) return null;
  const ratio = part / whole;
  return ratio <= 1 ? ratio : null;
}

/**
 * The item's share of its drive's capacity, from its known space on disk only: a size whose
 * allocation is unknown (a cloud file, for example) may not occupy the drive at all.
 */
export function driveShare(item, volume) {
  const total = volume?.total_bytes;
  const used = item?.allocated_bytes;
  if (!(total > 0) || used == null || used < 0) return null;
  const ratio = used / total;
  return ratio <= 1 ? ratio : null;
}

/** How a share is shown: one decimal under 10 %, and never rounded down to zero. */
export function shareShown(ratio) {
  if (ratio == null || !Number.isFinite(ratio) || ratio < 0) return null;
  if (ratio === 0) return { ratio: 0, digits: 0, below: false };
  if (ratio < 0.001) return { ratio: 0.001, digits: 1, below: true };
  return { ratio, digits: ratio < 0.1 ? 1 : 0, below: false };
}

/** The item's coverage: "ok", "denied" (not allowed to read all of it) or "partial". */
export function itemCoverage(item) {
  if (item?.status === "ok") return "ok";
  return item?.status === "permission_denied" ? "denied" : "partial";
}

/** The i18n key of the sentence that explains an incomplete item; null for a complete one. */
export function coverageNote(kind, item) {
  const coverage = itemCoverage(item);
  if (coverage === "ok") return null;
  if (coverage === "denied") return "storage.insp.note.denied";
  if (kind === "dirs") return "storage.itemPartial";
  return item.allocated_bytes == null ? "storage.insp.note.noAllocation" : "storage.insp.note.partial";
}

/**
 * The inspector's facts about one item of `view` (a scan view) as data, in display order:
 * { key, vars, value } with value { bytes } | { count } | { share } | { date } | null, where null
 * is a value the scan did not give (shown as unknown, never as zero). A share that cannot be
 * computed is left out. The last use is always unknown: a scan reads modification dates only.
 */
export function itemFacts(kind, item, view) {
  const rows = [
    { key: "storage.insp.onDisk", value: item.allocated_bytes != null ? { bytes: item.allocated_bytes } : null },
    { key: "storage.insp.logical", value: Number.isFinite(item.logical_bytes) ? { bytes: item.logical_bytes } : null },
  ];
  if (kind === "dirs") rows.push({ key: "storage.insp.files", value: item.files != null ? { count: item.files } : null });
  const inFolder = folderShare(item, view.scanned);
  // A whole drive's scan is not its capacity: its share is of what the scan measured.
  if (inFolder != null) {
    rows.push(isDriveRoot(view.root) ? { key: "storage.insp.shareScanned", value: { share: inFolder } } : { key: "storage.insp.shareFolder", vars: { folder: folderName(view.root) }, value: { share: inFolder } });
  }
  const onDrive = driveShare(item, view.volume);
  const drive = driveLetter(view.root);
  if (onDrive != null) rows.push({ key: drive ? "storage.insp.shareDrive" : "storage.insp.shareDriveAny", vars: drive ? { drive } : undefined, value: { share: onDrive } });
  rows.push({ key: "storage.insp.modified", value: item.modified_unix_ms != null ? { date: item.modified_unix_ms } : null });
  rows.push({ key: "storage.insp.used", value: null });
  return rows;
}

/**
 * The i18n keys of the notes under the facts: why the item is incomplete, then, when the scan was
 * partial and a share is shown, that the shares count only what the scan could read.
 */
export function itemNotes(kind, item, view, facts = itemFacts(kind, item, view)) {
  const notes = [];
  const coverage = coverageNote(kind, item);
  if (coverage) notes.push(coverage);
  if (view.status !== "ok" && facts.some((row) => row.value && "share" in row.value)) notes.push("storage.insp.scanPartial");
  return notes;
}
