// Admin-Seite und Anmeldung auf der Hören-Seite im Browser (Playwright). Frische Testumgebung wie security_test.py:
//   node tools/e2e/admin.spec.mjs http://127.0.0.1:8090 <Startpasswort von admin> [Ordner für Bilder]
import { chromium } from 'playwright';
const [base, startPw, shots] = [process.argv[2] || 'http://127.0.0.1:8090', process.argv[3], process.argv[4]];
const b = await chromium.launch();
const ctx = await b.newContext({ viewport: { width: 1300, height: 900 }, colorScheme: 'dark' });
const p = await ctx.newPage();
const errors = [];
p.on('pageerror', e => errors.push('pageerror: ' + e.message));
p.on('console', m => { if (m.type() === 'error' && !/status 4\d\d|Failed to load resource/.test(m.text())) errors.push('console: ' + m.text()); if (/Content Security Policy/.test(m.text())) errors.push('CSP: ' + m.text()); });
const check = (n, ok, i) => { console.log((ok ? 'OK  ' : 'FEHL') + ' ' + n + (i !== undefined ? ' → ' + JSON.stringify(i) : '')); if (!ok) errors.push(n); };
const shot = async n => { if (shots) await p.screenshot({ path: `${shots}/admin-${n}.png` }); };
const tab = async t => { await p.click(`#tabs button[data-tab="${t}"]`); await p.waitForTimeout(700); };
const toast = () => p.evaluate(() => document.getElementById('toast').textContent);

await p.goto(base + '/admin/');
await p.fill('.adm-login input[autocomplete=username]', 'admin');
await p.fill('.adm-login input[type=password]', startPw);
await p.click('.adm-login button[type=submit]');
await p.waitForTimeout(800);
check('Startpasswort: Pflicht zum Ändern', await p.isVisible('text=Eigenes Passwort festlegen'));
const pws = await p.$$('.adm-login input[type=password]');
await pws[0].fill('Admin-Passwort-im-Browser'); await pws[1].fill('Admin-Passwort-im-Browser');
await p.click('.adm-login button[type=submit]');
await p.waitForTimeout(1200);
check('Überblick nach Passwortwechsel', await p.isVisible('.adm-stat'), await p.evaluate(() => document.querySelectorAll('.adm-stat').length));
check('Platte im Überblick', await p.evaluate(() => [...document.querySelectorAll('.adm-stat small')].some(e => /^Platte /.test(e.textContent))));
await shot('ueberblick');

await tab('station');
await p.fill('.adm-card input >> nth=0', 'Admin-Test');
await p.click('text=Speichern');
await p.waitForTimeout(800);
check('Station gespeichert', /Gespeichert/.test(await toast()), await toast());
check('Hinweis Neustart', await p.isVisible('#restartbar'));
check('Schalter Verzeichnis da, aus', await p.isVisible('text=Im Verzeichnis auf') && !(await p.isChecked('.adm-check input')));
await shot('station');

await tab('bands');
const n = await p.$$eval('#view > form.adm-card', f => f.length);
check('Bänder als Formulare', n === 5, n);
await p.fill('#view > form.adm-card >> nth=0 >> input >> nth=2', '146,0');   // Mittenfrequenz? (drittes Feld)
const bandFields = await p.$$eval('#view > form.adm-card:first-of-type .adm-field span', s => s.map(x => x.textContent));
await p.locator('#view > form.adm-card').first().locator('button[type=submit]').click();
await p.waitForTimeout(800);
check('Band gespeichert', /Gespeichert|Nichts/.test(await toast()), [await toast(), bandFields.slice(0, 4)]);
await p.click('text=Band hinzufügen');
const add = p.locator('details.adm-card form').first();
await add.locator('input').nth(0).fill('23cm');
await add.locator('input').nth(1).fill('23 cm Test');
await add.locator('input').nth(2).fill('1296,2');
await add.locator('button[type=submit]').click();
await p.waitForTimeout(1000);
check('Band angehängt', (await p.$$eval('#view > form.adm-card', f => f.length)) === 6);
p.once('dialog', d => d.accept());
await p.locator('#view > form.adm-card').nth(5).locator('text=Band entfernen').click();
await p.waitForTimeout(1000);
check('Band entfernt', (await p.$$eval('#view > form.adm-card', f => f.length)) === 5);
await shot('baender');

await tab('decoders');
check('Decoder als Formulare', (await p.$$eval('#view > form.adm-card', f => f.length)) === 2);
await p.locator('#view > form.adm-card').first().locator('text=+ Option').click();
await p.locator('#view > form.adm-card').first().locator('.adm-opts input.k').last().fill('mycall');
await p.locator('#view > form.adm-card').first().locator('.adm-opts input.v').last().fill('DO1XX-10');
await p.locator('#view > form.adm-card').first().locator('button[type=submit]').click();
await p.waitForTimeout(900);
check('Decoder-Option gespeichert', /Gespeichert/.test(await toast()), await toast());

await tab('users');
await p.click('text=Benutzer anlegen');
const nu = p.locator('details.adm-card form').last();
await nu.locator('input').first().fill('dk0tst');
await nu.locator('.adm-picks input[value="70cm-oben"]').check();
await nu.locator('button[type=submit]').click();
await p.waitForTimeout(1200);
const out = await nu.locator('.adm-msg.ok').textContent();
const userPw = (out.match(/: ([A-Za-z0-9]{16}) /) || [])[1];
check('Benutzer angelegt, Passwort einmal gezeigt', !!userPw, out);
await shot('benutzer');

await tab('config');
check('Datei im Editor', (await p.inputValue('textarea.adm-editor')).includes('[[bands]]'));
await p.click('text=Prüfen'); await p.waitForTimeout(600);
check('Prüfen: in Ordnung', await p.isVisible('text=In Ordnung'));
const txt = await p.inputValue('textarea.adm-editor');
await p.fill('textarea.adm-editor', txt.replace('port = 8090', 'port = 81'));
await p.click('.adm-row >> text=Speichern'); await p.waitForTimeout(800);
check('gesperrter Port: nicht gespeichert, Fehler sichtbar', await p.isVisible('li.bad'), await p.evaluate(() => [...document.querySelectorAll('li.bad')].map(l => l.textContent)));
await shot('konfiguration');
await tab('chat'); check('Chat & Logbuch lädt', await p.isVisible('text=Logbuch'));
await tab('devices'); check('Sticks lädt', await p.isVisible('text=Seriennummer') || await p.isVisible('text=Keine Empfänger'));

// Hören-Seite: Anmelden mit dem neuen Konto öffnet das Mitglieder-Band
const l = await ctx.newPage();
l.on('pageerror', e => errors.push('hören: ' + e.message));
await l.goto(base + '/');
await l.waitForFunction(() => window._crab && _crab.started, null, { timeout: 15000 });
const before = await l.evaluate(() => bandinfo.map(x => x.name));
check('Gast: Mitglieder-Band unsichtbar', !before.includes('70cm-oben'), before);
check('Knopf „Anmelden“ da', await l.isVisible('#acctbtn'));
await l.click('#acctbtn');
await l.fill('.acct-box input[name=username]', 'dk0tst');
await l.fill('.acct-box input[name=password]', userPw || 'x');
await l.click('.acct-box button[type=submit]');
await l.waitForTimeout(900);
check('Hören-Seite: Pflicht zum Ändern', await l.isVisible('text=Neues Passwort festlegen'));
await l.fill('.acct-box input[name=new]', 'Hoerer-Passwort-lang');
await l.fill('.acct-box input[name=new2]', 'Hoerer-Passwort-lang');
await Promise.all([l.waitForNavigation(), l.click('.acct-box button[type=submit]')]);
await l.waitForFunction(() => window._crab && _crab.started, null, { timeout: 15000 });
const after = await l.evaluate(() => bandinfo.map(x => x.name));
check('angemeldet: Mitglieder-Band sichtbar', after.includes('70cm-oben'), after);
check('Knopf zeigt den Namen', /dk0tst/.test(await l.textContent('#acctbtn')));
await l.evaluate(() => { const i = bandinfo.findIndex(x => x.name === '70cm-oben'); setBand(i); });
await l.waitForTimeout(2500);
check('Mitglieder-Band verbindet', await l.evaluate(() => { const i = bandinfo.findIndex(x => x.name === '70cm-oben'); return !!(_crab.bands[i] && _crab.bands[i].started); }));

console.log(errors.length ? 'FEHLER: ' + errors.join(' | ') : 'ALLES IN ORDNUNG');
await b.close();
process.exit(errors.length ? 1 : 0);
