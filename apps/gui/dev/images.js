// Browser-only image fixture. Production decoding, bounds and EXIF handling live in Rust.
export function createImageFixture(fail, invalidate) {
  let draft = null;
  const saved = new Map();
  const read = (id) => {
    if (!draft || draft.id !== id || draft.expires <= Date.now()) fail("image_expired", "Attach the image again.");
    return draft.view;
  };
  const metadata = ({ data_url, id, ...info }) => info;
  return {
    read,
    metadata,
    async prepare(dataBase64) {
      if (dataBase64.length > Math.ceil(8 * 1024 * 1024 / 3) * 4) fail("image_too_large", "Image too large.");
      draft = null;
      invalidate();
      let bitmap;
      try {
        const raw = Uint8Array.from(atob(dataBase64), (c) => c.charCodeAt(0));
        const png = raw[0] === 137 && raw[1] === 80;
        const jpeg = raw[0] === 255 && raw[1] === 216;
        if (!png && !jpeg) fail("image_unsupported", "Choose a static PNG or JPEG.");
        bitmap = await createImageBitmap(new Blob([raw], { type: png ? "image/png" : "image/jpeg" }));
      } catch (error) {
        if (error.code) throw error;
        fail("image_invalid", "Image could not be decoded.");
      }
      try {
        if (bitmap.width > 16384 || bitmap.height > 16384 || bitmap.width * bitmap.height > 24000000) fail("image_too_large", "Image too large.");
        const scale = Math.min(1, 1600 / Math.max(bitmap.width, bitmap.height));
        const canvas = document.createElement("canvas");
        canvas.width = Math.round(bitmap.width * scale);
        canvas.height = Math.round(bitmap.height * scale);
        const context = canvas.getContext("2d");
        context.fillStyle = "#ffffff";
        context.fillRect(0, 0, canvas.width, canvas.height);
        context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
        const blob = await new Promise((resolve) => canvas.toBlob(resolve, "image/jpeg", 0.85));
        if (!blob || blob.size > 2 * 1024 * 1024) fail("image_too_large", "Prepared image too large.");
        const sha256 = [...new Uint8Array(await crypto.subtle.digest("SHA-256", await blob.arrayBuffer()))].map((b) => b.toString(16).padStart(2, "0")).join("");
        const view = { id: crypto.randomUUID(), mime_type: "image/jpeg", width: canvas.width, height: canvas.height, bytes: blob.size, sha256, data_url: canvas.toDataURL("image/jpeg", 0.85) };
        draft = { view, id: view.id, expires: Date.now() + 300000 };
        return view;
      } finally {
        bitmap.close();
      }
    },
    discard(id) {
      if (draft?.id === id) { draft = null; invalidate(); }
    },
    save(image, turnId, conversationId) {
      if (image) saved.set(turnId, { conversationId, image });
    },
    get(turnId, conversationId) {
      const entry = saved.get(turnId);
      return entry?.conversationId === conversationId ? entry.image : null;
    },
    delete(conversationId) {
      for (const [id, entry] of saved) if (entry.conversationId === conversationId) saved.delete(id);
    },
    clear() { draft = null; saved.clear(); invalidate(); },
  };
}
