// Squarified rectangles; zero/unknown sizes stay in the accessible list, not invented areas.
export function squarify(items, width, height) {
  if (!(width > 0 && height > 0)) return [];
  const values = items.filter((item) => Number.isFinite(item.size) && item.size > 0)
    .sort((a, b) => b.size - a.size);
  const total = values.reduce((sum, item) => sum + item.size, 0);
  if (!Number.isFinite(total) || !total) return [];
  const pending = values.map((item) => ({ item, area: item.size / total * width * height }));
  const result = [];
  let x = 0, y = 0, w = width, h = height, cursor = 0;
  const worst = (row, side) => {
    if (!row.length || !side) return Infinity;
    const sum = row.reduce((n, entry) => n + entry.area, 0);
    const max = row[0].area;
    const min = row[row.length - 1].area;
    return Math.max(side * side * max / (sum * sum), sum * sum / (side * side * min));
  };
  while (cursor < pending.length) {
    const side = Math.min(w, h);
    const row = [pending[cursor++]];
    while (cursor < pending.length && worst([...row, pending[cursor]], side) <= worst(row, side)) {
      row.push(pending[cursor++]);
    }
    const area = row.reduce((sum, entry) => sum + entry.area, 0);
    const vertical = w >= h;
    const thickness = side ? area / side : 0;
    let offset = 0;
    for (const { item, area: tile } of row) {
      const length = thickness ? tile / thickness : 0;
      result.push({ item, x: x + (vertical ? 0 : offset), y: y + (vertical ? offset : 0),
        width: vertical ? thickness : length, height: vertical ? length : thickness });
      offset += length;
    }
    if (vertical) { x += thickness; w = Math.max(0, w - thickness); }
    else { y += thickness; h = Math.max(0, h - thickness); }
  }
  return result;
}

/** The remainder reflects all displayed pages, not just the last response's page. */
export function otherSize(parent, items) {
  return Math.max(0, (parent?.size || 0) - items.reduce((sum, item) => sum + (item.size || 0), 0));
}

export function watchProposal(view) {
  const volume = view?.volume;
  if (!volume?.fixed) return { reason: "storage.watch.notFixed" };
  if (!volume.root || !Number.isFinite(volume.total_bytes) || volume.total_bytes < 1024 ** 3) {
    return { reason: "storage.watch.unknown" };
  }
  return { rule: { kind: "volume_free_below", root: volume.root,
    threshold_bytes: Math.max(1024 ** 3, Math.min(10 * 1024 ** 3, volume.total_bytes)) } };
}
