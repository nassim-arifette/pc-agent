const { session } = require('./cap-lib2.cjs');
const Q = '?cloud=ready&privacy=complete';
const only = process.argv.slice(2);
const scenes = {
  chat: () => session('chat', { query: Q }, async ({ p, click, mark, wait }) => {
    await wait(800); mark('home');
    await click(p.locator('.ask textarea')); await p.keyboard.type('Why is my PC slow?', { delay: 55 }); mark('typed');
    await click(p.locator('.send')); mark('sent');
    await p.waitForSelector('.answer-foot', { timeout: 40000 }); mark('answer'); await wait(2500);
    await click(p.locator('.answer-text .cite').first()); mark('cite');
    await p.waitForSelector('.panel.open .source-card.focused'); await wait(3500); mark('end');
  }),
  action: () => session('action', { query: Q + '&tour=actions' }, async ({ p, click, mark, wait }) => {
    await wait(600);
    await p.locator('.ask textarea').fill('Why is my PC slow? Anything I can clean?'); await p.locator('.send').click();
    await p.waitForSelector('.action-card .btn', { timeout: 40000 }); await wait(300);
    await p.locator('.action-card').first().scrollIntoViewIfNeeded(); await wait(2200); mark('card');
    await click(p.locator('.action-card .btn').first(), 350); mark('review');
    await p.waitForSelector('.dialog .plan-targets'); mark('dialog'); await wait(5200);
    const run = p.locator('.dialog button', { hasText: /Run now/ });
    await click(run.first(), 300); mark('run'); await wait(3200); mark('end');
  }),
  privacy: () => session('privacy', { query: Q }, async ({ p, click, mark, wait }) => {
    await wait(600);
    await click(p.locator('.ask textarea')); await p.keyboard.type('My PC is slow when I open Chrome', { delay: 25 }); mark('typed');
    await click(p.locator('.share-preview').first(), 150); mark('preview'); await wait(3800);
    await p.keyboard.press('Escape'); mark('closed'); await wait(350);
    await click(p.locator('[data-share-key="names"], .share-toggle').first(), 150); mark('names'); await wait(450);
    await click(p.locator('.share-preview').first(), 150); mark('preview2'); await wait(2600); mark('end');
  }),
  live: () => session('live', { query: Q, theme: 'dark', hash: '#/live', zoom: 1.15 }, async ({ p, click, mark, wait }) => {
    await p.waitForSelector('.livepage-card'); await wait(1800); mark('cards');
    await click(p.locator('.livepage-card').first()); mark('select'); await wait(2300);
    await click(p.locator('[data-key="actbar:monitor"]').first()); mark('monitor'); await wait(3000); mark('end');
  }),
  watches: () => session('watches', { query: Q, theme: 'dark', hash: '#/watches' }, async ({ p, mark, wait, move }) => {
    await wait(1800); mark('top'); await move(900, 600);
    await p.mouse.wheel(0, 500); await wait(2500); mark('end');
  }),
  report: () => session('report', { query: Q }, async ({ p, mark, wait, move }) => {
    await wait(1200); await move(1000, 700);
    for (let i = 0; i < 3; i++) { await p.mouse.wheel(0, 110); await wait(140); }
    mark('report'); await wait(3500); mark('end');
  }),
  web: () => session('web', { query: Q + '&web=on' }, async ({ p, click, mark, wait }) => {
    await wait(600);
    await click(p.locator('.ask textarea')); await p.keyboard.type('Is there a known issue with my NVIDIA driver?', { delay: 30 });
    await click(p.locator('.send')); mark('sent');
    await p.waitForSelector('.answer-foot', { timeout: 40000 }); mark('answer'); await wait(900);
    const w = p.locator('.answer-text .cite', { hasText: /W|1/ }).last(); await click(w); mark('cite'); await wait(3800); mark('end');
  }),
  storage: () => session('storage', { query: Q, hash: '#/storage' }, async ({ p, click, mark, wait }) => {
    await wait(800); mark('home');
    await click(p.locator('.chip').first()); mark('scan');
    await p.waitForSelector('.storage-map, .tabs', { timeout: 20000 }); await wait(1200); mark('map');
    await p.locator('.storage-map').scrollIntoViewIfNeeded(); await p.mouse.wheel(0, 260); await wait(700); mark('scrolled');
    await click(p.locator('.storage-tile').first()); mark('select'); await wait(3500); mark('end');
  }),
  history: () => session('history', { query: Q, theme: 'dark', hash: '#/history' }, async ({ p, click, mark, wait }) => {
    await p.waitForFunction(() => document.querySelectorAll('.history-chart').length >= 4, null, { timeout: 15000 }); await wait(600); mark('charts');
    const chart = p.locator('.history-chart').first(); const bb = await chart.boundingBox();
    await p.mouse.move(bb.x + bb.width * 0.3, bb.y + bb.height / 2, { steps: 10 }); await p.mouse.move(bb.x + bb.width * 0.75, bb.y + bb.height / 2, { steps: 80 }); mark('hover'); await wait(1500); mark('end');
  }),
  search: () => session('search', { query: Q }, async ({ p, click, mark, wait }) => {
    await p.locator('.ask textarea').fill('Why is my PC slow?'); await p.locator('.send').click();
    await p.waitForSelector('.answer-foot', { timeout: 40000 }); await wait(400);
    await p.locator('button', { hasText: 'New conversation' }).first().click(); await wait(600);
    await p.locator('.ask textarea').fill('What can I delete on C:?'); await p.locator('.send').click();
    await p.waitForSelector('.answer-foot', { timeout: 40000 }); await wait(700); mark('ready');
    const s = p.locator('.sidebar input').first();
    await click(s); await p.keyboard.type('chrome', { delay: 90 }); mark('typed'); await wait(3200); mark('end');
  }),
  spec: () => session('spec', { query: Q + '&specialist=ready', hash: '#/settings' }, async ({ p, click, mark, wait }) => {
    await wait(900);
    const card = p.locator('[data-specialist="settings"]'); await card.scrollIntoViewIfNeeded(); await wait(700); mark('card');
    await click(card.locator('[role="switch"], .switch').first()); mark('on'); await wait(1800);
    await click(p.locator('.rail-item').first()); await wait(800); mark('chat');
    await click(p.locator('.ask textarea')); await p.keyboard.type('Why is my PC slow?', { delay: 40 });
    await click(p.locator('.send')); mark('sent');
    await p.waitForSelector('.answer-foot', { timeout: 40000 }); mark('answer'); await wait(1500); mark('end');
  }),
  ram: () => session('ram', { query: Q + '&web=on' }, async ({ p, click, mark, wait }) => {
    await wait(700);
    await click(p.locator('.ask textarea')); await p.keyboard.type('Do I need more RAM?', { delay: 45 });
    await click(p.locator('.send')); mark('sent');
    await p.waitForSelector('.answer-foot', { timeout: 40000 }); mark('answer'); await wait(900);
    const w = p.locator('.answer-text .cite').filter({ hasText: '3' }).first(); await click(w); mark('cite'); await wait(3800); mark('end');
  }),
};
(async () => { for (const [k, f] of Object.entries(scenes)) if (!only.length || only.includes(k)) { try { await f(); } catch (e) { console.log(k, 'FAILED', e.message.split('\n')[0]); } } })();
