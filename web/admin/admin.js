/* admin.js — Admin-Seite von crabSDR (docs/SECURITY.md).
   Admin-Token nur im sessionStorage dieses Tabs; alle Anfragen mit Authorization-Kopf. Alle Werte werden als Text
   eingesetzt (textContent), nie als HTML. Keine eingebetteten Skripte (CSP). */
(function () {
  'use strict';
  var API = '../api/', KEY = 'crab_admin_token';
  var S = { token: null, user: null, tab: 'overview', cfg: null, over: null };
  try { S.token = sessionStorage.getItem(KEY); S.user = JSON.parse(sessionStorage.getItem(KEY + '_user') || 'null'); } catch (e) {}

  /* ---------- Hilfen ---------- */
  function $(id) { return document.getElementById(id); }
  function h(tag, props) {
    var e = document.createElement(tag);
    if (props) for (var k in props) {
      var v = props[k]; if (v == null || v === false) continue;
      if (k === 'text') e.textContent = v;
      else if (k === 'cls') e.className = v;
      else if (k.slice(0, 2) === 'on') e.addEventListener(k.slice(2), v);
      else if (k === 'value') e.value = v;
      else if (k === 'checked' || k === 'disabled' || k === 'hidden' || k === 'required' || k === 'selected') e[k] = !!v;
      else e.setAttribute(k, v === true ? '' : v);
    }
    for (var i = 2; i < arguments.length; i++) add(e, arguments[i]);
    return e;
  }
  function add(e, c) { if (c == null || c === false) return; if (Array.isArray(c)) c.forEach(function (x) { add(e, x); }); else e.appendChild(typeof c === 'string' || typeof c === 'number' ? document.createTextNode(String(c)) : c); }
  function clear(e) { while (e.firstChild) e.removeChild(e.firstChild); return e; }
  function toast(msg, bad) { var t = $('toast'); t.textContent = msg; t.hidden = false; t.classList.toggle('bad', !!bad); clearTimeout(t._t); t._t = setTimeout(function () { t.hidden = true; }, bad ? 6000 : 2500); }
  function mhz(hz) { return (Number(hz) / 1e6).toFixed(4).replace(/0+$/, '').replace(/\.$/, '').replace('.', ','); }
  function hzFromMhz(s) { var v = parseFloat(String(s).replace(',', '.')); return isFinite(v) ? Math.round(v * 1e6) : null; }
  function num(s) { var v = parseFloat(String(s).replace(',', '.')); return isFinite(v) ? v : null; }
  function ago(sec) { if (sec == null) return '–'; var d = Math.floor(sec / 86400), hh = Math.floor(sec % 86400 / 3600), m = Math.floor(sec % 3600 / 60); return d ? d + ' d ' + hh + ' h' : hh ? hh + ' h ' + m + ' min' : m + ' min'; }
  function when(t) { return t ? new Date(t * 1000).toLocaleString('de-DE', { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' }) : '–'; }

  function api(method, path, body, cb) {
    var x = new XMLHttpRequest(); x.open(method, API + path);
    if (S.token) x.setRequestHeader('Authorization', 'Bearer ' + S.token);
    if (body !== undefined) x.setRequestHeader('Content-Type', 'application/json');
    x.onload = function () {
      var r = null; try { r = JSON.parse(x.responseText); } catch (e) {}
      if (x.status === 401 && path.indexOf('auth/') !== 0) { logout(true); return; }   // Anmelden/Passwort: 401 heißt „falsch“, nicht „abgelaufen“
      if (x.status === 403 && r && r.must_change) { passwordView(null, true); return; }
      cb(x.status, r || {});
    };
    x.onerror = function () { cb(0, { error: 'Server nicht erreichbar' }); };
    x.send(body !== undefined ? JSON.stringify(body) : null);
  }
  function okOr(code, r, msg) { if (code >= 200 && code < 300) { if (msg) toast(msg); return true; } toast(r.error || ('Fehler ' + code), true); return false; }

  function setSession(tok, user) {
    S.token = tok; S.user = user;
    try { if (tok) { sessionStorage.setItem(KEY, tok); sessionStorage.setItem(KEY + '_user', JSON.stringify(user)); } else { sessionStorage.removeItem(KEY); sessionStorage.removeItem(KEY + '_user'); } } catch (e) {}
  }
  function logout(expired) {
    setSession(null, null); $('tabs').hidden = true; $('logout').hidden = true; $('ownpw').hidden = true; $('restartbar').hidden = true; $('whoami').textContent = '';
    loginView(expired ? 'Sitzung abgelaufen – bitte neu anmelden.' : '');
  }

  /* ---------- Hell/Dunkel ---------- */
  (function () {
    var b = $('themebtn');
    function mark() { var c = document.documentElement.getAttribute('data-theme'); b.textContent = !c ? '◐' : c === 'dark' ? '☾' : '☀'; }
    b.addEventListener('click', function () {
      var el = document.documentElement, c = el.getAttribute('data-theme'), n = !c ? 'dark' : c === 'dark' ? 'light' : null;
      if (n) { el.setAttribute('data-theme', n); el.classList.remove('sysdark'); try { localStorage.setItem('crab_theme', n); } catch (e) {} }
      else { el.removeAttribute('data-theme'); el.classList.toggle('sysdark', window.matchMedia('(prefers-color-scheme: dark)').matches); try { localStorage.removeItem('crab_theme'); } catch (e) {} }
      mark();
    });
    mark();
  })();

  /* ---------- Anmelden ---------- */
  function field(label, input, hint) { return h('label', { cls: 'adm-field' }, h('span', { text: label }), input, hint ? h('small', { text: hint }) : null); }
  function loginView(note) {
    var v = clear($('view'));
    var u = h('input', { type: 'text', autocomplete: 'username', required: true, maxlength: '32' });
    var p = h('input', { type: 'password', autocomplete: 'current-password', required: true, maxlength: '72' });
    var msg = h('div', { cls: 'adm-msg' }, note || '');
    var go = h('button', { cls: 'btn btn-accent', type: 'submit', text: 'Anmelden' });
    var f = h('form', { cls: 'adm-card adm-login', onsubmit: function (e) {
      e.preventDefault(); msg.textContent = ''; go.disabled = true;
      api('POST', 'auth/login', { username: u.value.trim(), password: p.value, scope: 'admin' }, function (code, r) {
        go.disabled = false;
        if (code === 200 && r.token) { setSession(r.token, r.user); if (r.user.must_change) passwordView(p.value, true); else start(); }
        else msg.textContent = r.error || 'Anmeldung fehlgeschlagen';
      });
    } }, h('h2', { text: 'Admin-Anmeldung' }), field('Benutzername', u), field('Passwort', p), msg, go,
      h('p', { cls: 'muted small', text: 'Beim ersten Start steht das Passwort des Kontos „admin“ im Protokoll von crabSDR. Vergessen: am Rechner „crabsdr-server --reset-admin“.' }));
    v.appendChild(f); setTimeout(function () { u.focus(); }, 30);
  }
  function passwordView(oldPw, forced) {
    var v = clear($('view')); $('tabs').hidden = true;
    var o = h('input', { type: 'password', autocomplete: 'current-password', required: true, maxlength: '72' });
    var n = h('input', { type: 'password', autocomplete: 'new-password', required: true, minlength: '10', maxlength: '72' });
    var n2 = h('input', { type: 'password', autocomplete: 'new-password', required: true, minlength: '10', maxlength: '72' });
    var msg = h('div', { cls: 'adm-msg' });
    var f = h('form', { cls: 'adm-card adm-login', onsubmit: function (e) {
      e.preventDefault(); msg.textContent = '';
      if (n.value !== n2.value) { msg.textContent = 'Die beiden neuen Passwörter sind verschieden'; return; }
      api('POST', 'auth/password', { old: oldPw || o.value, new: n.value }, function (code, r) {
        if (code === 200 && r.token) { setSession(r.token, r.user); toast('Passwort geändert – alle anderen Sitzungen sind abgemeldet'); start(); }
        else msg.textContent = r.error || 'Nicht gespeichert';
      });
    } }, h('h2', { text: forced ? 'Eigenes Passwort festlegen' : 'Passwort ändern' }),
      forced ? h('p', { cls: 'muted small', text: 'Das Passwort wurde vergeben oder zurückgesetzt. Bitte ein eigenes wählen: mindestens 10 Zeichen, am besten ein Satz.' }) : null,
      oldPw ? null : field('Bisheriges Passwort', o), field('Neues Passwort', n), field('Wiederholen', n2), msg,
      h('button', { cls: 'btn btn-accent', type: 'submit', text: 'Speichern' }),
      forced ? null : h('button', { cls: 'btn', type: 'button', text: 'Abbrechen', onclick: function () { start(); } }));
    v.appendChild(f);
  }

  /* ---------- Rahmen ---------- */
  function start() {
    $('tabs').hidden = false; $('logout').hidden = false; $('ownpw').hidden = false;
    $('whoami').textContent = 'angemeldet als ' + (S.user ? S.user.username : '?');
    show(S.tab);
  }
  document.querySelectorAll('#tabs button').forEach(function (b) { b.addEventListener('click', function () { show(b.dataset.tab); }); });
  $('logout').addEventListener('click', function () { logout(false); });
  $('ownpw').addEventListener('click', function () { passwordView(null, false); });
  $('restartbtn').addEventListener('click', restart);
  function show(tab) {
    S.tab = tab;
    document.querySelectorAll('#tabs button').forEach(function (b) { b.classList.toggle('active', b.dataset.tab === tab); });
    var v = clear($('view')); v.appendChild(h('p', { cls: 'muted', text: 'lädt…' }));
    ({ overview: overview, station: station, bands: bands, decoders: decoders, users: users, chat: chat, config: configText, devices: devices })[tab](v);
  }
  function restartPending(on) { $('restartbar').hidden = !on; }
  function restart() {
    if (!confirm('crabSDR neu starten? Alle Hörer werden kurz getrennt.')) return;
    api('POST', 'admin/restart', {}, function (code, r) {
      if (!okOr(code, r)) return;
      toast('Neustart …'); var t0 = Date.now();
      (function poll() {
        var x = new XMLHttpRequest(); x.open('GET', API + 'health');
        x.onload = function () { var j = {}; try { j = JSON.parse(x.responseText); } catch (e) {} if (x.status === 200 && j.uptime_s < 60 && Date.now() - t0 > 1500) { toast('crabSDR läuft wieder'); restartPending(false); show(S.tab); } else setTimeout(poll, 1000); };
        x.onerror = function () { setTimeout(poll, 1000); };
        x.send();
      })();
    });
  }

  /* Konfiguration laden (für die Formulare) und Änderungen speichern */
  function loadCfg(cb) {
    api('GET', 'admin/config', undefined, function (code, r) {
      if (!okOr(code, r)) return;
      S.cfg = r; cb(r);
    });
  }
  function saveOps(ops, done) {
    if (!ops.length) { toast('Nichts geändert'); return; }
    api('PUT', 'admin/config', { hash: S.cfg.hash, ops: ops }, function (code, r) {
      if (code === 200) { S.cfg.hash = r.hash; restartPending(r.restart_pending); toast('Gespeichert' + (r.warnings && r.warnings.length ? ' (mit Hinweisen)' : '')); if (done) done(); return; }
      if (code === 409) { toast('Die Datei wurde inzwischen geändert – Seite lädt neu', true); show(S.tab); return; }
      if (code === 422) { problems(r); return; }
      toast(r.error || 'Nicht gespeichert', true);
    });
  }
  function problems(r) {
    var box = h('div', { cls: 'adm-card adm-problems' }, h('b', { text: r.error || 'Prüfung fehlgeschlagen' }),
      h('ul', null, (r.errors || []).map(function (e) { return h('li', { cls: 'bad', text: e }); })),
      (r.warnings || []).length ? h('ul', null, r.warnings.map(function (w) { return h('li', { cls: 'warn', text: w }); })) : null);
    var v = $('view'); v.insertBefore(box, v.firstChild); box.scrollIntoView({ behavior: 'smooth' });
  }
  function readonlyNote(v) {
    if (S.cfg && !S.cfg.writable) v.appendChild(h('div', { cls: 'adm-card warn', text: 'Die Konfigurationsdatei ' + S.cfg.path + ' ist für crabSDR nicht beschreibbar. Ändern nur am Rechner – oder Ordner und Datei für die Gruppe crabsdr beschreibbar machen.' }));
  }

  /* ---------- Überblick ---------- */
  function overview(v) {
    api('GET', 'admin/overview', undefined, function (code, r) {
      if (!okOr(code, r)) return;
      S.over = r; clear(v); restartPending(r.restart_pending);
      $('stname').textContent = r.station || 'crabSDR';
      var sys = r.system || {}, mem = sys.memory || {};
      v.appendChild(h('div', { cls: 'adm-grid' },
        stat('Version', r.version), stat('Läuft seit', ago(r.uptime_s)),
        stat('Last', sys.load ? sys.load.map(function (x) { return x.toFixed(2); }).join(' · ') : '–'),
        stat('Speicher frei', mem.total_mb ? mem.available_mb + ' von ' + mem.total_mb + ' MB' : '–'),
        stat('Temperatur', sys.temp_c != null ? sys.temp_c + ' °C' : '–'),
        (sys.disks || []).map(function (d) {
          var e = stat('Platte ' + d.mount, gb(d.used_mb) + ' von ' + gb(d.total_mb) + ' GB (' + d.percent + ' %)');
          if (d.percent >= 85) { e.classList.add('adm-stat-bad'); e.title = 'Platte fast voll: Protokolle und Aufnahmen prüfen'; }
          return e;
        }),
        stat('Hörer', r.bands.reduce(function (a, b) { return a + b.clients; }, 0))));
      v.appendChild(h('div', { cls: 'adm-row' },
        h('button', { cls: 'btn', type: 'button', text: 'Neu starten', disabled: !r.can_restart, title: r.can_restart ? null : 'läuft ohne systemd/Docker', onclick: restart }),
        h('span', { cls: 'muted small', text: 'Konfiguration: ' + (r.config_path || 'keine Datei') })));
      v.appendChild(h('h3', { text: 'Bänder' }));
      v.appendChild(table(['Band', 'Frequenz', 'Zustand', 'Hörer', 'Zugang', 'Verstärkung'], r.bands.map(function (b) {
        return [b.label || b.id, mhz(b.center_freq) + ' MHz', state(b.running, b.running ? 'läuft' : (b.error || 'steht')), b.clients, accessLabel(b.access), b.gain + ' dB'];
      })));
      var lis = []; r.bands.forEach(function (b) { (b.listeners || []).forEach(function (l) { lis.push([l.name || 'Hörer', b.label || b.id, l.freq ? (l.freq / 1000).toFixed(3).replace('.', ',') + ' kHz' : '–', (l.mode || '').toUpperCase()]); }); });
      v.appendChild(h('h3', { text: 'Hörer online (' + lis.length + ')' }));
      v.appendChild(lis.length ? table(['Name', 'Band', 'Frequenz', 'Betriebsart'], lis) : h('p', { cls: 'muted', text: 'Niemand hört gerade zu.' }));
      v.appendChild(h('h3', { text: 'Decoder' }));
      v.appendChild(r.decoders.length ? h('div', null, r.decoders.map(function (d) {
        return h('details', { cls: 'adm-dec' }, h('summary', null, h('b', { text: d.label || d.id }), ' ', mhz(d.freq) + ' MHz · ', state(d.state === 'läuft', d.state),
          ' · ' + d.events + ' Treffer · ' + d.restarts + ' Neustarts' + (d.public ? '' : ' · nicht öffentlich')),
          (d.stderr || []).length ? h('pre', { cls: 'adm-pre', text: d.stderr.join('\n') }) : h('p', { cls: 'muted small', text: 'keine Meldungen' }));
      })) : h('p', { cls: 'muted', text: 'Keine Decoder eingerichtet.' }));
    });
  }
  function gb(mb) { return (mb / 1024).toFixed(mb < 10240 ? 1 : 0).replace('.', ','); }
  function stat(k, v) { return h('div', { cls: 'adm-stat' }, h('small', { text: k }), h('b', { text: String(v) })); }
  function state(ok, text) { return h('span', { cls: ok ? 'ok' : 'warn', text: text }); }
  function accessLabel(a) { return a === 'public' ? 'öffentlich' : a === 'members' ? 'Mitglieder' : 'nur Admin'; }
  function table(head, rows) {
    return h('div', { cls: 'adm-tablewrap' }, h('table', { cls: 'adm-table' }, h('thead', null, h('tr', null, head.map(function (x) { return h('th', { text: x }); }))),
      h('tbody', null, rows.map(function (r) { return h('tr', null, r.map(function (c) { return h('td', null, c instanceof Node ? c : String(c == null ? '' : c)); })); }))));
  }

  /* ---------- Station und Oberfläche ---------- */
  var UI_SWITCHES = [['login', 'Knopf „Anmelden“'], ['chat', 'Chat'], ['logbook', 'Logbuch'], ['digital', 'Seite Digital'], ['info', 'Seite Info'],
    ['decoders', 'Decoder-Menü'], ['recording', 'Aufnahme'], ['status', 'Statuszeile']];
  function station(v) {
    loadCfg(function (c) {
      clear(v); readonlyNote(v);
      var p = c.parsed || {}, st = p.station || {}, ui = p.ui || {}, dir = p.directory || {};
      var inp = {};
      function t(k, label, val, hint) { inp[k] = h('input', { type: 'text', value: val == null ? '' : String(val), maxlength: '200' }); return field(label, inp[k], hint); }
      var sw = {}, listed = h('input', { type: 'checkbox', checked: !!dir.enabled });
      v.appendChild(h('form', { cls: 'adm-card', onsubmit: function (e) {
        e.preventDefault();
        var ops = [];
        [['name', st.name], ['subtitle', st.subtitle], ['locator', st.locator], ['url', st.url]].forEach(function (x) {
          var nv = inp[x[0]].value.trim(); if (nv !== (x[1] || '')) ops.push({ op: 'set', path: ['station', x[0]], value: nv || null });
        });
        for (var li = 0, LL = ['lat', 'lon']; li < LL.length; li++) {
          var k = LL[li], nv = inp[k].value.trim(), n = nv === '' ? null : num(nv);
          if (nv !== '' && n === null) return toast((k === 'lat' ? 'Breite' : 'Länge') + ': Zahl in Dezimalgrad erwartet', true);
          if (n !== (st[k] == null ? null : st[k])) ops.push({ op: 'set', path: ['station', k], value: n });
        }
        UI_SWITCHES.forEach(function (x) { var val = sw[x[0]].value, cur = ui[x[0]] == null ? 'auto' : ui[x[0]] ? 'on' : 'off'; if (val !== cur) ops.push({ op: 'set', path: ['ui', x[0]], value: val === 'auto' ? null : val === 'on' }); });
        ['banner', 'impressum', 'datenschutz'].forEach(function (k) { var nv = inp[k].value.trim(); if (nv !== (ui[k] || '')) ops.push({ op: 'set', path: ['ui', k], value: nv || null }); });
        if (listed.checked && !inp.url.value.trim()) return toast('Für das Verzeichnis bitte die öffentliche Adresse eintragen (https://…)', true);
        if (listed.checked !== !!dir.enabled) ops.push({ op: 'set', path: ['directory', 'enabled'], value: listed.checked });
        saveOps(ops, function () { show('station'); });
      } },
        h('h3', { text: 'Station' }),
        t('name', 'Name', st.name), t('subtitle', 'Untertitel', st.subtitle, 'z. B. Ort · Antenne · Höhe'),
        h('div', { cls: 'adm-cols' }, t('locator', 'Locator', st.locator), t('lat', 'Breite (Dezimalgrad)', st.lat, 'leer = aus dem Locator'), t('lon', 'Länge (Dezimalgrad)', st.lon)),
        t('url', 'Öffentliche Adresse', st.url, 'so erreichen Hörer die Station, z. B. https://sdr.example.org'),
        h('label', { cls: 'adm-check' }, listed, ' Im Verzeichnis auf ' + (dir.server || 'https://crabsdr.de').replace(/^https?:\/\//, '') + ' listen'),
        h('p', { cls: 'muted small', text: 'Die Station meldet alle 5 Minuten Name, Untertitel, Standort, die öffentlichen Bänder und Decoder und die Hörerzahl. Mitglieder- und Admin-Bänder bleiben unsichtbar. Das Verzeichnis prüft die Angaben über die öffentliche Adresse. Wirkt nach dem Neustart.' }),
        h('h3', { text: 'Oberfläche' }),
        h('div', { cls: 'adm-switches' }, UI_SWITCHES.map(function (x) {
          var cur = ui[x[0]] == null ? 'auto' : ui[x[0]] ? 'on' : 'off';
          sw[x[0]] = h('select', null, h('option', { value: 'auto', text: 'automatisch', selected: cur === 'auto' }), h('option', { value: 'on', text: 'an', selected: cur === 'on' }), h('option', { value: 'off', text: 'aus', selected: cur === 'off' }));
          return field(x[1], sw[x[0]]);
        })),
        t('banner', 'Hinweis oben auf der Seite', ui.banner, 'z. B. „Wartung heute ab 20 Uhr“ – leer = keiner'),
        h('div', { cls: 'adm-cols' }, t('impressum', 'Link Impressum', ui.impressum), t('datenschutz', 'Link Datenschutz', ui.datenschutz)),
        h('button', { cls: 'btn btn-accent', type: 'submit', text: 'Speichern', disabled: !c.writable })));
    });
  }

  /* ---------- Bänder ---------- */
  var MODES = [['', '–'], ['fm', 'FM'], ['am', 'AM'], ['usb', 'USB'], ['lsb', 'LSB'], ['cw', 'CW']], DRIVERS = ['rtl_sdr', 'rtl_tcp', 'hackrf', 'rx_sdr'];
  function sel(opts, cur) { return h('select', null, opts.map(function (o) { var val = Array.isArray(o) ? o[0] : o, lab = Array.isArray(o) ? o[1] : (o || '–'); return h('option', { value: val, text: lab, selected: String(cur == null ? '' : cur) === String(val) }); })); }
  function bands(v) {
    loadCfg(function (c) {
      clear(v); readonlyNote(v);
      var list = (c.parsed && c.parsed.bands) || [];
      if (c.parsed && !c.parsed.band_file_order) v.appendChild(h('div', { cls: 'adm-card warn', text: 'Die Datei hat das alte Format ohne [[bands]] – Bänder bitte in der Textansicht bearbeiten.' }));
      list.forEach(function (b, i) { v.appendChild(bandCard(b, i, c.writable)); });
      v.appendChild(newBandCard(c.writable));
    });
  }
  function bandCard(b, i, writable) {
    var f = {};
    function t(k, label, val, hint, type) { f[k] = h('input', { type: type || 'text', value: val == null ? '' : String(val) }); return field(label, f[k], hint); }
    var access = b.admin_only ? 'admin' : b.guest ? 'public' : 'members';
    f.access = sel([['public', 'öffentlich (ohne Anmeldung)'], ['members', 'Mitglieder (angemeldet, zugeteilt)'], ['admin', 'nur Admin']], access);
    f.mode = sel(MODES, b.mode || b.default_mode);
    f.driver = sel(DRIVERS, b.driver);
    f.enabled = h('input', { type: 'checkbox', checked: b.enabled !== false });
    var live = h('button', { cls: 'btn', type: 'button', text: 'sofort ausprobieren', title: 'Verstärkung sofort setzen, ohne zu speichern', onclick: function () {
      var g = num(f.gain.value); if (g == null) return toast('Verstärkung: Zahl', true);
      api('POST', 'admin/bands/' + encodeURIComponent(b.id) + '/gain', { gain: g }, function (code, r) { okOr(code, r, 'Verstärkung ' + g + ' dB gesetzt (noch nicht gespeichert)'); });
    } });
    return h('form', { cls: 'adm-card', onsubmit: function (e) {
      e.preventDefault();
      var ops = [], P = function (k) { return ['bands', i, k]; };
      function cmp(k, nv, cur) { if (String(nv == null ? '' : nv) !== String(cur == null ? '' : cur)) ops.push({ op: 'set', path: P(k), value: nv === '' ? null : nv }); }
      cmp('label', f.label.value.trim(), b.label); cmp('note', f.note.value.trim(), b.note);
      var cf = hzFromMhz(f.center.value); if (cf == null) return toast('Mittenfrequenz: Zahl in MHz', true); cmp('center_freq', cf, b.center_freq);
      var g = num(f.gain.value); if (g == null) return toast('Verstärkung: Zahl', true); cmp('gain', g, b.gain);
      cmp('mode', f.mode.value, b.mode || b.default_mode);
      var sc = f.smeter.value.trim() === '' ? '' : num(f.smeter.value); if (sc === null) return toast('S-Meter: Zahl', true); cmp('smeter_cal', sc, b.smeter_cal);
      var a = f.access.value; if (a !== access) { ops.push({ op: 'set', path: P('guest'), value: a === 'public' }); ops.push({ op: 'set', path: P('admin_only'), value: a === 'admin' ? true : null }); }
      if (f.enabled.checked !== (b.enabled !== false)) ops.push({ op: 'set', path: P('enabled'), value: f.enabled.checked ? null : false });
      cmp('driver', f.driver.value, b.driver); cmp('device', f.device.value.trim(), b.device); cmp('host', f.host.value.trim(), b.host);
      var port = f.port.value.trim() === '' ? '' : parseInt(f.port.value, 10); if (port !== '' && !(port > 0 && port < 65536)) return toast('Port 1–65535', true); cmp('port', port, b.port);
      saveOps(ops, function () { show('bands'); });
    } },
      h('div', { cls: 'adm-cardhead' }, h('h3', { text: (b.label || b.id) + ' ' }, h('small', { cls: 'muted', text: b.id })),
        h('button', { cls: 'btn btn-ghost', type: 'button', text: 'Band entfernen', disabled: !writable, onclick: function () {
          if (confirm('Band „' + (b.label || b.id) + '“ aus der Konfiguration entfernen?')) saveOps([{ op: 'remove', path: ['bands', i] }], function () { show('bands'); });
        } })),
      h('div', { cls: 'adm-cols' }, t('label', 'Bezeichnung', b.label), t('note', 'Kurzbeschreibung', b.note)),
      h('div', { cls: 'adm-cols' }, t('center', 'Mittenfrequenz (MHz)', mhz(b.center_freq)), field('Betriebsart beim Öffnen', f.mode), field('Zugang', f.access)),
      h('div', { cls: 'adm-cols' }, h('div', { cls: 'adm-field' }, h('span', { text: 'Verstärkung (dB)' }), h('div', { cls: 'adm-inline' }, (f.gain = h('input', { type: 'text', value: String(b.gain) })), live)),
        t('smeter', 'S-Meter-Korrektur (dB)', b.smeter_cal, 'leer = keine')),
      h('details', null, h('summary', { text: 'Quelle' }),
        h('div', { cls: 'adm-cols' }, field('Treiber', f.driver), t('device', 'Gerät (Index oder Seriennummer)', b.device), t('host', 'rtl_tcp: Adresse', b.host), t('port', 'rtl_tcp: Port', b.port))),
      h('label', { cls: 'adm-check' }, f.enabled, ' Band eingeschaltet'),
      h('button', { cls: 'btn btn-accent', type: 'submit', text: 'Speichern', disabled: !writable }));
  }
  function newBandCard(writable) {
    var id = h('input', { type: 'text', placeholder: 'z. B. 70cm', maxlength: '32' }), label = h('input', { type: 'text' }), cf = h('input', { type: 'text', placeholder: '438,5' });
    var drv = sel(DRIVERS, 'rtl_sdr'), dev = h('input', { type: 'text', value: '0' }), gain = h('input', { type: 'text', value: '30' });
    var host = h('input', { type: 'text', placeholder: '127.0.0.1' }), port = h('input', { type: 'text', placeholder: '1234' });
    var acc = sel([['public', 'öffentlich'], ['members', 'Mitglieder'], ['admin', 'nur Admin']], 'public');
    return h('details', { cls: 'adm-card' }, h('summary', { text: 'Band hinzufügen' }), h('form', { onsubmit: function (e) {
      e.preventDefault();
      if (!/^[A-Za-z0-9._-]{1,32}$/.test(id.value.trim())) return toast('Kennung: Buchstaben, Ziffern, . - _', true);
      var hz = hzFromMhz(cf.value); if (hz == null) return toast('Mittenfrequenz in MHz', true);
      var t = { id: id.value.trim(), label: label.value.trim() || id.value.trim(), driver: drv.value, center_freq: hz, gain: num(gain.value) || 0, guest: acc.value === 'public' };
      if (acc.value === 'admin') t.admin_only = true;
      if (drv.value === 'rtl_tcp') { if (host.value.trim()) t.host = host.value.trim(); if (port.value.trim()) t.port = parseInt(port.value, 10); } else t.device = dev.value.trim() || '0';
      saveOps([{ op: 'append', array: 'bands', table: t }], function () { show('bands'); });
    } },
      h('div', { cls: 'adm-cols' }, field('Kennung', id, 'kurz, ohne Leerzeichen'), field('Bezeichnung', label), field('Mittenfrequenz (MHz)', cf)),
      h('div', { cls: 'adm-cols' }, field('Treiber', drv), field('Gerät', dev, 'Index oder Seriennummer (siehe „Sticks“)'), field('Verstärkung (dB)', gain), field('Zugang', acc)),
      h('div', { cls: 'adm-cols' }, field('rtl_tcp: Adresse', host), field('rtl_tcp: Port', port)),
      h('button', { cls: 'btn btn-accent', type: 'submit', text: 'Hinzufügen', disabled: !writable })));
  }

  /* ---------- Decoder ---------- */
  function decoders(v) {
    loadCfg(function (c) {
      clear(v); readonlyNote(v);
      var list = (c.parsed && c.parsed.decoders) || [], ids = (c.parsed && c.parsed.decoder_ids) || [];
      if (!list.length) v.appendChild(h('p', { cls: 'muted', text: 'Noch keine Decoder.' }));
      list.forEach(function (d, i) { v.appendChild(decoderCard(d, i, ids[i], c.writable)); });
      v.appendChild(newDecoderCard(c));
    });
  }
  function optionRows(opts) {
    var box = h('div', { cls: 'adm-opts' });
    function row(k, val) {
      var r = h('div', { cls: 'adm-inline' }, h('input', { type: 'text', value: k || '', placeholder: 'name', cls: 'k' }), h('input', { type: 'text', value: val || '', placeholder: 'Wert', cls: 'v' }),
        h('button', { cls: 'btn btn-ghost btn-icon', type: 'button', text: '✕', title: 'Option entfernen', onclick: function () { r.remove(); } }));
      box.appendChild(r);
    }
    Object.keys(opts || {}).sort().forEach(function (k) { row(k, opts[k]); });
    var more = h('button', { cls: 'btn btn-ghost', type: 'button', text: '+ Option', onclick: function () { row('', ''); box.appendChild(more); } });
    box.appendChild(more);
    box.read = function () { var o = {}; box.querySelectorAll('.adm-inline').forEach(function (r) { var k = r.querySelector('.k').value.trim(), val = r.querySelector('.v').value; if (k) o[k] = val; }); return o; };
    return box;
  }
  function decoderCard(d, i, id, writable) {
    var label = h('input', { type: 'text', value: d.label || '' }), freq = h('input', { type: 'text', value: mhz(d.freq) });
    var en = h('input', { type: 'checkbox', checked: d.enabled !== false }), pub = h('input', { type: 'checkbox', checked: d.public !== false });
    var opts = optionRows(d.options);
    return h('form', { cls: 'adm-card', onsubmit: function (e) {
      e.preventDefault();
      var ops = [], P = function () { return ['decoders', i].concat([].slice.call(arguments)); };
      if (label.value.trim() !== (d.label || '')) ops.push({ op: 'set', path: P('label'), value: label.value.trim() || null });
      var hz = hzFromMhz(freq.value); if (hz == null) return toast('Frequenz in MHz', true); if (hz !== d.freq) ops.push({ op: 'set', path: P('freq'), value: hz });
      if (en.checked !== (d.enabled !== false)) ops.push({ op: 'set', path: P('enabled'), value: en.checked ? null : false });
      if (pub.checked !== (d.public !== false)) ops.push({ op: 'set', path: P('public'), value: pub.checked ? null : false });
      var o = opts.read(), old = d.options || {};
      for (var k in o) { if (!/^[a-z0-9_]{1,40}$/.test(k)) return toast('Optionsname „' + k + '“: nur a–z, 0–9, _', true); if (o[k] !== old[k]) ops.push({ op: 'set', path: P('options', k), value: o[k] }); }
      for (var k2 in old) if (!(k2 in o)) ops.push({ op: 'remove', path: P('options', k2) });
      saveOps(ops, function () { show('decoders'); });
    } },
      h('div', { cls: 'adm-cardhead' }, h('h3', { text: (d.label || d.plugin) + ' ' }, h('small', { cls: 'muted', text: id + ' · Plugin ' + d.plugin })),
        h('button', { cls: 'btn btn-ghost', type: 'button', text: 'Decoder entfernen', disabled: !writable, onclick: function () {
          if (confirm('Decoder „' + (d.label || id) + '“ entfernen? Seine bisherigen Ergebnisse bleiben im Datenordner.')) saveOps([{ op: 'remove', path: ['decoders', i] }], function () { show('decoders'); });
        } })),
      h('div', { cls: 'adm-cols' }, field('Bezeichnung', label), field('Frequenz (MHz)', freq)),
      h('label', { cls: 'adm-check' }, en, ' eingeschaltet'),
      h('label', { cls: 'adm-check' }, pub, ' öffentlich (aus: nur Admins und zugeteilte Benutzer sehen die Ergebnisse)'),
      h('div', { cls: 'adm-field' }, h('span', { text: 'Optionen des Plugins' }), opts, h('small', { text: 'siehe docs/CONFIG.md, z. B. mycall, igate, depth' })),
      h('button', { cls: 'btn btn-accent', type: 'submit', text: 'Speichern', disabled: !writable }));
  }
  function newDecoderCard(c) {
    var plugins = c.plugins || [];
    var pl = sel(plugins.map(function (p) { return [p.name, (p.label || p.name) + (p.description ? ' – ' + p.description : '')]; }), plugins[0] && plugins[0].name);
    var freq = h('input', { type: 'text', placeholder: '144,800' }), label = h('input', { type: 'text' }), pub = h('input', { type: 'checkbox', checked: true });
    return h('details', { cls: 'adm-card' }, h('summary', { text: 'Decoder hinzufügen' }), plugins.length ? h('form', { onsubmit: function (e) {
      e.preventDefault();
      var hz = hzFromMhz(freq.value); if (hz == null) return toast('Frequenz in MHz', true);
      var t = { plugin: pl.value, freq: hz }; if (label.value.trim()) t.label = label.value.trim(); if (!pub.checked) t.public = false;
      saveOps([{ op: 'append', array: 'decoders', table: t }], function () { show('decoders'); });
    } }, h('div', { cls: 'adm-cols' }, field('Plugin', pl), field('Frequenz (MHz)', freq), field('Bezeichnung', label)),
      h('label', { cls: 'adm-check' }, pub, ' öffentlich'), h('button', { cls: 'btn btn-accent', type: 'submit', text: 'Hinzufügen', disabled: !c.writable }))
      : h('p', { cls: 'muted', text: 'Keine Plugins installiert.' }));
  }

  /* ---------- Benutzer ---------- */
  function pickList(all, cur, allLabel) {
    // Häkchen je Band/Decoder, dazu „alle“
    var box = h('div', { cls: 'adm-picks' }), star = h('input', { type: 'checkbox', checked: (cur || []).indexOf('*') >= 0 });
    var items = all.map(function (a) { var cb = h('input', { type: 'checkbox', value: a[0], checked: (cur || []).indexOf(a[0]) >= 0 }); box.appendChild(h('label', { cls: 'adm-check' }, cb, ' ' + a[1])); return cb; });
    box.insertBefore(h('label', { cls: 'adm-check' }, star, ' ' + allLabel), box.firstChild);
    function sync() { items.forEach(function (c) { c.disabled = star.checked; }); }
    star.addEventListener('change', sync); sync();
    box.read = function () { return star.checked ? ['*'] : items.filter(function (c) { return c.checked; }).map(function (c) { return c.value; }); };
    return box;
  }
  function users(v) {
    loadCfg(function (c) {
      var bandsAll = ((c.parsed && c.parsed.bands) || []).filter(function (b) { return !b.guest || b.admin_only; }).map(function (b) { return [b.id, (b.label || b.id) + (b.admin_only ? ' (nur Admin)' : '')]; });
      var decs = (c.parsed && c.parsed.decoders) || [], ids = (c.parsed && c.parsed.decoder_ids) || [];
      var decAll = decs.map(function (d, i) { return [ids[i], (d.label || ids[i]) + (d.public === false ? '' : ' (öffentlich)')]; }).filter(function (x, i) { return decs[i].public === false; });
      api('GET', 'admin/users', undefined, function (code, r) {
        if (!okOr(code, r)) return;
        clear(v);
        v.appendChild(h('p', { cls: 'muted small', text: 'Admins dürfen alles. Nutzer hören alle öffentlichen Bänder und zusätzlich die hier zugeteilten Mitglieder-Bänder; nicht öffentliche Decoder sehen sie nur, wenn zugeteilt.' }));
        r.users.forEach(function (u) { v.appendChild(userCard(u, bandsAll, decAll)); });
        v.appendChild(newUserCard(bandsAll, decAll));
      });
    });
  }
  function userCard(u, bandsAll, decAll) {
    var me = S.user && u.username === S.user.username;
    var role = sel([['user', 'Nutzer'], ['admin', 'Admin']], u.role), active = h('input', { type: 'checkbox', checked: u.active, disabled: me });
    var b = pickList(bandsAll, u.bands, 'alle Mitglieder-Bänder'), d = pickList(decAll, u.decoders, 'alle nicht öffentlichen Decoder');
    var out = h('div', { cls: 'adm-msg ok' });
    role.disabled = me;
    return h('form', { cls: 'adm-card', onsubmit: function (e) {
      e.preventDefault();
      api('PUT', 'admin/users/' + encodeURIComponent(u.id), { role: role.value, active: active.checked, bands: b.read(), decoders: d.read() }, function (code, r) { if (okOr(code, r, 'Gespeichert')) show('users'); });
    } },
      h('div', { cls: 'adm-cardhead' }, h('h3', { text: u.username + ' ' }, h('small', { cls: 'muted', text: (u.role === 'admin' ? 'Admin' : 'Nutzer') + (u.active ? '' : ' · gesperrt') + (u.must_change_pw ? ' · muss Passwort ändern' : '') + ' · zuletzt ' + (u.last_login || 'nie') })),
        h('span', null,
          h('button', { cls: 'btn btn-ghost', type: 'button', text: 'Passwort zurücksetzen', onclick: function () {
            if (!confirm('Neues Zufallspasswort für „' + u.username + '“? Alle Sitzungen des Kontos enden.')) return;
            api('POST', 'admin/users/' + encodeURIComponent(u.id) + '/password', {}, function (code, r) { if (okOr(code, r)) { out.textContent = 'Neues Passwort (nur jetzt sichtbar): ' + r.password + ' – beim Anmelden muss es geändert werden.'; } });
          } }),
          me ? null : h('button', { cls: 'btn btn-ghost', type: 'button', text: 'Löschen', onclick: function () {
            if (confirm('Konto „' + u.username + '“ löschen?')) api('DELETE', 'admin/users/' + encodeURIComponent(u.id), undefined, function (code, r) { if (okOr(code, r, 'Gelöscht')) show('users'); });
          } }))),
      out,
      h('div', { cls: 'adm-cols' }, field('Rolle', role, me ? 'das eigene Konto nicht' : null), h('label', { cls: 'adm-check' }, active, ' aktiv')),
      bandsAll.length ? h('div', { cls: 'adm-field' }, h('span', { text: 'Mitglieder-Bänder' }), b) : null,
      decAll.length ? h('div', { cls: 'adm-field' }, h('span', { text: 'Nicht öffentliche Decoder' }), d) : null,
      h('button', { cls: 'btn btn-accent', type: 'submit', text: 'Speichern' }));
  }
  function newUserCard(bandsAll, decAll) {
    var name = h('input', { type: 'text', maxlength: '32', placeholder: 'z. B. DL1ABC' }), role = sel([['user', 'Nutzer'], ['admin', 'Admin']], 'user');
    var pw = h('input', { type: 'text', autocomplete: 'off', placeholder: 'leer = Zufallspasswort' });
    var b = pickList(bandsAll, [], 'alle Mitglieder-Bänder'), d = pickList(decAll, [], 'alle nicht öffentlichen Decoder');
    var out = h('div', { cls: 'adm-msg ok' });
    return h('details', { cls: 'adm-card', open: false }, h('summary', { text: 'Benutzer anlegen' }), h('form', { onsubmit: function (e) {
      e.preventDefault();
      api('POST', 'admin/users', { username: name.value.trim(), role: role.value, password: pw.value, bands: b.read(), decoders: d.read() }, function (code, r) {
        if (!okOr(code, r, 'Angelegt')) return;
        out.textContent = r.password ? 'Passwort für „' + name.value.trim() + '“ (nur jetzt sichtbar): ' + r.password + ' – beim ersten Anmelden muss es geändert werden.' : 'Angelegt. Beim ersten Anmelden muss das Passwort geändert werden.';
        name.value = ''; pw.value = '';
      });
    } }, h('div', { cls: 'adm-cols' }, field('Benutzername', name, '3–32 Zeichen: Buchstaben, Ziffern, . - _'), field('Rolle', role), field('Passwort', pw, 'mindestens 10 Zeichen')),
      bandsAll.length ? h('div', { cls: 'adm-field' }, h('span', { text: 'Mitglieder-Bänder' }), b) : h('p', { cls: 'muted small', text: 'Alle Bänder sind öffentlich – für Mitglieder-Bänder im Reiter „Bänder“ den Zugang ändern.' }),
      decAll.length ? h('div', { cls: 'adm-field' }, h('span', { text: 'Nicht öffentliche Decoder' }), d) : null,
      out, h('button', { cls: 'btn btn-accent', type: 'submit', text: 'Anlegen' }), h('button', { cls: 'btn', type: 'button', text: 'Liste neu laden', onclick: function () { show('users'); } })));
  }

  /* ---------- Chat und Logbuch ---------- */
  function chat(v) {
    api('GET', 'admin/chat?n=300', undefined, function (code, r) {
      if (!okOr(code, r)) return;
      clear(v);
      function del(kind, id, row) { return h('button', { cls: 'btn btn-ghost', type: 'button', text: 'löschen', onclick: function () { api('DELETE', 'admin/' + kind + '/' + id, undefined, function (c2, r2) { if (okOr(c2, r2, 'Gelöscht')) row.remove(); }); } }); }
      v.appendChild(h('h3', { text: 'Chat (' + r.chat.length + ')' }));
      var t1 = h('tbody');
      r.chat.forEach(function (m) { var tr = h('tr'); [when(m.t), m.name, m.msg].forEach(function (x) { tr.appendChild(h('td', { text: x })); }); tr.appendChild(h('td', null, del('chat', m.id, tr))); t1.appendChild(tr); });
      v.appendChild(r.chat.length ? h('div', { cls: 'adm-tablewrap' }, h('table', { cls: 'adm-table' }, h('thead', null, h('tr', null, ['Zeit', 'Name', 'Nachricht', ''].map(function (x) { return h('th', { text: x }); }))), t1)) : h('p', { cls: 'muted', text: 'leer' }));
      v.appendChild(h('h3', { text: 'Logbuch (' + r.log.length + ')' }));
      var t2 = h('tbody');
      r.log.forEach(function (m) { var tr = h('tr'); [when(m.t), m.name || '', m.call || '', m.freq ? (m.freq / 1000).toFixed(3).replace('.', ',') : '', m.loc || '', m.comment || ''].forEach(function (x) { tr.appendChild(h('td', { text: x })); }); tr.appendChild(h('td', null, del('log', m.id, tr))); t2.appendChild(tr); });
      v.appendChild(r.log.length ? h('div', { cls: 'adm-tablewrap' }, h('table', { cls: 'adm-table' }, h('thead', null, h('tr', null, ['Zeit', 'von', 'Rufzeichen', 'MHz', 'Locator', 'Bemerkung', ''].map(function (x) { return h('th', { text: x }); }))), t2)) : h('p', { cls: 'muted', text: 'leer' }));
    });
  }

  /* ---------- Konfiguration als Text ---------- */
  function configText(v) {
    loadCfg(function (c) {
      clear(v); readonlyNote(v);
      var ta = h('textarea', { cls: 'adm-editor', spellcheck: 'false', autocomplete: 'off', wrap: 'off' }); ta.value = c.text;
      var res = h('div', { cls: 'adm-result' });
      function showRes(r, okText) {
        clear(res);
        if (!(r.errors || []).length && !(r.warnings || []).length) { res.appendChild(h('p', { cls: 'ok', text: okText })); return; }
        if ((r.errors || []).length) res.appendChild(h('ul', null, r.errors.map(function (e) { return h('li', { cls: 'bad', text: e }); })));
        if ((r.warnings || []).length) res.appendChild(h('ul', null, r.warnings.map(function (e) { return h('li', { cls: 'warn', text: e }); })));
      }
      v.appendChild(h('div', { cls: 'adm-card' },
        h('div', { cls: 'adm-cardhead' }, h('h3', { text: c.path }), h('small', { cls: 'muted', text: 'Nicht änderbar über die Admin-Seite: ' + c.locked.join(', ') })),
        ta,
        h('div', { cls: 'adm-row' },
          h('button', { cls: 'btn', type: 'button', text: 'Prüfen', onclick: function () { api('POST', 'admin/config/check', { text: ta.value }, function (code, r) { if (okOr(code, r)) showRes(r, 'In Ordnung – kann gespeichert werden.'); }); } }),
          h('button', { cls: 'btn btn-accent', type: 'button', text: 'Speichern', disabled: !c.writable, onclick: function () {
            api('PUT', 'admin/config', { hash: S.cfg.hash, text: ta.value }, function (code, r) {
              if (code === 200) { S.cfg.hash = r.hash; restartPending(r.restart_pending); showRes({ warnings: r.warnings }, 'Gespeichert. Vorige Fassung liegt in den Sicherungen.'); toast('Gespeichert'); return; }
              if (code === 409) { toast('Die Datei wurde inzwischen geändert – bitte neu laden (Ihre Änderung ist nicht gespeichert)', true); return; }
              if (code === 422) { showRes(r, ''); toast('Prüfung fehlgeschlagen – nichts gespeichert', true); return; }
              toast(r.error || 'Nicht gespeichert', true);
            });
          } }),
          h('button', { cls: 'btn btn-ghost', type: 'button', text: 'Neu laden', onclick: function () { show('config'); } })),
        res));
      if (c.backups.length) {
        var pre = h('pre', { cls: 'adm-pre', hidden: true });
        v.appendChild(h('div', { cls: 'adm-card' }, h('h3', { text: 'Sicherungen (vor jeder Änderung, die letzten 20)' }),
          h('ul', { cls: 'adm-list' }, c.backups.map(function (b) {
            return h('li', null, h('button', { cls: 'btn btn-ghost', type: 'button', text: b.name, onclick: function () {
              api('GET', 'admin/config/backups/' + encodeURIComponent(b.name), undefined, function (code, r) { if (!okOr(code, r)) return; pre.textContent = r.text; pre.hidden = false; pre.dataset.name = b.name; });
            } }));
          })), pre,
          h('button', { cls: 'btn', type: 'button', text: 'Angezeigte Sicherung in den Editor übernehmen', onclick: function () {
            if (pre.hidden) return toast('Zuerst eine Sicherung anklicken', true);
            ta.value = pre.textContent; ta.scrollIntoView({ behavior: 'smooth' }); toast('Übernommen – mit „Speichern“ zurücksetzen');
          } })));
      }
    });
  }

  /* ---------- Sticks ---------- */
  function devices(v) {
    api('GET', 'admin/devices', undefined, function (code, r) {
      if (!okOr(code, r)) return;
      clear(v);
      v.appendChild(h('p', { cls: 'muted small', text: 'Angeschlossene Empfänger. Mit einer eigenen Seriennummer (z. B. „2m“) findet crabSDR jeden Stick zuverlässig wieder, egal an welchem USB-Anschluss. Ein Stick, der gerade als Band läuft, kann nicht umprogrammiert werden.' }));
      if (!r.devices.length) { v.appendChild(h('p', { cls: 'muted', text: 'Keine Empfänger gefunden.' })); return; }
      v.appendChild(table(['Gerät', 'Seriennummer', 'als Band', 'Hinweis', ''], r.devices.map(function (d) {
        var act = null;
        if (d.driver === 'rtlsdr' && d.index != null) {
          var inp = h('input', { type: 'text', value: d.serial || '', maxlength: '16', size: '10' });
          act = h('span', { cls: 'adm-inline' }, inp, h('button', { cls: 'btn', type: 'button', text: 'Seriennummer setzen', onclick: function () {
            if (!confirm('Seriennummer „' + inp.value + '“ in Stick ' + d.index + ' schreiben? Danach Stick ab- und wieder anstecken.')) return;
            api('POST', 'admin/devices/serial', { index: d.index, serial: inp.value.trim() }, function (c2, r2) { okOr(c2, r2, r2.message); });
          } }));
        }
        return [d.label || d.product || d.driver, d.serial || '–', d.band || '–', d.duplicate_serial ? 'Seriennummer doppelt!' : (d.available ? '' : 'belegt'), act || ''];
      })));
    });
  }

  /* ---------- Start ---------- */
  if (S.token) api('GET', 'auth/me', undefined, function (code, r) {
    if (code === 200 && r.user && r.user.scope === 'admin' && r.user.role === 'admin') { S.user = r.user; if (r.user.must_change) passwordView(null, true); else start(); }
    else logout(false);
  });
  else loginView('');
})();
