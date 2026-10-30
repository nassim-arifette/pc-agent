const { session } = require('./cap-lib.cjs');
const Q = '?cloud=ready&privacy=complete';
const only = process.argv[2];
const scenes = {
  storage: () => session('storage', { query: Q, hash: '#/storage' }, async ({ p, click, mark, wait }) => {
    await wait(1200); mark('home');
    await click(p.locator('.chip').first()); mark('scan');
    await p.waitForSelector('.storage-map, .tabs', { timeout: 20000 }); await wait(1500); mark('map');
    await p.locator('.storage-map').scrollIntoViewIfNeeded(); await p.mouse.wheel(0, 260); await wait(700); mark('scrolled');
    await click(p.locator('.storage-tile').first()); mark('select'); await wait(2800);
    mark('end');
  }),
  history: () => session('history', { query: Q, theme: 'dark', hash: '#/history' }, async ({ p, click, mark, wait }) => {
    await p.waitForFunction(() => document.querySelectorAll('.history-chart').length >= 4, null, { timeout: 15000 }); await wait(800); mark('charts');
    const day = p.getByText('24 hours', { exact: true }).first();
    if (await day.count()) { await click(day); mark('day'); await wait(1800); }
    const chart = p.locator('.history-chart').first(); const bb = await chart.boundingBox();
    if (bb) { await p.mouse.move(bb.x + bb.width * 0.3, bb.y + bb.height / 2, { steps: 10 }); await p.mouse.move(bb.x + bb.width * 0.75, bb.y + bb.height / 2, { steps: 60 }); mark('hover'); await wait(800); }
    mark('end');
  }),
  spec: () => session('spec', { query: Q + '&specialist=ready', hash: '#/settings' }, async ({ p, click, mark, wait }) => {
    await wait(1200);
    const card = p.locator('[data-specialist="settings"]'); await card.scrollIntoViewIfNeeded(); await wait(600); mark('card');
    await click(card.locator('[role="switch"], .switch').first()); mark('on'); await wait(1600);
    await click(p.locator('.rail-item').first()); await wait(900); mark('chat');
    await click(p.locator('.ask textarea')); await p.keyboard.type('Why is my PC slow?', { delay: 45 });
    await click(p.locator('.send')); mark('sent');
    await wait(6500); mark('steps');
    await p.waitForSelector('.answer-foot', { timeout: 40000 }); mark('answer'); await wait(800);
    const fold = p.locator('.steps-fold summary, .steps-fold button').first(); if (await fold.count()) { await click(fold); mark('open'); } await wait(2500); mark('end');
  }),
  ram: () => session('ram', { query: Q + '&web=on' }, async ({ p, click, mark, wait }) => {
    await wait(1000);
    await click(p.locator('.ask textarea')); await p.keyboard.type('Do I need more RAM?', { delay: 45 });
    await click(p.locator('.send')); mark('sent');
    await p.waitForSelector('.answer-foot', { timeout: 40000 }); mark('answer'); await wait(800);
    const w = p.locator('.answer-text .cite').filter({ hasText: '3' }).first(); await click(w); mark('cite'); await wait(3000); mark('end');
  }),
};
(async () => { for (const [k, f] of Object.entries(scenes)) if (!only || only === k) { try { await f(); } catch (e) { console.log(k, 'FAILED', e.message.split('\n')[0]); } } })();
