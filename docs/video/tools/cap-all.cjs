const { session } = require('./cap-lib.cjs');
const Q = '?cloud=ready&privacy=complete';
const only = process.argv[2];
const scenes = {
  chat: () => session('chat', { query: Q }, async ({ p, click, mark, wait }) => {
    await wait(1200); mark('home');
    await click(p.locator('.ask textarea')); await p.keyboard.type('Why is my PC slow?', { delay: 55 }); mark('typed');
    await click(p.locator('.send')); mark('sent');
    await p.waitForSelector('.answer-foot', { timeout: 40000 }); mark('answer'); await wait(1500);
    await click(p.locator('.answer-text .cite').first()); mark('cite');
    await p.waitForSelector('.panel.open .source-card.focused'); await wait(2500); mark('end');
  }),
  action: () => session('action', { query: Q + '&tour=actions' }, async ({ p, click, mark, wait }) => {
    await wait(800);
    await p.locator('.ask textarea').fill('Why is my PC slow? Anything I can clean?'); await p.locator('.send').click();
    await p.waitForSelector('.action-card .btn', { timeout: 40000 }); await wait(400);
    await p.locator('.action-card').first().scrollIntoViewIfNeeded(); await wait(600); mark('card');
    await click(p.locator('.action-card .btn').first()); await p.waitForSelector('.dialog .plan-targets'); mark('dialog'); await wait(2800);
    const run = p.locator('.dialog button', { hasText: /Run now/ }); 
    if (await run.count()) { await click(run.first(), 500); mark('run'); await wait(3500); }
    mark('end');
  }),
  privacy: () => session('privacy', { query: Q }, async ({ p, click, mark, wait }) => {
    await wait(1000);
    await click(p.locator('.ask textarea')); await p.keyboard.type('My PC is slow when I open Chrome', { delay: 35 }); mark('typed');
    await click(p.locator('.share-preview').first()); mark('preview'); await wait(3500);
    await p.keyboard.press('Escape'); await wait(500);
    await click(p.locator('[data-share-key="names"], .share-toggle').first()); mark('names'); await wait(1200);
    await click(p.locator('.share-preview').first()); mark('preview2'); await wait(3000); mark('end');
  }),
  live: () => session('live', { query: Q, theme: 'dark', hash: '#/live', zoom: 1.15 }, async ({ p, click, mark, wait }) => {
    await p.waitForSelector('.livepage-card'); await wait(2500); mark('cards');
    await click(p.locator('.livepage-card').first()); mark('select'); await wait(2200);
    const mon = p.locator('[data-key="actbar:monitor"]');
    if (await mon.count()) { await click(mon.first()); mark('monitor'); await wait(2800); }
    mark('end');
  }),
  watches: () => session('watches', { query: Q, theme: 'dark', hash: '#/watches' }, async ({ p, mark, wait, move }) => {
    await wait(2500); mark('top'); await move(900, 600);
    await p.mouse.wheel(0, 500); await wait(2500); mark('end');
  }),
  report: () => session('report', { query: Q }, async ({ p, mark, wait, move }) => {
    await wait(1500); await move(1000, 700);
    for (let i = 0; i < 3; i++) { await p.mouse.wheel(0, 110); await wait(140); }
    mark('report'); await wait(3000); mark('end');
  }),
  web: () => session('web', { query: Q + '&web=on' }, async ({ p, click, mark, wait }) => {
    await wait(1000);
    await click(p.locator('.ask textarea')); await p.keyboard.type('Is there a known issue with my NVIDIA driver?', { delay: 35 });
    await click(p.locator('.send')); mark('sent');
    await p.waitForSelector('.answer-foot', { timeout: 40000 }); mark('answer'); await wait(800);
    const w = p.locator('.answer-text .cite', { hasText: /W|1/ }).last(); await click(w); mark('cite'); await wait(3000); mark('end');
  }),
  search: () => session('search', { query: Q }, async ({ p, click, mark, wait }) => {
    await p.locator('.ask textarea').fill('Why is my PC slow?'); await p.locator('.send').click();
    await p.waitForSelector('.answer-foot', { timeout: 40000 }); await wait(500);
    await p.locator('.new-chat, button', { hasText: 'New conversation' }).first().click(); await wait(800);
    await p.locator('.ask textarea').fill('What can I delete on C:?'); await p.locator('.send').click();
    await p.waitForSelector('.answer-foot', { timeout: 40000 }); await wait(800); mark('ready');
    const s = p.locator('.sidebar input[type="search"], .sidebar input').first();
    await click(s); await p.keyboard.type('chrome', { delay: 90 }); mark('typed'); await wait(2500); mark('end');
  }),
  settings: () => session('settings', { query: Q, hash: '#/settings' }, async ({ p, mark, wait, move }) => {
    await wait(1500); await move(1000, 600); mark('top');
    const card = p.locator('[data-specialist="settings"]');
    if (await card.count()) { await card.scrollIntoViewIfNeeded(); }
    await wait(2500); mark('end');
  }),
};
(async () => { for (const [k, f] of Object.entries(scenes)) if (!only || only === k) { try { await f(); } catch (e) { console.log(k, 'FAILED', e.message.split('\n')[0]); } } })();
