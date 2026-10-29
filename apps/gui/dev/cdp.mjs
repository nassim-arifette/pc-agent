// Minimal Chrome DevTools Protocol driver for design review: a headless Chrome, no extension.
// `eval` sends the review script's own expressions to that local page; it never runs page data.
import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function launch({ chrome = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe", port = 9333, width = 1280, height = 860 } = {}) {
  const profile = mkdtempSync(join(tmpdir(), "pcagent-qa-"));
  const proc = spawn(chrome, [
    "--headless=new",
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profile}`,
    `--window-size=${width},${height}`,
    "--hide-scrollbars",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    "about:blank",
  ], { stdio: "ignore" });
  let targets = null;
  for (let i = 0; i < 60; i++) {
    try {
      targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      if (targets.some((t) => t.type === "page")) break;
    } catch {}
    await sleep(250);
  }
  const target = targets.find((t) => t.type === "page");
  return { proc, page: await connect(target.webSocketDebuggerUrl), port };
}

export async function attach(port) {
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const target = targets.find((t) => t.type === "page");
  return connect(target.webSocketDebuggerUrl);
}

async function connect(url) {
  const ws = new WebSocket(url);
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });
  let id = 0;
  const pending = new Map();
  const logs = [];
  const handlers = new Map();
  ws.onmessage = (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      msg.error ? reject(new Error(msg.error.message)) : resolve(msg.result);
    } else if (msg.method === "Runtime.consoleAPICalled") {
      logs.push(`[${msg.params.type}] ${msg.params.args.map((a) => a.value ?? a.description ?? "").join(" ")}`);
    } else if (msg.method === "Runtime.exceptionThrown") {
      const d = msg.params.exceptionDetails;
      logs.push(`[exception] ${d.exception?.description || d.text} at ${d.url}:${d.lineNumber}`);
    } else if (msg.method === "Log.entryAdded") {
      logs.push(`[log ${msg.params.entry.level}] ${msg.params.entry.text} ${msg.params.entry.url || ""}`);
    }
    for (const cb of handlers.get(msg.method) || []) cb(msg.params);
  };
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const n = ++id;
      pending.set(n, { resolve, reject });
      ws.send(JSON.stringify({ id: n, method, params }));
    });
  await send("Page.enable");
  await send("Runtime.enable");
  await send("Log.enable");
  const page = {
    send,
    logs,
    on(method, cb) {
      if (!handlers.has(method)) handlers.set(method, []);
      handlers.get(method).push(cb);
    },
    async viewport(width, height) {
      await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
    },
    async scheme(dark, reducedMotion = false, forcedColors = false) {
      await send("Emulation.setEmulatedMedia", {
        features: [
          { name: "prefers-color-scheme", value: dark ? "dark" : "light" },
          { name: "prefers-reduced-motion", value: reducedMotion ? "reduce" : "no-preference" },
          { name: "forced-colors", value: forcedColors ? "active" : "none" },
        ],
      });
    },
    async goto(url) {
      const loaded = new Promise((resolve) => page.on("Page.loadEventFired", resolve));
      await send("Page.navigate", { url });
      await Promise.race([loaded, sleep(8000)]);
    },
    async eval(expression) {
      const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
      return r.result.value;
    },
    async waitFor(expression, timeout = 10000) {
      const start = Date.now();
      while (Date.now() - start < timeout) {
        try {
          if (await page.eval(expression)) return true;
        } catch {}
        await sleep(100);
      }
      throw new Error(`Timed out waiting for: ${expression}`);
    },
    async click(selector, index = 0) {
      const ok = await page.eval(`(() => { const el = document.querySelectorAll(${JSON.stringify(selector)})[${index}]; if (!el) return false; el.scrollIntoView({block: 'center'}); el.click(); return true; })()`);
      if (!ok) throw new Error(`No element for ${selector}[${index}]`);
      await sleep(150);
    },
    async clickText(selector, text) {
      const ok = await page.eval(`(() => { const el = [...document.querySelectorAll(${JSON.stringify(selector)})].find(e => e.textContent.includes(${JSON.stringify(text)})); if (!el) return false; el.scrollIntoView({block: 'center'}); el.click(); return true; })()`);
      if (!ok) throw new Error(`No ${selector} containing "${text}"`);
      await sleep(150);
    },
    async type(selector, text) {
      await page.eval(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); el.focus(); el.value = ${JSON.stringify(text)}; el.dispatchEvent(new Event('input', {bubbles: true})); })()`);
    },
    async key(key, code = key) {
      for (const type of ["keyDown", "keyUp"]) {
        await send("Input.dispatchKeyEvent", { type, key, code, windowsVirtualKeyCode: key === "Enter" ? 13 : key === "Escape" ? 27 : 0, text: key === "Enter" ? "\r" : undefined });
      }
      await sleep(120);
    },
    async shot(file, { full = false } = {}) {
      let params = { format: "png" };
      if (full) {
        const metrics = await send("Page.getLayoutMetrics");
        const h = Math.ceil(metrics.cssContentSize?.height || metrics.contentSize.height);
        params = { format: "png", captureBeyondViewport: true, clip: { x: 0, y: 0, width: metrics.cssLayoutViewport.clientWidth, height: h, scale: 1 } };
      }
      const r = await send("Page.captureScreenshot", params);
      writeFileSync(file, Buffer.from(r.data, "base64"));
      return file;
    },
    sleep,
  };
  return page;
}
