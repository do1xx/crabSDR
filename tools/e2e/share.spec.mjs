// „Bildschirm teilen“: angezeigte Wasserfälle teilen sich die sichtbare Höhe gleichmäßig.
// Aufruf: node tools/e2e/share.spec.mjs [http://127.0.0.1:8082]
import { chromium } from 'playwright';
const base = process.argv[2] || 'http://127.0.0.1:8082';
const browser = await chromium.launch();
const page = await (await browser.newContext({ viewport: { width: 1500, height: 950 } })).newPage();
const errors = [];
page.on('pageerror', e => errors.push('pageerror: ' + e.message));
const check = (name, ok, info) => { console.log((ok ? 'OK  ' : 'FEHL') + ' ' + name + (info !== undefined ? ' → ' + JSON.stringify(info) : '')); if (!ok) errors.push(name); };
const geo = () => page.evaluate(() => { const m = document.getElementById('main').getBoundingClientRect(), w = document.getElementById('rxwrap').getBoundingClientRect();
  const hs = [...document.querySelectorAll('#waterfalls > div > div[id^=wfdiv]')].map(d => Math.round(d.getBoundingClientRect().height));
  return { n: nWaterfalls, wh: wfHeight, hs, wrapBottom: Math.round(w.bottom), mainBottom: Math.round(m.bottom), free: Math.round(m.bottom - w.bottom), wf: document.getElementById('wfsize').value }; });
const ok = g => g.free >= 0 && g.free < 40 && Math.max(...g.hs) - Math.min(...g.hs) <= 1;
await page.goto(base + '/'); await page.waitForFunction(() => window._crab && _crab.started && wfWaiting === 0, null, { timeout: 15000 }); await page.waitForTimeout(1500);
let g = await geo(); check('1 Band füllt die Höhe', g.wf === 'share' && g.n === 1 && ok(g), g);
const counts = [];
for (const b of [1, 2, 3, 4]) {
  await page.click(`#bandbar .band[data-band="${b}"]`); await page.waitForTimeout(2500);
  g = await geo(); counts.push(g.wh); check(`${g.n} Bänder teilen sich die Höhe`, ok(g), g);
}
check('Mehr Bänder → niedrigere Wasserfälle', counts.every((h, i) => i === 0 || h < counts[i - 1]), counts);
await page.click('#bandbar .band[data-band="4"]'); await page.click('#bandbar .band[data-band="3"]'); await page.click('#bandbar .band[data-band="2"]'); await page.waitForTimeout(2500);
g = await geo(); check('Zurück auf 2 Bänder: je halbe Höhe', g.n === 2 && ok(g) && g.wh > counts[0] - 3, g);
await page.setViewportSize({ width: 1300, height: 760 }); await page.waitForTimeout(1500);
g = await geo(); check('Kleineres Fenster: passt weiter', g.n === 2 && ok(g), g);
check('keine JS-Fehler', errors.length === 0, errors.slice(0, 3));
await browser.close(); process.exit(errors.length ? 1 : 0);
