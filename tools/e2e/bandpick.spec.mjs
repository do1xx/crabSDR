// Bandauswahl (Desktop): Reiter schalten Wasserfälle an/aus, Klick in den Wasserfall stimmt mit Raster ab.
// Aufruf: node tools/e2e/bandpick.spec.mjs [http://127.0.0.1:8082]
import { chromium } from 'playwright';
const base = process.argv[2] || 'http://127.0.0.1:8082';
const browser = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
const ctx = await browser.newContext({ viewport: { width: 1400, height: 1000 } });
const page = await ctx.newPage();
const errors = [];
page.on('pageerror', e => errors.push('pageerror: ' + e.message));
page.on('console', m => { if (m.type() === 'error' && !/404|logbuch|status.json|gains.json|ServiceWorker/.test(m.text())) errors.push('console: ' + m.text()); });
const check = (name, ok, info) => { console.log((ok ? 'OK  ' : 'FEHL') + ' ' + name + (info !== undefined ? ' → ' + JSON.stringify(info) : '')); if (!ok) errors.push(name); };
const st = () => page.evaluate(() => ({ n: nWaterfalls, sel: _crab.sel.slice(), band, freq, view: Number(view),
  ws: _crab.bands.map(B => !!(B.ws && B.ws.readyState <= 1)), shown: [...document.querySelectorAll('#bandbar .band.shown')].map(b => Number(b.dataset.band)),
  started: _crab.sel.map(b => !!(_crab.bands[b] && _crab.bands[b].started)) }));
const tab = n => page.click(`#bandbar .band[data-band="${n}"]`);
const settle = (ms = 1500) => page.waitForTimeout(ms);
await page.goto(base + '/');
await page.waitForFunction(() => window._crab && _crab.started && _crab.bands[band] && _crab.bands[band].started, null, { timeout: 15000 });
let s = await st();
check('Erster Besuch: ein Wasserfall', s.n === 1 && s.sel.length === 1 && s.view === 0, s);
check('„Alle Bänder zeigen“ ausgeblendet, Raster sichtbar', await page.evaluate(() => getComputedStyle(document.getElementById('allbandschk').closest('label')).display === 'none' && getComputedStyle(document.querySelector('label.snap')).display !== 'none'));
check('Raster-Stufen', (await page.evaluate(() => [...document.querySelectorAll('#snapsel option')].map(o => o.value))).join(',') === '0,1,2.5,5,6.25,8.33,10,12.5,20,25');
// Vorher im ersten Band eine Marke setzen (USB, feste Frequenz)
await page.click('#modes [data-mode="USB"]'); await page.evaluate(() => setFreqText('145212.5')); await settle(400);
const f0 = await page.evaluate(() => [freq, mode]);
await tab(1); await settle(2500); s = await st();
check('Band 2 zugeschaltet: zwei Wasserfälle, Empfänger bleibt auf Band 1', s.n === 2 && s.band === 0 && s.freq === f0[0] && s.sel.join() === '0,1' && s.shown.join() === '0,1' && s.started.every(Boolean), s);
check('Nur angezeigte Bänder verbunden', s.ws[0] && s.ws[1] && !s.ws[2], s.ws);
await tab(2); await settle(2500); s = await st();
check('Band 3 zugeschaltet: drei Wasserfälle, Marke unverändert', s.n === 3 && s.band === 0 && s.freq === f0[0], s);
const hist = await page.evaluate(() => { const B = _crab.bands[2]; const d = B.ctx.getImageData(0, Math.floor(B.canvas.height * 0.8), 1024, 1).data; let n = 0; for (let i = 0; i < d.length; i += 4) if (d[i] + d[i + 1] + d[i + 2] > 15) n++; return n / 1024; });
check('Neu eingeblendeter Wasserfall hat Verlauf (unten nicht schwarz)', hist > 0.5, Math.round(hist * 100) + ' %');
// Klick in den zweiten Wasserfall: dort Marke, Band 1 bekommt Parkmarke
await page.selectOption('#snapsel', '12.5');
let bb = await page.locator('#wfdiv1').boundingBox();
await page.mouse.click(bb.x + bb.width * 0.37, bb.y + bb.height / 2); await settle(500); s = await st();
check('Klick in Wasserfall 2: Band 2 aktiv, 12,5-kHz-Raster', s.band === 1 && Math.abs((s.freq * 1000) % 12500) < 1, s.freq);
const park = await page.evaluate(() => { const el = _crab.bands[0].park; return el && el.style.display !== 'none' ? el.firstChild.textContent : null; });
check('Keine Parkmarke im nicht gehörten Band', park === null, park);
await page.selectOption('#snapsel', '6.25');
await page.mouse.click(bb.x + bb.width * 0.41, bb.y + bb.height / 2); await settle(400); s = await st();
check('Raster 6,25 kHz', Math.abs((s.freq * 1000) % 6250) < 1, s.freq);
await page.selectOption('#snapsel', '5');
await page.mouse.click(bb.x + bb.width * 0.53, bb.y + bb.height / 2); await settle(400); s = await st();
check('Raster 5 kHz', Math.abs((s.freq * 1000) % 5000) < 1, s.freq);
// Klick in den ersten Wasserfall: zurück auf Band 1; ohne Parkmarke stimmt der Klick dort neu ab
const b0 = await page.locator('#wfdiv0').boundingBox();
await page.mouse.click(b0.x + b0.width * 0.3, b0.y + b0.height / 2); await settle(500);
const back = await page.evaluate(() => [band, freq]);
check('Klick in Wasserfall 1: Band 1 aktiv', back[0] === 0, back);
// Aktives Band abwählen → ein anderes angezeigtes wird aktiv
await tab(0); await settle(1500); s = await st();
check('Aktives Band 1 abgewählt: zwei Wasserfälle, anderes Band aktiv', s.n === 2 && s.sel.join() === '1,2' && s.band === 1 && !s.ws[0], s);
// Neu laden: Auswahl und Raster bleiben
await page.reload();
await page.waitForFunction(() => window._crab && _crab.started && nWaterfalls >= 1, null, { timeout: 15000 }); await settle(2000); s = await st();
check('Nach Neuladen: Auswahl und zuletzt gehörtes Band gespeichert', s.n === 2 && s.sel.join() === '1,2' && s.band === 1, s);
check('Nach Neuladen: Raster 5 kHz gespeichert', await page.evaluate(() => document.getElementById('snapsel').value) === '5');
// Deep-Link / Schnellwahl auf ausgeblendetes Band blendet es ein
await page.evaluate(() => setBand(0)); await settle(2500); s = await st();
check('Abstimmen auf ausgeblendetes Band blendet es ein', s.n === 3 && s.band === 0 && s.started.every(Boolean), s);
// Letztes Band lässt sich nicht abwählen
await tab(0); await settle(800); await tab(1); await settle(800); await tab(2); await settle(1200); s = await st();
check('Letztes Band bleibt stehen', s.n === 1 && s.sel.join() === '2' && s.band === 2, s);
// Handy: weiter ein Band mit Reitern zum Wechseln
const m = await browser.newPage({ viewport: { width: 390, height: 800 }, isMobile: true, hasTouch: true });
await m.goto(base + '/'); await m.waitForFunction(() => window._crab && _crab.started, null, { timeout: 15000 }); await m.waitForTimeout(1500);
await m.click('#bandbar .band[data-band="1"]'); await m.waitForTimeout(1500);
const ms = await m.evaluate(() => ({ n: nWaterfalls, band, view: Number(view) }));
check('Handy: ein Wasserfall, Reiter wechselt das Band', ms.n === 1 && ms.band === 1 && ms.view === 2, ms);
check('keine JS-Fehler', errors.length === 0, errors.slice(0, 4));
await browser.close();
process.exit(errors.length ? 1 : 0);
