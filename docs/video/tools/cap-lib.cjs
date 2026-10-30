const { chromium } = require('playwright');
const base = 'http://127.0.0.1:5174/ui/index.html';
const CURSOR = (z) => {
  addEventListener('DOMContentLoaded', () => {
    const c = document.createElement('div');
    c.id = '__cursor';
    Object.assign(c.style, { position: 'fixed', left: '0', top: '0', width: '22px', height: '22px', zIndex: 2147483647, pointerEvents: 'none', transform: 'translate(-3px,-2px)', transition: 'none' });
    c.innerHTML = '<svg width="22" height="22" viewBox="0 0 24 24"><path d="M4 2l15 11-7 1-3 7z" fill="#111" stroke="#fff" stroke-width="1.6" stroke-linejoin="round"/></svg>';
    document.documentElement.appendChild(c);
    const r = document.createElement('div');
    Object.assign(r.style, { position: 'fixed', width: '34px', height: '34px', borderRadius: '50%', border: '3px solid #76B900', zIndex: 2147483646, pointerEvents: 'none', opacity: '0', transform: 'translate(-17px,-17px) scale(.4)', transition: 'transform .35s ease-out, opacity .35s ease-out' });
    document.documentElement.appendChild(r);
    addEventListener('mousemove', e => { c.style.left = e.clientX / z + 'px'; c.style.top = e.clientY / z + 'px'; }, true);
    addEventListener('mousedown', e => { r.style.left = e.clientX / z + 'px'; r.style.top = e.clientY / z + 'px'; r.style.transition = 'none'; r.style.opacity = '1'; r.style.transform = 'translate(-17px,-17px) scale(.4)'; requestAnimationFrame(() => requestAnimationFrame(() => { r.style.transition = 'transform .4s ease-out, opacity .4s ease-out'; r.style.opacity = '0'; r.style.transform = 'translate(-17px,-17px) scale(1.4)'; })); }, true);
  });
};
async function session(name, { theme = 'light', query = '', hash = '#/chat', w = 1920, h = 1080, zoom = 1.35 } = {}, fn) {
  const b = await chromium.launch();
  const ctx = await b.newContext({ viewport: { width: w, height: h }, deviceScaleFactor: 1, recordVideo: { dir: `video/clips/tmp-${name}`, size: { width: 1920, height: 1080 } } });
  await ctx.addInitScript((theme) => { try { localStorage.setItem('pcagent.prefs.v1', JSON.stringify({ language: 'en', theme })); } catch {} }, theme);
  await ctx.addInitScript((z) => { addEventListener('DOMContentLoaded', () => { document.documentElement.style.zoom = String(z); }); }, zoom);
  await ctx.addInitScript(CURSOR, zoom);
  const p = await ctx.newPage(); const errs = [];
  p.on('pageerror', e => errs.push(e.message));
  let mx = w / 2, my = h / 2;
  const move = async (x, y, steps = 18) => { await p.mouse.move(x, y, { steps }); mx = x; my = y; };
  const at = async (loc) => { const bb = await loc.boundingBox(); return [bb.x + bb.width / 2, bb.y + bb.height / 2]; };
  const click = async (loc, pause = 250) => { await loc.waitFor({ state: 'visible', timeout: 40000 }); const [x, y] = await at(loc); await move(x, y); await p.waitForTimeout(pause); await p.mouse.down(); await p.mouse.up(); };
  const t0 = Date.now(); const marks = {};
  const mark = (k) => { marks[k] = (Date.now() - t0) / 1000; };
  await p.goto(base + query + hash); await move(w * 0.6, h * 0.55, 1);
  await fn({ p, click, move, mark, wait: (ms) => p.waitForTimeout(ms) });
  await p.waitForTimeout(600);
  const v = p.video(); await ctx.close(); await v.saveAs(`video/clips/${name}.webm`); await b.close();
  require('fs').writeFileSync(`video/clips/${name}.marks.json`, JSON.stringify(marks, null, 1));
  console.log(name, errs.join(' | ') || 'ok', JSON.stringify(marks));
}
module.exports = { session };
