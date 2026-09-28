// 60-s-Hörtest ohne Ohren: Opus-Pakete, Puffer-Unterläufe und Latenz-Puffer über Bandwechsel und Zoom hinweg.
import { chromium } from 'playwright';
const base = process.argv[2] || 'http://127.0.0.1:8082';
const browser = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
await page.goto(base + '/?tune=438900fm');
await page.waitForFunction(() => window._crab && _crab.bands[band] && _crab.bands[band].started, null, { timeout: 15000 });
await page.evaluate(() => document.getElementById('audiobtn').click());
const st = () => page.evaluate(() => ({ ts: _crab.audio.ts, under: _crab.underruns || 0, band, freq }));
let a = await st(); const t0 = Date.now(); const steps = [];
for (let i = 0; i < 6; i++) {
  await page.waitForTimeout(10000);
  const b = await st(); steps.push({ s: (i + 1) * 10, pakete: (b.ts - a.ts) / 20000, unter: b.under - a.under }); a = b;
  if (i === 1) await page.evaluate(() => setFreqText('438650'));
  if (i === 2) await page.evaluate(() => setBand(1));
  if (i === 3) await page.evaluate(() => { setBand(2); setFreqText('438900'); setZoom(2); });
  if (i === 4) await page.evaluate(() => document.querySelectorAll('#bandbar .band[data-band]:not(.shown):not(.off)').forEach(b => b.click()));
}
console.log(JSON.stringify(steps));
const total = steps.reduce((x, s) => x + s.pakete, 0), under = steps.reduce((x, s) => x + s.unter, 0);
console.log(`60 s: ${total} Opus-Pakete (Soll ≈ 3000 abzüglich Umschaltpausen), Unterläufe gesamt ${under}`);
await browser.close();
