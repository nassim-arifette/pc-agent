// Bridge to the Rust side. Inside the app, Tauri provides window.__TAURI__; opened in a plain
// browser for design work, a local mock with synthetic data answers instead (never shipped: it
// lives outside the ui folder that the app embeds).
let impl = null;

export async function connect() {
  const tauri = window.__TAURI__;
  if (tauri?.core?.invoke) {
    impl = {
      invoke: (cmd, args) => tauri.core.invoke(cmd, args),
      listen: (name, cb) => tauri.event.listen(name, (event) => cb(event.payload)),
      onDrop: (cb) => {
        const webview = tauri.webview?.getCurrentWebview?.();
        if (!webview?.onDragDropEvent) return () => {};
        let off = () => {};
        webview.onDragDropEvent((event) => cb(event.payload)).then((unlisten) => {
          off = unlisten;
        });
        return () => off();
      },
      mock: false,
    };
    return impl;
  }
  const { createMock } = await import("../../dev/mock.js");
  impl = createMock();
  return impl;
}

/**
 * Calls a Rust command. A failure rejects with an Error whose `code` is a stable code from a
 * closed set ("unknown" when the host sent a bare sentence), `reason` the closed reason of a
 * refused action, and `message` the host's English detail: failure.js turns the code into a
 * sentence in the interface language and never shows the message as the main text.
 */
export async function call(command, args) {
  try {
    return await impl.invoke(command, args);
  } catch (error) {
    throw commandError(error);
  }
}

const CODE = /^[a-z][a-z0-9_]{0,63}$/;

/** One shape for a failed command, whether the host sent a string or `{ code, message, reason? }`. */
export function commandError(error) {
  const message = typeof error === "string" ? error : typeof error?.message === "string" ? error.message : String(error);
  const failure = new Error(message);
  failure.code = typeof error?.code === "string" && CODE.test(error.code) ? error.code : "unknown";
  if (typeof error?.reason === "string" && CODE.test(error.reason)) failure.reason = error.reason;
  return failure;
}

export function on(name, callback) {
  return impl.listen(name, callback);
}

/** Drag-and-drop of files or folders onto the window (paths are resolved by Rust). */
export function onDrop(callback) {
  return impl.onDrop(callback);
}

export function isMock() {
  return Boolean(impl?.mock);
}
