/* Logbuch der Hörer (crabSDR: /logbuch/api/log) und „Gerade online“ (/logbuch/api/online). Kopf, Thema, Name: common.js. */
(function () {
  'use strict';
  function $(id) { return document.getElementById(id); }
  function esc(t) { return String(t == null ? '' : t).replace(/[&<>"]/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]; }); }
  var IPRE = /(?:\d{1,3}(?:\.\d{1,3}){3}|(?:[0-9a-f]{1,4}:){2,7}[0-9a-f]{0,4})/gi;   // IP-Adressen nie anzeigen
  function noIp(s) { return String(s).replace(IPRE, 'Hörer'); }
  function toast(msg) { var t = $('toast'); t.textContent = msg; t.classList.add('show'); clearTimeout(t._h); t._h = setTimeout(function () { t.classList.remove('show'); }, 2200); }
  function readCookie(n) { var m = document.cookie.match('(?:^|; )' + n + '=([^;]*)'); return m ? decodeURIComponent(m[1]) : null; }
  function setCookie(n, v) { document.cookie = n + '=' + encodeURIComponent(v) + '; path=/; max-age=' + (3652 * 86400) + '; SameSite=Lax'; }
  function fmtMHz(kHz) { return (kHz / 1000).toFixed(4).replace('.', ',') + ' MHz'; }

  // Name (gleicher Cookie wie die Hörer-Seite)
  var nameInp = $('myname');
  function myName() { return (nameInp.value.trim().slice(0, 20)) || 'Hörer'; }
  (function () { var c = readCookie('username'); if (c && c !== 'Hörer') nameInp.value = c; })();
  nameInp.onchange = function () { setCookie('username', myName()); };

  function tuneHref(kHz) { return '../?tune=' + kHz + 'fm'; }

  /* ---------- Logbuch ---------- */
  function api(method, path, data, cb) {
    var x = new XMLHttpRequest(); x.open(method, 'api/' + path);
    if (data) x.setRequestHeader('Content-Type', 'application/json');
    x.onload = function () { var r = null; try { r = JSON.parse(x.responseText); } catch (e) {} cb(x.status, r); };
    x.onerror = function () { cb(0, null); };
    x.send(data ? JSON.stringify(data) : null);
    return x;
  }
  function fmtT(t) { var d = new Date(t * 1000); return d.toLocaleDateString('de-DE', { day: '2-digit', month: '2-digit' }) + ' ' + d.toLocaleTimeString('de-DE', { hour: '2-digit', minute: '2-digit' }); }
  function linkFreqs(text) {   // 145,500 / 145.500 / 438725 im Text -> Link auf den Empfänger
    return esc(text).replace(/\b(\d{3})[.,](\d{3})\b|\b(\d{6})\b/g, function (m, a, b, c) {
      var kHz = c ? Number(c) : Number(a) * 1000 + Number(b);
      return (kHz >= 100000 && kHz < 30000000) ? '<a href="' + tuneHref(kHz) + '">' + m + '</a>' : m;
    });
  }
  function dirName(deg) { return ['N', 'NO', 'O', 'SO', 'S', 'SW', 'W', 'NW'][Math.round(deg / 45) % 8]; }
  function loadLogbook() {
    var byKm = $('sortkm').checked;
    api('GET', 'log?n=200' + (byKm ? '&sort=km' : '') + '&_=' + Date.now(), null, function (st, rows) {
      var box = $('logbook');
      if (st !== 200 || !rows) { box.innerHTML = '<span class="muted">Logbuch nicht erreichbar.</span>'; return; }
      if (!rows.length) { box.innerHTML = '<span class="muted">Noch keine Einträge. Trag ein, wen du gehört hast.</span>'; $('logstats').textContent = ''; return; }
      var far = rows.filter(function (r) { return r.km != null; }).sort(function (a, b) { return b.km - a.km; })[0];
      $('logstats').textContent = rows.length + (rows.length === 1 ? ' Eintrag' : ' Einträge') + (far ? ' · weiteste Station ' + far.call + ' aus ' + far.loc + ', ' + far.km + ' km' : '');
      box.innerHTML = '<div class="row head"><span>Zeit</span><span>MHz</span><span>Rufzeichen</span><span>Locator</span><span>Entfernung</span><span>Bemerkung</span><span>gehört von</span></div>' + rows.map(function (r) {
        var km = r.km != null ? r.km + ' km<small>' + dirName(r.deg) + '</small>' : '<span class="muted">–</span>';
        return '<div class="row' + (r.km >= 300 ? ' dx' : '') + '"><span class="t">' + esc(fmtT(r.t)) + '</span><a class="f" href="' + tuneHref(r.freq) + '" title="im Empfänger öffnen">' + esc((r.freq / 1000).toFixed(3)) + '</a><span class="c">' + esc(r.call) + '</span><span class="loc">' + esc(r.loc || '') + '</span><span class="km">' + km + '</span><span>' + esc(r.comment || '') + '</span><span class="by">' + esc(noIp(r.name || '')) + '</span></div>';
      }).join('');
    });
  }
  $('sortkm').onchange = loadLogbook;
  $('logform').onsubmit = function () {
    var f = this, call = f.call.value.trim();
    if (!call) return false;
    setCookie('username', myName());
    api('POST', 'log', { name: myName(), call: call, freq: f.freq.value, loc: f.loc.value.trim(), comment: f.comment.value.trim() }, function (st, r) {
      if (st === 200) { toast(r && r.km != null ? 'Eingetragen: ' + r.km + ' km ' + dirName(r.deg) : 'Eingetragen'); f.call.value = ''; f.loc.value = ''; f.comment.value = ''; loadLogbook(); }
      else toast((r && r.error) || 'Fehler beim Eintragen');
    });
    return false;
  };

  /* ---------- Hörer online ---------- */
  function renderOnline(o) {
    var groups = {}, n = 0;
    (o.users || []).forEach(function (u) {
      var key = String(u.khz || 0); (groups[key] = groups[key] || { kHz: u.khz || 0, names: [] }).names.push(noIp(u.name)); n++;
    });
    $('numusers').textContent = n; $('oncount').textContent = n ? '(' + n + ')' : '';
    var keys = Object.keys(groups).sort(function (a, c) { return groups[a].kHz - groups[c].kHz; });
    $('online').innerHTML = keys.length ? keys.map(function (k) { var g = groups[k]; return '<span><b>' + esc(g.names.join(', ')) + '</b>' + (g.kHz ? '<a href="' + tuneHref(g.kHz) + '">' + esc(fmtMHz(g.kHz)) + '</a>' : '') + '</span>'; }).join('')
      : '<span class="muted">' + (o.t ? 'niemand hört gerade zu' : 'keine Verbindung zum Empfänger') + '</span>';
  }
  function pollOnline() {
    api('GET', 'online?_=' + Date.now(), null, function (st, o) { if (st === 200 && o) renderOnline(o); });
  }

  loadLogbook(); setInterval(loadLogbook, 60000);
  pollOnline(); setInterval(pollOnline, 5000);
})();
