import test from "node:test";
import assert from "node:assert/strict";
import { squarify, otherSize, watchProposal } from "../ui/js/storage/treemap.js";

test("squarified areas are proportional, bounded and non-overlapping at wide and narrow sizes", () => {
  const items = [500, 300, 70, 30, 20, 1].map((size, i) => ({ item_ref: `${i}`, size }));
  for (const [w, h] of [[1000, 450], [280, 400], [1, 1]]) {
    const cells = squarify(items, w, h);
    assert.equal(cells.length, items.length);
    for (const cell of cells) {
      assert.ok(cell.x >= 0 && cell.y >= 0);
      assert.ok(cell.x + cell.width <= w + 1e-8);
      assert.ok(cell.y + cell.height <= h + 1e-8);
      assert.ok(Math.abs(cell.width * cell.height / (w * h) - cell.item.size / 921) < 1e-8);
      for (const other of cells) {
        if (cell === other) continue;
        const ix = Math.min(cell.x + cell.width, other.x + other.width) - Math.max(cell.x, other.x);
        const iy = Math.min(cell.y + cell.height, other.y + other.height) - Math.max(cell.y, other.y);
        assert.ok(ix < 1e-8 || iy < 1e-8);
      }
    }
  }
});

test("zero, unknown and invalid sizes do not acquire invented area", () => {
  assert.deepEqual(squarify([{ size: 0 }, { size: null }, { size: NaN }, { size: -1 }], 500, 400), []);
  assert.deepEqual(squarify([{ size: 1 }], 0, 400), []);
  assert.equal(otherSize({ size: 100 }, [{ size: 20 }, { size: 30 }]), 50);
  assert.equal(otherSize({ size: 10 }, [{ size: 20 }]), 0);
});

test("watch proposal requires a fixed known volume and clamps its threshold", () => {
  const GiB = 1024 ** 3;
  assert.ok(watchProposal({ volume: { fixed: false } }).reason);
  assert.ok(watchProposal({ volume: { fixed: true, root: "C:\\", total_bytes: null } }).reason);
  assert.ok(watchProposal({ volume: { fixed: true, root: "C:\\", total_bytes: GiB / 2 } }).reason);
  const result = watchProposal({ volume: { fixed: true, root: "C:\\", total_bytes: 5 * GiB } });
  assert.deepEqual(result.rule, { kind: "volume_free_below", root: "C:\\", threshold_bytes: 5 * GiB });
});
