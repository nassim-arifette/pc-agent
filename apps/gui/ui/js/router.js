// Hash routes: #/chat, #/history, #/live, #/storage, #/settings. The old #/home and #/investigate
// open the chat. #/setup is a resumable guide, not a primary navigation destination.
export const ROUTES = ["chat", "history", "live", "storage", "settings"];
const ALIASES = { home: "chat", investigate: "chat" };

export function currentRoute() {
  const name = window.location.hash.replace(/^#\/?/, "");
  const route = ALIASES[name] || name;
  return ROUTES.includes(route) || route === "setup" ? route : "chat";
}

export function navigate(name) {
  if (currentRoute() === name && window.location.hash) return;
  window.location.hash = `#/${name}`;
}
