// Ende-zu-Ende-Prüfung der Hören-Seite (Playwright, Chromium).
// Aufruf: node tools/e2e/ui.spec.mjs [http://127.0.0.1:8082]
import { chromium } from 'playwright';
const base = process.argv[2] || 'http://127.0.0.1:8082';
const browser = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
const errors = [];
page.on('pageerror', e => errors.push('pageerror: ' + e.message));
page.on('console', m => { if (m.type() === 'error' && !/404|logbuch|status.json|gains.json|ServiceWorker/.test(m.text())) errors.push('console: ' + m.text()); });
const t0 = Date.now();
await page.goto(base + '/?tune=145500fm');
await page.waitForFunction(() => window._crab && _crab.bands[band] && _crab.bands[band].started, null, { timeout: 15000 });
const st = async () => page.evaluate(() => ({ freq, mode, lo, hi, band, view: Number(view), nWaterfalls, zoom: bi[band].zoom, khzPerPx, level: _crab.level, ts: _crab.audio.ts, af: _crab.audioFrames || 0, pcm: _crab.audio.pcm, under: _crab.underruns || 0, ct: document.ct && document.ct.state, mhz: document.getElementById('freqmhz').textContent }));
const s0 = await st();
const check = (name, ok, info) => { console.log((ok ? 'OK  ' : 'FEHL') + ' ' + name + (info !== undefined ? ' → ' + JSON.stringify(info) : '')); if (!ok) errors.push(name); };
check('Deep-Link 145500 FM', s0.freq === 145500 && s0.mode === 'FM', s0);
check('Wasserfall gestartet in < 15 s', true, (Date.now() - t0) + ' ms');
// Klick in den Wasserfall → Frequenz ändert sich (Raster 6,25 kHz aus ui.json)
const wf = await page.locator('#wfdiv0').boundingBox();
await page.mouse.click(wf.x + wf.width * 0.3, wf.y + wf.height / 2);
const s1 = await st();
check('Klick im Wasserfall stimmt ab', s1.freq !== s0.freq && Math.abs((s1.freq * 1000) % 6250) < 1, s1.freq);
// Betriebsart, Bandbreite, Zoom
await page.click('#modes [data-mode="USB"]');
await page.evaluate(() => setZoom(0));
await page.waitForTimeout(800);
const s2 = await st();
check('USB + Zoom 1', s2.mode === 'USB' && s2.zoom === 1 && s2.khzPerPx === 1, s2);
await page.evaluate(() => setZoom(2)); await page.waitForTimeout(1200);
const sz = await st(); const szp = await page.evaluate(() => [bi[band].szoom, bi[band].sstart, bi[band].start, bi[band].effsamplerate, !!_crab.bands[band].prev]);
check('Max. Zoom (Stufe 7 = 16 kHz)', sz.zoom === 7 && Math.abs(sz.khzPerPx - 2048 / 128 / 1024) < 1e-9 && szp[4], szp);
await page.evaluate(() => setZoom(4));
// Band dazuschalten per Bandleiste (blendet nur ein), dann in dessen Wasserfall klicken → Band aktiv
await page.click('#bandbar .band[data-band="1"]');
await page.waitForTimeout(1500);
{ const i = await page.evaluate(() => band2id(1)); const b1 = await page.locator('#wfdiv' + i).boundingBox(); await page.mouse.click(b1.x + b1.width * 0.5, b1.y + b1.height / 2); }
await page.waitForTimeout(800);
const s3 = await st();
check('Band 2 aktiv, Wasserfall läuft', s3.band === 1 && (await page.evaluate(() => _crab.bands[1].started)), s3.band);
// Alle Bänder: alle Reiter anwählen (Bandauswahl seit 27.09.)
await page.evaluate(() => document.querySelectorAll('#bandbar .band[data-band]:not(.shown):not(.off)').forEach(b => b.click()));
await page.waitForTimeout(3000);
const s4 = await st();
const started4 = await page.evaluate(() => _crab.bands.map(B => [B.started, B.ws ? B.ws.readyState : null, !!B.prev]));
const nb = await page.evaluate(() => nbands);
check('Alle Bänder: alle Wasserfälle laufen', s4.view === 0 && s4.nWaterfalls === nb && started4.every(x => x[0]), { view: s4.view, n: s4.nWaterfalls, started4 });
// Ton: 10 s laufen lassen, Opus-Pakete und Unterläufe zählen (Rauschsperre aus: FM startet mit Sperre, die Aufnahme ist leise)
await page.evaluate(() => { if (typeof setSquelch === 'function') setSquelch(false); document.getElementById('audiobtn').click(); });
const a0 = await st(); await page.waitForTimeout(10000); const a1 = await st();
check('Ton läuft (Tonrahmen in 10 s)', a1.af - a0.af >= 400, (a1.af - a0.af) + ' Rahmen (' + (a1.pcm ? 'PCM, kein WebCodecs – unsicherer Kontext?' : 'Opus') + '), AudioContext ' + a1.ct);
check('Keine Puffer-Unterläufe', a1.under - a0.under === 0, a1.under - a0.under);
// Rauschsperre, Aufnahme, Teilen-Link
await page.evaluate(() => document.getElementById('squelchcheckbox').click());
await page.evaluate(() => recStart()); await page.waitForTimeout(1500);
const url = await page.evaluate(() => recStop());
check('Aufnahme liefert WAV-URL', /^blob:/.test(url || ''), url && url.slice(0, 20));
await page.screenshot({ path: 'testdata/data-ui/e2e-allbands.png' });
await browser.close();
console.log(errors.length ? 'FEHLER: ' + errors.join(' | ') : 'ALLE PRÜFUNGEN OK');
process.exit(errors.length ? 1 : 0);
