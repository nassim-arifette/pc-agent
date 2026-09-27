// Hash routes: #/home, #/investigate, #/storage, #/settings.
export const ROUTES = ["home", "investigate", "storage", "settings"];

export function currentRoute() {
  const name = window.location.hash.replace(/^#\/?/, "");
  return ROUTES.includes(name) ? name : "home";
}

export function navigate(name) {
  if (currentRoute() === name && window.location.hash) return;
  window.location.hash = `#/${name}`;
}
