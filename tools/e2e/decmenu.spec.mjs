// Decoder-Menü: Live-Zustand aus /api/decoders, „Hören“ stimmt ab. Aufruf: node tools/e2e/decmenu.spec.mjs <url> [erwartete Anzahl]
import { chromium } from 'playwright';
const base = process.argv[2] || 'http://127.0.0.1:8082', want = Number(process.argv[3] || 0);
const b = await chromium.launch(); const p = await b.newPage({ viewport: { width: 1500, height: 900 }, colorScheme: 'dark' });
const errors = []; p.on('pageerror', e => errors.push(e.message));
const check = (n, ok, i) => { console.log((ok ? 'OK  ' : 'FEHL') + ' ' + n + (i !== undefined ? ' → ' + JSON.stringify(i) : '')); if (!ok) errors.push(n); };
await p.goto(base + '/'); await p.waitForFunction(() => window._crab && _crab.started, null, { timeout: 15000 }); await p.waitForTimeout(1500);
if (!want) { const has = await p.evaluate(() => !!document.querySelector('.x1decbtn') && document.querySelector('.x1dec').isConnected);
  check('ohne Decoder: kein Decoder-Knopf', !has); await b.close(); process.exit(errors.length ? 1 : 0); }
await p.click('.x1decbtn'); await p.waitForTimeout(1500);
const items = await p.evaluate(() => [...document.querySelectorAll('.x1decitem')].map(d => ({ t: d.querySelector('b').textContent, st: d.querySelector('.x1decst').textContent, meta: d.querySelector('.x1decmeta').textContent, last: d.querySelectorAll('.x1declast div').length })));
check('Menü zeigt die Decoder', want ? items.length === want : true, items);
if (want) {
  await p.screenshot({ path: process.argv[4] || '/tmp/decmenu.png', clip: { x: 1000, y: 200, width: 500, height: 700 } });
  const i = items.findIndex(x => /FT8/.test(x.t));
  if (i >= 0) { await p.locator('.x1decitem').nth(i).locator('[data-tune]').click(); await p.waitForTimeout(800);
    const s = await p.evaluate(() => ({ band: bi[band].name, freq, mode })); check('„Hören“ bei FT8: 2 m, USB, 144,174', s.band === '2m' && s.mode === 'USB' && Math.abs(s.freq - 144174) < 0.01, s); }
} else check('ohne Decoder: Hinweis', (await p.textContent('#x1decpop')).includes('Noch keine Decoder'));
check('keine JS-Fehler', errors.length === 0, errors.slice(0, 3));
await b.close(); process.exit(errors.length ? 1 : 0);
