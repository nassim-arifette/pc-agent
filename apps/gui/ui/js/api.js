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

/** Calls a Rust command. Errors come back as plain sentences. */
export async function call(command, args) {
  try {
    return await impl.invoke(command, args);
  } catch (error) {
    throw new Error(typeof error === "string" ? error : error?.message || String(error));
  }
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
