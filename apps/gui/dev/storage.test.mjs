// Logic tests of the Storage inspector on synthetic scan data, not a browser test.
// node --experimental-default-type=module --test apps/gui/dev/storage.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import { coverageNote, driveLetter, driveShare, folderName, folderShare, isDriveRoot, itemCoverage, itemFacts, itemNotes, shareShown } from "../ui/js/storage/items.js";

const GiB = 1024 ** 3;
// A scan view as the host sends it (view.rs StorageView), reduced to what the inspector reads.
const view = (patch = {}) => ({
  scope_id: "scope-1",
  root: "C:\\Users\\Alex\\Downloads",
  status: "ok",
  scanned: { logical_bytes: 200 * GiB, allocated_bytes: 210 * GiB, allocation_unknown_files: 0, files: 5000 },
  volume: { total_bytes: 1000 * GiB, free_bytes: 300 * GiB },
  ...patch,
});
const folder = (patch = {}) => ({ item_ref: "scan-1:1", name: "Games", parent: "", path: "C:\\Users\\Alex\\Downloads\\Games", logical_bytes: 40 * GiB, allocated_bytes: 42 * GiB, size: 42 * GiB, files: 1200, modified_unix_ms: 1_760_000_000_000, status: "ok", ...patch });
const file = (patch = {}) => ({ item_ref: "scan-1:9", name: "holiday.mp4", parent: "Videos", path: "C:\\Users\\Alex\\Downloads\\Videos\\holiday.mp4", logical_bytes: 7 * GiB, allocated_bytes: 7 * GiB, size: 7 * GiB, files: null, modified_unix_ms: 1_760_000_000_000, status: "ok", ...patch });
const keys = (rows) => rows.map((row) => row.key);

test("paths name the folder and the drive without reading anything", () => {
  assert.equal(folderName("C:\\Users\\Alex\\Downloads"), "Downloads");
  assert.equal(folderName("C:\\Users\\Alex\\Downloads\\"), "Downloads");
  assert.equal(folderName("C:\\"), "C:");
  assert.equal(folderName(""), "");
  assert.equal(isDriveRoot("C:\\"), true);
  assert.equal(isDriveRoot("d:"), true);
  assert.equal(isDriveRoot("C:\\Users"), false);
  assert.equal(driveLetter("c:\\Users\\Alex"), "C:");
  assert.equal(driveLetter("D:\\"), "D:");
  assert.equal(driveLetter("\\\\server\\share\\folder"), null);
  assert.equal(driveLetter(null), null);
});

test("the folder share compares the same measure: space on disk, or data when the item's space is unknown", () => {
  assert.equal(folderShare(folder(), view().scanned), 42 / 210);
  // An item whose allocation is unknown is listed by its data, and shared over the folder's data.
  assert.equal(folderShare(file({ allocated_bytes: null }), view().scanned), 7 / 200);
  // Cloud files: 1 GiB on disk for 10 GiB of data. A folder holding half of each is half of it,
  // never 0.5 GiB over 10 GiB.
  const cloud = { logical_bytes: 10 * GiB, allocated_bytes: 1 * GiB, allocation_unknown_files: 0, files: 10 };
  assert.equal(folderShare(folder({ logical_bytes: 5 * GiB, allocated_bytes: 0.5 * GiB, size: 0.5 * GiB }), cloud), 0.5);
  assert.equal(folderShare(folder(), null), null);
  assert.equal(folderShare(folder(), { logical_bytes: 0, allocated_bytes: 0 }), null);
  assert.equal(folderShare(file({ allocated_bytes: null, logical_bytes: Number.NaN }), view().scanned), null);
  // Inconsistent totals are not shown as a whole folder.
  assert.equal(folderShare(folder({ allocated_bytes: 300 * GiB }), view().scanned), null);
});

test("the drive share uses the known space on disk only", () => {
  assert.equal(driveShare(folder(), view().volume), 42 / 1000);
  // A cloud file's data may not occupy the drive at all: no share rather than a guess.
  assert.equal(driveShare(file({ allocated_bytes: null }), view().volume), null);
  assert.equal(driveShare(folder(), { total_bytes: null, free_bytes: null }), null);
  assert.equal(driveShare(folder(), { total_bytes: 0 }), null);
  assert.equal(driveShare(folder(), undefined), null);
});

test("a share keeps a decimal under 10 % and is never rounded down to zero", () => {
  assert.deepEqual(shareShown(0.234), { ratio: 0.234, digits: 0, below: false });
  assert.deepEqual(shareShown(0.042), { ratio: 0.042, digits: 1, below: false });
  assert.deepEqual(shareShown(0.0004), { ratio: 0.001, digits: 1, below: true });
  assert.deepEqual(shareShown(0), { ratio: 0, digits: 0, below: false });
  assert.equal(shareShown(null), null);
  assert.equal(shareShown(Number.NaN), null);
});

test("coverage: complete, partial or denied, with the sentence that explains it", () => {
  assert.equal(itemCoverage(folder()), "ok");
  assert.equal(coverageNote("dirs", folder()), null);
  assert.equal(itemCoverage(folder({ status: "partial" })), "partial");
  assert.equal(coverageNote("dirs", folder({ status: "partial" })), "storage.itemPartial");
  assert.equal(itemCoverage(folder({ status: "permission_denied" })), "denied");
  assert.equal(coverageNote("dirs", folder({ status: "permission_denied" })), "storage.insp.note.denied");
  assert.equal(coverageNote("files", file({ status: "partial", allocated_bytes: null })), "storage.insp.note.noAllocation");
  assert.equal(coverageNote("files", file({ status: "partial" })), "storage.insp.note.partial");
  // Any other status is not presented as complete.
  assert.equal(itemCoverage(file({ status: "error" })), "partial");
});

test("a folder's facts: both sizes, its files, both shares, its modification date and an unknown last use", () => {
  const rows = itemFacts("dirs", folder(), view());
  assert.deepEqual(keys(rows), ["storage.insp.onDisk", "storage.insp.logical", "storage.insp.files", "storage.insp.shareFolder", "storage.insp.shareDrive", "storage.insp.modified", "storage.insp.used"]);
  assert.deepEqual(rows[0].value, { bytes: 42 * GiB });
  assert.deepEqual(rows[1].value, { bytes: 40 * GiB });
  assert.deepEqual(rows[2].value, { count: 1200 });
  assert.deepEqual(rows[3], { key: "storage.insp.shareFolder", vars: { folder: "Downloads" }, value: { share: 42 / 210 } });
  assert.deepEqual(rows[4], { key: "storage.insp.shareDrive", vars: { drive: "C:" }, value: { share: 42 / 1000 } });
  assert.deepEqual(rows[5].value, { date: 1_760_000_000_000 });
  assert.equal(rows[6].value, null, "a scan never reads when an item was last used");
});

test("a file's facts: an unknown value stays unknown, an uncomputable share is left out", () => {
  const rows = itemFacts("files", file({ allocated_bytes: null, modified_unix_ms: null, status: "partial" }), view());
  assert.deepEqual(keys(rows), ["storage.insp.onDisk", "storage.insp.logical", "storage.insp.shareFolder", "storage.insp.modified", "storage.insp.used"]);
  assert.equal(rows[0].value, null, "size on disk unknown, never zero");
  assert.deepEqual(rows[1].value, { bytes: 7 * GiB });
  assert.deepEqual(rows[2].value, { share: 7 / 200 });
  assert.equal(rows[3].value, null, "no modification date, never a made-up one");
  // Files never get a file count; a folder without one says it is unknown.
  assert.equal(itemFacts("dirs", folder({ files: null }), view()).find((row) => row.key === "storage.insp.files").value, null);
});

test("a whole drive's scan shares by what it measured, apart from the drive's capacity", () => {
  const rows = itemFacts("dirs", folder({ path: "D:\\Games" }), view({ root: "D:\\" }));
  assert.deepEqual(keys(rows), ["storage.insp.onDisk", "storage.insp.logical", "storage.insp.files", "storage.insp.shareScanned", "storage.insp.shareDrive", "storage.insp.modified", "storage.insp.used"]);
  assert.equal(rows[3].vars, undefined);
  assert.deepEqual(rows[4].vars, { drive: "D:" });
});

test("without totals or a drive letter, the shares degrade without inventing a value", () => {
  const rows = itemFacts("dirs", folder(), view({ root: "\\\\server\\share", scanned: null }));
  assert.deepEqual(keys(rows), ["storage.insp.onDisk", "storage.insp.logical", "storage.insp.files", "storage.insp.shareDriveAny", "storage.insp.modified", "storage.insp.used"]);
  assert.equal(rows[3].vars, undefined);
  const none = itemFacts("dirs", folder(), view({ scanned: null, volume: { total_bytes: null, free_bytes: null } }));
  assert.equal(none.some((row) => row.key.startsWith("storage.insp.share")), false);
});

test("notes: why the item is incomplete, then the partial scan only when a share is shown", () => {
  assert.deepEqual(itemNotes("dirs", folder(), view()), []);
  assert.deepEqual(itemNotes("dirs", folder({ status: "permission_denied" }), view()), ["storage.insp.note.denied"]);
  assert.deepEqual(itemNotes("files", file({ status: "partial", allocated_bytes: null }), view({ status: "partial" })), ["storage.insp.note.noAllocation", "storage.insp.scanPartial"]);
  assert.deepEqual(itemNotes("dirs", folder(), view({ status: "cancelled" })), ["storage.insp.scanPartial"]);
  // Without any share, a partial scan has nothing to qualify.
  assert.deepEqual(itemNotes("dirs", folder(), view({ status: "partial", scanned: null, volume: { total_bytes: null, free_bytes: null } })), []);
});

test("the facts and notes carry no path: the location is shown apart, on this PC only", () => {
  const item = file();
  const shown = JSON.stringify([itemFacts("files", item, view()), itemNotes("files", item, view())]);
  assert.equal(shown.includes("Alex"), false);
  assert.equal(shown.includes("holiday"), false);
});
