// Per-person conveniences kept in this window's storage: theme, language and motion. Storage can
// be unavailable; everything works with the defaults then.
import { setLanguage } from "./i18n.js";

const KEY = "pcagent.prefs.v1";
// English by default (the submission's language); Settings still offers the system language and French.
const defaults = { theme: "system", language: "en", motion: "system", askBeforeCloud: true };
let prefs = { ...defaults };

export function loadPrefs() {
  try {
    const saved = JSON.parse(window.localStorage.getItem(KEY) || "{}");
    const { mode, names, ...current } = saved;
    void mode;
    void names;
    prefs = { ...defaults, ...current, askBeforeCloud: true };
  } catch {
    prefs = { ...defaults };
  }
  applyPrefs();
  return prefs;
}

export function getPrefs() {
  return prefs;
}

export function setPref(name, value) {
  prefs = { ...prefs, [name]: value };
  try {
    // Consent to send data is never remembered across sessions.
    const { askBeforeCloud, ...kept } = prefs;
    void askBeforeCloud;
    window.localStorage.setItem(KEY, JSON.stringify(kept));
  } catch {
    // Not persisted; the choice still applies to this session.
  }
  applyPrefs();
}

const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");
const motionQuery = window.matchMedia("(prefers-reduced-motion: reduce)");

export function applyPrefs() {
  const root = document.documentElement;
  const dark = prefs.theme === "dark" || (prefs.theme === "system" && darkQuery.matches);
  root.dataset.theme = dark ? "dark" : "light";
  const reduce = prefs.motion === "off" || (prefs.motion === "system" && motionQuery.matches);
  root.classList.toggle("reduce-motion", reduce);
  root.classList.toggle("force-motion", prefs.motion === "on");
  setLanguage(prefs.language);
}

darkQuery.addEventListener("change", applyPrefs);
motionQuery.addEventListener("change", applyPrefs);
