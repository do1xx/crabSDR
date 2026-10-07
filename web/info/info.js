/* Info-Seite: baut Standort, Bänder und Decoder aus ui.json, bandinfo.js und api/decoders. */
crabReady(function (UI) {
  'use strict';
  function $(id) { return document.getElementById(id); }
  function esc(t) { return String(t == null ? '' : t).replace(/[&<>"]/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]; }); }
  function mhz(kHz, d) { return (kHz / 1000).toFixed(d == null ? 3 : d).replace('.', ','); }
  function dms(v, pos, neg) { var a = Math.abs(v), d = Math.floor(a), m = Math.floor((a - d) * 60), s = Math.round(((a - d) * 60 - m) * 60); return d + '° ' + String(m).padStart(2, '0') + '′ ' + String(s).padStart(2, '0') + '″ ' + (v >= 0 ? pos : neg); }
  var st = UI.station || {}, rows = [];
  rows.push(['Station', esc(st.name || 'WebSDR')]);
  if (st.subtitle) rows.push(['Beschreibung', esc(st.subtitle)]);
  if (st.locator || st.lat != null) rows.push(['Lage', [st.locator ? esc(st.locator) : '', st.lat != null ? dms(st.lat, 'N', 'S') + ' · ' + dms(st.lon, 'O', 'W') : ''].filter(Boolean).join(', ')]);
  $('site').innerHTML = rows.map(function (r) { return '<dt>' + r[0] + '</dt><dd>' + r[1] + '</dd>'; }).join('');
  // Betreiber (Impressum), wenn der Sysop ihn eingetragen hat
  if (st.operator) {
    var imp = [['Betreiber', esc(st.operator)]];
    if (st.address) imp.push(['Anschrift', esc(st.address).split(' · ').join('<br>')]);
    if (st.contact) imp.push(['Kontakt', /@/.test(st.contact) ? '<a href="mailto:' + esc(st.contact) + '">' + esc(st.contact) + '</a>' : esc(st.contact)]);
    $('imp').innerHTML = imp.map(function (r) { return '<dt>' + r[0] + '</dt><dd>' + r[1] + '</dd>'; }).join('');
    $('impwrap').hidden = false;
  }

  var B = window.bandinfo || [];
  $('bands').innerHTML = B.length ? B.map(function (b) {
    var lo = b.centerfreq - b.samplerate / 2, hi = b.centerfreq + b.samplerate / 2, d = hi > 1e6 ? 1 : 3;
    return '<dt>' + esc(b.label || b.name) + '</dt><dd>' + mhz(lo, d) + '–' + mhz(hi, d) + ' MHz' + (b.note ? ' <span class="muted">· ' + esc(b.note) + '</span>' : '') + '</dd>';
  }).join('') : '<dt>–</dt><dd class="muted">keine Bänder</dd>';

  var x = new XMLHttpRequest(); x.open('GET', '../api/decoders?' + Date.now());
  x.onload = function () {
    var D = []; try { D = JSON.parse(x.responseText).decoders || []; } catch (e) {}
    if (!D.length) return;
    $('decwrap').hidden = false;
    $('decs').innerHTML = D.map(function (d) {
      var ok = d.state === 'läuft';
      return '<dt>' + esc(d.label || d.plugin) + '</dt><dd>' + mhz(d.freq / 1000) + ' MHz ' + esc(String(d.mode || '').toUpperCase()) + ' · <span class="' + (ok ? 'ok' : 'muted') + '">' + esc(d.state) + '</span>' + (d.events ? ' <span class="muted">· ' + d.events + ' Meldungen seit dem Start</span>' : '') + '</dd>';
    }).join('');
  };
  x.send();
  if (UI.version) $('ver').textContent = 'Version ' + UI.version + (UI.build ? ' (' + UI.build + ')' : '') + '.';

  // Stream-Beispiele aus der Schnellwahl der Station (presets.json), als anklickbare, absolute Adressen
  (function () {
    var base = location.origin + location.pathname.replace(/info\/?(index\.html)?$/, '');
    var m3 = $('m3u'); if (m3) m3.href = base + 'stream/presets.m3u';
    var ex = $('exlink'); if (ex) { ex.href = base + '?tune=145500.000fm&sq=auto:6&ui=min'; ex.textContent = ex.href; }
    var x = new XMLHttpRequest(); x.open('GET', '../presets.json?' + Date.now());
    x.onload = function () {
      var P = []; try { P = JSON.parse(x.responseText); } catch (e) {}
      P = P.filter(function (p) { return p && p.freq && p.mode; }).slice(0, 6);
      var box = $('streamex'); if (!box) return;
      if (!P.length) { box.innerHTML = '<dt>Beispiel</dt><dd><a href="' + base + 'stream/145500/fm.ogg">' + base + 'stream/145500/fm.ogg</a></dd>'; return; }
      box.innerHTML = P.map(function (p) {
        var u = base + 'stream/' + p.freq + '/' + String(p.mode).toLowerCase() + '.ogg';
        return '<dt>' + esc(p.label || p.freq) + '</dt><dd><a href="' + u + '">' + u + '</a></dd>';
      }).join('') + '<dt>Senderliste</dt><dd><a href="' + base + 'stream/presets.m3u">' + base + 'stream/presets.m3u</a></dd>';
    };
    x.onerror = function () { var box = $('streamex'); if (box) box.innerHTML = '<dt>Beispiel</dt><dd><a href="' + base + 'stream/145500/fm.ogg">' + base + 'stream/145500/fm.ogg</a></dd>'; };
    x.send();
  })();
});
