// Rauchtest für das neutrale Paket (web/ im Docker-Image oder install.sh): Seite, Wasserfall, Ton, Abstimmen, Bandwechsel.
// Aufruf: node tools/e2e/package-smoke.mjs http://127.0.0.1:18081 [Erwarteter Stationsname]
import { chromium } from 'playwright';
const base = process.argv[2] || 'http://127.0.0.1:8080';
const want = process.argv[3] || '';
const browser = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
const errors = [];
page.on('pageerror', e => errors.push('pageerror: ' + e.message));
page.on('console', m => { if (m.type() === 'error' && !/404|status.json|gains.json|ServiceWorker/.test(m.text())) errors.push('console: ' + m.text()); });
const check = (name, ok, info) => { console.log((ok ? 'OK  ' : 'FEHL') + ' ' + name + (info !== undefined ? ' → ' + JSON.stringify(info) : '')); if (!ok) errors.push(name); };
const t0 = Date.now();
await page.goto(base + '/?tune=438900fm');
await page.waitForFunction(() => window._crab && _crab.bands[band] && _crab.bands[band].started, null, { timeout: 20000 });
check('Wasserfall gestartet', true, (Date.now() - t0) + ' ms');
const title = await page.title();
check('Stationsname im Titel', !want || title.includes(want), title);
check('Stationsname aus der Konfiguration im Kopf', (await page.content()).includes('<title>'));
await page.evaluate(() => document.getElementById('audiobtn') && document.getElementById('audiobtn').click());
// FM startet mit Rauschsperre; auf einer leeren Frequenz käme dann kein Ton -> für die Messung aus
await page.evaluate(() => { if (typeof setSquelch === 'function') setSquelch(false); });
await page.waitForTimeout(3000);
// Wasserfall darf nicht schwarz sein (Verlauf beim Öffnen + laufende Zeilen)
const lit = await page.evaluate(() => {
  const c = document.querySelector('#wfdiv0 canvas'); if (!c) return -1;
  const d = c.getContext('2d').getImageData(0, 0, c.width, Math.min(c.height, 100)).data; let n = 0;
  // Rauschboden der dunklen Palette ist fast schwarz-blau; „Inhalt“ = kein reines Schwarz und helle Signalspuren
  let base = 0, hot = 0;
  for (let i = 0; i < d.length; i += 4) { const s = d[i] + d[i + 1] + d[i + 2]; if (s > 15) base++; if (s > 300) hot++; }
  return { flaeche: base / (d.length / 4), signale: hot / (d.length / 4) };
});
check('Wasserfall hat Inhalt', lit.flaeche > 0.8 && lit.signale > 0.002, { flaeche: Math.round(lit.flaeche * 100) + ' %', signale: (lit.signale * 100).toFixed(1) + ' %' });
const a0 = await page.evaluate(() => _crab.audioFrames || 0);
await page.waitForTimeout(5000);
const a1 = await page.evaluate(() => ({ n: _crab.audioFrames || 0, pcm: _crab.audio.pcm, under: _crab.underruns || 0, level: _crab.level }));
const pk = a1.n - a0;   // Tonrahmen (Opus 50/s, PCM je nach Blockgröße)
check('Ton läuft (Tonrahmen in 5 s)', pk > 150, { rahmen: pk, art: a1.pcm ? 'PCM (kein WebCodecs, z. B. http:// im LAN)' : 'Opus', unterlaeufe: a1.under, pegel: a1.level });
const f0 = await page.evaluate(() => freq);
const wf = await page.locator('#wfdiv0').boundingBox();
await page.mouse.click(wf.x + wf.width * 0.3, wf.y + wf.height / 2);
const f1 = await page.evaluate(() => freq);
check('Klick im Wasserfall stimmt ab', f1 !== f0, [f0, f1]);
const nb = await page.evaluate(() => nWaterfalls || 1);
if (await page.evaluate(() => typeof setBand === 'function' && bandinfo.length > 1)) {
  await page.evaluate(() => setBand(1)); await page.waitForTimeout(2500);
  const b = await page.evaluate(() => ({ band, started: !!(_crab.bands[band] && _crab.bands[band].started) }));
  check('Bandwechsel', b.band === 1 && b.started, b);
}
check('keine JS-Fehler', errors.length === 0, errors.slice(0, 3));
await browser.close();
process.exit(errors.length ? 1 : 0);
