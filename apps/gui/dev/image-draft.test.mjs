import test from "node:test";
import assert from "node:assert/strict";
import { createImageDraft, imageFileBytes, pastedImages, MAX_IMAGE_INPUT_BYTES } from "../ui/js/chat/image-draft.js";

const file = (type = "image/png", size = 3) => ({
  type, size, name: "private-name.png", path: "C:\\Users\\private\\image.png",
  arrayBuffer: async () => Uint8Array.of(1, 2, 3).buffer,
});
const prepared = { id: "opaque-1", data_url: "data:image/jpeg;base64,prepared", sha256: "abc" };

test("file preparation forwards only bounded bytes, never a name or path", async () => {
  assert.equal(await imageFileBytes(file()), "AQID");
  await assert.rejects(imageFileBytes(file("image/gif")), { code: "image_unsupported" });
  await assert.rejects(imageFileBytes(file("image/png", MAX_IMAGE_INPUT_BYTES + 1)), { code: "image_too_large" });
});

test("clipboard accepts image bytes, never remote URLs or pasted text", () => {
  const image = file();
  assert.deepEqual(pastedImages({ items: [
    { kind: "string", type: "text/uri-list", getAsFile: () => { throw Error("not a file"); } },
    { kind: "file", type: "image/png", getAsFile: () => image },
  ] }), [image]);
  assert.deepEqual(pastedImages(null), []);
});

test("one image, exact prepared preview, replacement, withdrawal and single-turn consumption", async () => {
  const discarded = [];
  const draft = createImageDraft({
    prepare: async (bytes) => { assert.equal(bytes, "AQID"); return prepared; },
    discard: (id) => discarded.push(id),
  });
  draft.bind(0);
  await assert.rejects(draft.attach([file(), file()]), { code: "image_single" });
  await draft.attach([file()]);
  assert.equal(draft.image, prepared);
  assert.equal(draft.pending, false);
  await draft.attach([file()]);
  assert.deepEqual(discarded, [prepared.id]);
  draft.consume("different");
  assert.equal(draft.image, prepared);
  draft.consume(prepared.id);
  assert.equal(draft.image, null);
  assert.deepEqual(discarded, [prepared.id], "consumed bytes belong to the turn, not a discarded draft");
  await draft.attach([file()]);
  draft.clear();
  assert.equal(draft.image, null);
  assert.deepEqual(discarded, [prepared.id, prepared.id]);
});

test("conversation change during decoding discards a late result rather than attaching it", async () => {
  let finish;
  const discarded = [];
  const draft = createImageDraft({
    prepare: () => new Promise((resolve) => { finish = resolve; }),
    discard: (id) => discarded.push(id),
  });
  draft.bind(0);
  const pending = draft.attach([file()]);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(draft.pending, true);
  await assert.rejects(draft.attach([file()]), { code: "image_preparing" });
  draft.bind(1);
  finish(prepared);
  await pending;
  assert.equal(draft.image, null);
  assert.equal(draft.pending, false);
  assert.deepEqual(discarded, [prepared.id]);
});

test("a failed replacement cannot retain an obsolete preview or leave preparation stuck", async () => {
  let fail = false;
  const draft = createImageDraft({
    prepare: async () => { if (fail) throw Error("decode failed"); return prepared; },
    discard: () => {},
  });
  await draft.attach([file()]);
  fail = true;
  await assert.rejects(draft.attach([file()]));
  assert.equal(draft.image, null);
  assert.equal(draft.pending, false);
});
