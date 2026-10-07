// [ui]-Schalter und Voreinstellungen ohne Stationsordner: Logbuch/Info eingebaut, Digital ohne Decoder weg, Chat aus, Banner, Impressum,
// Krabbe als Logo ohne zweite Marke. Aufruf: node tools/e2e/features.spec.mjs http://127.0.0.1:8083
import { chromium } from 'playwright';
const base = process.argv[2] || 'http://127.0.0.1:8083';
const b = await chromium.launch(); const p = await b.newPage({ viewport: { width: 1400, height: 900 } });
const errors = []; p.on('pageerror', e => errors.push(e.message));
const check = (n, ok, i) => { console.log((ok ? 'OK  ' : 'FEHL') + ' ' + n + (i !== undefined ? ' → ' + JSON.stringify(i) : '')); if (!ok) errors.push(n); };
await p.goto(base + '/'); await p.waitForFunction(() => window._crab && _crab.started, null, { timeout: 15000 }); await p.waitForTimeout(1500);
const r = await p.evaluate(() => {
  const vis = el => !!el && getComputedStyle(el).display !== 'none';
  return { title: document.title, tabs: [...document.querySelectorAll('.tabs a')].filter(vis).map(a => a.textContent), chat: vis(document.getElementById('chatbtn')),
    banner: (document.getElementById('x1banner') || {}).textContent, links: [...document.querySelectorAll('#x1links a')].map(a => a.textContent + ' ' + a.href),
    logo: document.querySelector('.logo').getAttribute('src'), brand: !!document.getElementById('crabbrand'), bands: [...document.querySelectorAll('#bandbar .band b')].map(e => e.textContent.trim()) };
});
check('Titel aus [station]', r.title === 'Teststation', r.title);
check('Logbuch und Info eingebaut, Digital nur mit Decodern', r.tabs.join() === 'Hören,Logbuch,Info', r.tabs);
check('Chat aus', !r.chat);
check('Banner', r.banner === 'Wartung heute ab 20 Uhr', r.banner);
check('Impressum-Link', r.links.length === 1 && r.links[0].startsWith('Impressum https://example.org/impressum'), r.links);
check('Krabbe als Logo, keine zweite Marke', r.logo === 'logo.svg' && !r.brand, [r.logo, r.brand]);
check('Bandnamen mit Bereich', r.bands.length >= 2 && /MHz/.test(r.bands[0]), r.bands);
check('keine JS-Fehler', errors.length === 0, errors.slice(0, 3));
await b.close(); process.exit(errors.length ? 1 : 0);
