import { h, fill } from "../dom.js";
import { t } from "../i18n.js";
import { bytes } from "../format.js";
import { call } from "../api.js";
import { dialog, button, toast } from "../ui.js";
import { failureText } from "../failure.js";

export function imagePreview(image) {
  return h("figure", { class: "image-preview" },
    h("img", { src: image.data_url, alt: t("image.alt") }),
    h("figcaption", null, t("image.details", { width: image.width, height: image.height, size: bytes(image.bytes) })),
  );
}

export function showImage(image) {
  return dialog((close) => [
    h("h2", { class: "dialog-title" }, t("image.preview")),
    imagePreview(image),
    h("p", { class: "note" }, t("image.warning")),
    h("p", { class: "note mono" }, `SHA-256: ${image.sha256}`),
    h("div", { class: "dialog-actions" }, button(t("common.close"), { onClick: () => close(null) })),
  ], { label: t("image.preview"), dismissValue: null });
}

/** History loads pixels only on request and never attaches them to another question. */
export function savedImage(image, conversationId, turnId) {
  if (!image || !conversationId || !turnId) return null;
  const host = h("div", { class: "saved-image" });
  const load = button(t("image.saved"), { onClick: async () => {
    load.disabled = true;
    try {
      const prepared = await call("conversation_image_get", { conversationId, turnId });
      if (!prepared) throw Object.assign(new Error(), { code: "image_expired" });
      if (host.isConnected) fill(host, imagePreview(prepared), h("p", { class: "note" }, t("image.notResent")));
    } catch (error) {
      toast(failureText(error), "bad");
      load.disabled = false;
    }
  } });
  host.append(load);
  return host;
}
