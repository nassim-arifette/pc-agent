// Hash routes: #/chat, #/history, #/storage, #/settings. The old #/home and #/investigate open the chat.
export const ROUTES = ["chat", "history", "storage", "settings"];
const ALIASES = { home: "chat", investigate: "chat" };

export function currentRoute() {
  const name = window.location.hash.replace(/^#\/?/, "");
  const route = ALIASES[name] || name;
  return ROUTES.includes(route) ? route : "chat";
}

export function navigate(name) {
  if (currentRoute() === name && window.location.hash) return;
  window.location.hash = `#/${name}`;
}
