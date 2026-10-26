export const IMAGE_MODEL_ID = "nvidia/Nemotron-3.5-Super-VL";
export const MAX_IMAGE_INPUT_BYTES = 8 * 1024 * 1024;

function failure(code) {
  return Object.assign(new Error(code), { code });
}

/** Only bytes cross IPC. Filenames, paths and clipboard text never become an image authority. */
export async function imageFileBytes(file) {
  if (!file || (file.type && !["image/png", "image/jpeg"].includes(file.type))) throw failure("image_unsupported");
  if (file.size > MAX_IMAGE_INPUT_BYTES) throw failure("image_too_large");
  const bytes = new Uint8Array(await file.arrayBuffer());
  if (bytes.length > MAX_IMAGE_INPUT_BYTES) throw failure("image_too_large");
  let binary = "";
  for (let i = 0; i < bytes.length; i += 16384) binary += String.fromCharCode(...bytes.subarray(i, i + 16384));
  return btoa(binary);
}

export function pastedImages(data) {
  return Array.from(data?.items || [])
    .filter((item) => item.kind === "file" && item.type.startsWith("image/"))
    .map((item) => item.getAsFile()).filter(Boolean);
}

/** A single local draft; stale preparations cannot attach to a newly selected conversation. */
export function createImageDraft({ prepare, discard, changed = () => {} }) {
  let image = null;
  let pending = false;
  let revision = 0;
  let owner;
  const forget = (id) => Promise.resolve(discard(id)).catch(() => {});
  const clear = () => {
    revision += 1;
    const previous = image;
    image = null;
    pending = false;
    if (previous) void forget(previous.id);
    changed();
  };
  return {
    get image() { return image; },
    get pending() { return pending; },
    get revision() { return revision; },
    bind(conversationRevision) {
      if (owner === conversationRevision) return;
      owner = conversationRevision;
      clear();
    },
    clear,
    consume(id) {
      if (image?.id !== id) return;
      revision += 1;
      image = null;
      changed();
    },
    async attach(files) {
      if (files.length !== 1) throw failure("image_single");
      if (pending) throw failure("image_preparing");
      clear();
      const current = revision;
      pending = true;
      changed();
      try {
        const prepared = await prepare(await imageFileBytes(files[0]));
        if (revision !== current) {
          await forget(prepared.id);
          return;
        }
        image = prepared;
      } finally {
        if (revision === current) {
          pending = false;
          changed();
        }
      }
    },
  };
}
