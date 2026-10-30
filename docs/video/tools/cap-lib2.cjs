// High-resolution capture: CDP screencast frames (JPEG q92 at device pixels), assembled to 30 fps.
const { chromium } = require('playwright');
const fs = require('fs');
const { execFileSync } = require('child_process');
const base = 'http://127.0.0.1:5174/ui/index.html';
const CURSOR = (z) => {
  addEventListener('DOMContentLoaded', () => {
    const c = document.createElement('div');
    Object.assign(c.style, { position: 'fixed', left: '0', top: '0', width: '22px', height: '22px', zIndex: 2147483647, pointerEvents: 'none', transform: 'translate(-3px,-2px)' });
    c.innerHTML = '<svg width="22" height="22" viewBox="0 0 24 24"><path d="M4 2l15 11-7 1-3 7z" fill="#111" stroke="#fff" stroke-width="1.6" stroke-linejoin="round"/></svg>';
    document.documentElement.appendChild(c);
    const r = document.createElement('div');
    Object.assign(r.style, { position: 'fixed', width: '34px', height: '34px', borderRadius: '50%', border: '3px solid #8db87a', zIndex: 2147483646, pointerEvents: 'none', opacity: '0', transform: 'translate(-17px,-17px) scale(.4)' });
    document.documentElement.appendChild(r);
    addEventListener('mousemove', e => { c.style.left = e.clientX / z + 'px'; c.style.top = e.clientY / z + 'px'; }, true);
    addEventListener('mousedown', e => { r.style.left = e.clientX / z + 'px'; r.style.top = e.clientY / z + 'px'; r.style.transition = 'none'; r.style.opacity = '1'; r.style.transform = 'translate(-17px,-17px) scale(.4)'; requestAnimationFrame(() => requestAnimationFrame(() => { r.style.transition = 'transform .4s ease-out, opacity .4s ease-out'; r.style.opacity = '0'; r.style.transform = 'translate(-17px,-17px) scale(1.4)'; })); }, true);
  });
};
async function session(name, { theme = 'light', query = '', hash = '#/chat', zoom: z0 = 1.35, scale = 1.5 } = {}, fn) {
  // Same CSS layout as a 1920x1080 window at zoom z0, rendered at 1.5x the pixels.
  const w = Math.round(1920 * scale), h = Math.round(1080 * scale), zoom = z0 * scale, dsf = 1;
  const b = await chromium.launch();
  const ctx = await b.newContext({ viewport: { width: w, height: h }, deviceScaleFactor: dsf });
  await ctx.addInitScript((theme) => { try { localStorage.setItem('pcagent.prefs.v1', JSON.stringify({ language: 'en', theme })); } catch {} }, theme);
  await ctx.addInitScript((z) => { addEventListener('DOMContentLoaded', () => { document.documentElement.style.zoom = String(z); }); }, zoom);
  await ctx.addInitScript(CURSOR, zoom);
  const p = await ctx.newPage(); const errs = [];
  p.on('pageerror', e => errs.push(e.message));
  const move = async (x, y, steps = 18) => { await p.mouse.move(x, y, { steps }); };
  const at = async (loc) => { const bb = await loc.boundingBox(); return [bb.x + bb.width / 2, bb.y + bb.height / 2]; };
  const click = async (loc, pause = 250) => { await loc.waitFor({ state: 'visible', timeout: 40000 }); const [x, y] = await at(loc); await move(x, y); await p.waitForTimeout(pause); await p.mouse.down(); await p.mouse.up(); };
  await p.goto(base + query + hash); await p.mouse.move(w * 0.6, h * 0.55); await p.waitForTimeout(400);
  const dir = `video/hires/${name}`; fs.rmSync(dir, { recursive: true, force: true }); fs.mkdirSync(dir, { recursive: true });
  const cdp = await ctx.newCDPSession(p); const frames = []; let n = 0;
  cdp.on('Page.screencastFrame', async ({ data, metadata, sessionId }) => {
    const f = `${dir}/f${String(n++).padStart(5, '0')}.jpg`; fs.writeFileSync(f, Buffer.from(data, 'base64')); frames.push({ f, t: metadata.timestamp });
    try { await cdp.send('Page.screencastFrameAck', { sessionId }); } catch {}
  });
  const start = Date.now() / 1000; const marks = {};
  const mark = (k) => { marks[k] = +(Date.now() / 1000 - start).toFixed(3); };
  await cdp.send('Page.startScreencast', { format: 'jpeg', quality: 92, maxWidth: Math.round(w * dsf), maxHeight: Math.round(h * dsf), everyNthFrame: 1 });
  // A tiny repaint keeps frames coming even on a still page.
  await p.evaluate(() => { const d = document.createElement('div'); d.id = '__tick'; Object.assign(d.style, { position: 'fixed', right: '0', bottom: '0', width: '1px', height: '1px', opacity: '0.01', pointerEvents: 'none' }); document.body.appendChild(d); let i = 0; setInterval(() => { d.style.background = (i++ % 2) ? '#000' : '#fff'; }, 33); });
  await fn({ p, click, move, mark, wait: (ms) => p.waitForTimeout(ms) });
  await p.waitForTimeout(700); const end = Date.now() / 1000;
  await cdp.send('Page.stopScreencast'); await p.waitForTimeout(200);
  await ctx.close(); await b.close();
  frames.sort((a, b) => a.t - b.t);
  let list = '';
  const first = frames[0];
  if (first.t > start) list += `file '${process.cwd()}/${first.f}'\nduration ${(first.t - start).toFixed(4)}\n`;
  frames.forEach((fr, i) => { const nt = i + 1 < frames.length ? frames[i + 1].t : end; list += `file '${process.cwd()}/${fr.f}'\nduration ${Math.max(0.001, nt - fr.t).toFixed(4)}\n`; });
  list += `file '${process.cwd()}/${frames[frames.length - 1].f}'\n`;
  fs.writeFileSync(`${dir}/list.txt`, list);
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'concat', '-safe', '0', '-i', `${dir}/list.txt`, '-vf', 'fps=30,format=yuv420p', '-c:v', 'libx264', '-crf', '14', '-preset', 'medium', `video/hires/${name}.mp4`]);
  fs.writeFileSync(`video/hires/${name}.marks.json`, JSON.stringify(marks, null, 1));
  console.log(name, errs.join(' | ') || 'ok', `${frames.length} frames / ${(end - start).toFixed(1)} s`, JSON.stringify(marks));
}
module.exports = { session };
