/* crabSDR – Oberflächen-Logik.
   Baut auf den globalen Funktionen von core.js auf (setBand, setFreqText, setMode, setZoom, …)
   und liest dessen Zustand (freq, mode, band, lo, hi, bi[]). core.js bleibt dabei unverändert;
   überlagert werden nur: mouseFreq/touchFreq/touchPassband (Koordinaten, wegen CSS-Skalierung),
   setFreq (Raster-Einrasten) und freqStep (Schrittweite im Raster). */
(function () {
  'use strict';

  // Bandnamen, Beschreibung und Frequenzbereich aus der Konfiguration ([[sdrs]] label/note, bandinfo.js vom Server)
  var BANDMETA = {};
  (function () {
    function mhz(v) { return v >= 1000 ? (v / 1000).toFixed(3).replace('.', ',') : v.toFixed(1).replace('.', ','); }
    (typeof bandinfo !== 'undefined' ? bandinfo : []).forEach(function (b) {
      var lo = (b.centerfreq - b.samplerate / 2) / 1000, hi = (b.centerfreq + b.samplerate / 2) / 1000, g = lo >= 1000;
      BANDMETA[b.name] = { de: b.label || b.name, en: b.label_en || b.label || b.name, range: mhz(lo) + ' – ' + mhz(hi) + (g ? ' GHz' : ' MHz'),
        note_de: b.note || '', note_en: b.note_en || b.note || '' };
    });
  })();
  var SOON = [];   // Bänder ohne Empfänger stehen in ui.json bands_off (ausgegraut), nicht mehr hier
  var MODEFILTER = { fm: [-6, 6], am: [-4, 4], usb: [0.3, 2.7], lsb: [-2.7, -0.3], cw: [-0.95, -0.55] };
  var PHONE_MAX = 700;          // darunter: ein Band, Handy-Bedienleiste
  var MARKROW = 22;            // Streifen über der ersten Skala für Hörer-Marken (ui.css .rx-wrap padding-top)
  var PHONE_ZOOM = 2;           // Startzoom auf dem Handy (2 = Viertel des Bandes sichtbar)

  var lang = 'de', snap = 12.5, viewPref = 'one';   // Desktop: 'one' = ein Band groß (Standard), 'all' = alle Bänder gestapelt
  try { viewPref = localStorage.getItem('crab_view') === 'all' ? 'all' : 'one'; } catch (e) {}
  (function () { var m = /[?&]view=(all|one)\b/.exec(location.search); if (m) viewPref = m[1]; })();   // ?view=all als Deep-Link
  try { lang = localStorage.getItem('crab_lang') || 'de'; } catch (e) {}
  var snapUser = null; try { var sv = localStorage.getItem('crab_snap'); if (sv !== null) snapUser = Number(sv); } catch (e) {}
  if (snapUser !== null) snap = snapUser;
  var presets = [];
  var rx, wrap, scalesBox, scale = 1, phone = false, lastSig = '', _setfreq0 = null, layoutInit = false;

  function $(id) { return document.getElementById(id); }
  var CHAT_SIDE_MIN = 1100;   // ab dieser Fensterbreite sitzt der Chat rechts neben dem Wasserfall, darunter unter ihm
  function isPhone() { return window.innerWidth < PHONE_MAX; }
  // Wasserfallhöhe auf dem Handy: der Empfänger wird auf Bildschirmbreite verkleinert (1024 px → ~350 px), deshalb die
  // Canvas-Höhe so wählen, dass auf dem Bildschirm etwa 230 px übrig bleiben (vorher fest 200 Canvas-Pixel = ~70 px)
  function phoneWf() {
    var sc = Math.max(0.2, (window.innerWidth - 20) / 1024);
    var top = document.querySelector('.top'), bd = document.querySelector('.bands'), mb = $('mbar');
    var used = (top ? top.offsetHeight : 0) + (bd ? bd.offsetHeight : 0) + (mb ? mb.offsetHeight : 60) + MARKROW + 30;   // 30: Skala + Ränder
    var avail = Math.max(160, window.innerHeight - used);
    return Math.round(Math.min(2000, avail / sc));
  }
  /* Handy: Einstellungen als Menü von unten, Name/Chat/Fuß ziehen mit hinein; Leiste unten bekommt SQ/Stumm/Menü */
  var sheetReady = false;
  function openSheet(on) { document.body.classList.toggle('sheet-open', on === undefined ? !document.body.classList.contains('sheet-open') : !!on); }
  function initPhoneSheet() {
    if (sheetReady) return; sheetReady = true;
    var panel = document.querySelector('.panel'), head = document.createElement('div'); head.className = 'sheethead';
    var title = document.createElement('span'); title.className = 'sheettitle'; title.textContent = lang === 'de' ? 'Einstellungen' : 'Settings'; head.appendChild(title);
    ['myname', 'chatbtn'].forEach(function (id) { var e = $(id); if (e) head.appendChild(e); });
    var us = document.querySelector('.top .users'); if (us) head.appendChild(us);
    var cl = document.createElement('button'); cl.type = 'button'; cl.className = 'btn'; cl.textContent = '✕'; cl.setAttribute('aria-label', lang === 'de' ? 'Schließen' : 'Close'); cl.onclick = function () { openSheet(false); }; head.appendChild(cl);
    panel.insertBefore(head, panel.firstChild);
    // Reiter als eigene Zeile unter dem Kopf (im Kopf bleiben Logo, Name, Ton, Sprache)
    var tabs = document.querySelector('.top .tabs'), top = document.querySelector('.top'); if (tabs && top) { tabs.classList.add('phonetabs'); top.parentNode.insertBefore(tabs, top.nextSibling); }
    var sf = document.createElement('div'); sf.className = 'sheetfoot'; var foot = document.querySelector('.foot');
    while (foot && foot.firstChild) sf.appendChild(foot.firstChild);
    var tb = $('themebtn'); if (tb) sf.appendChild(tb);
    panel.appendChild(sf);
    var back = document.createElement('div'); back.className = 'sheetback'; back.onclick = function () { openSheet(false); }; document.body.appendChild(back);
    $('mset').onclick = function () { openSheet(); };
    $('msq').onclick = function () { setSquelch(!_crab.squelchOn); };
    $('mmute').onclick = function () { setMute(); };
  }
  function phoneBar() {
    if (!phone) return;
    var sq = $('msq'), mu = $('mmute');
    if (sq) { sq.classList.toggle('active', !!_crab.squelchOn); sq.classList.toggle('open', !!_crab.squelchOn && !!_crab.sq); }
    if (mu) mu.classList.toggle('active', !!(_crab.audio && _crab.audio.muted));
  }

  /* ================= Skalierung des Empfängerblocks ================= */
  function fit() {
    var host = wrap.parentNode, cs = getComputedStyle(host);
    var avail = host.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
    scale = Math.max(0.2, Math.min(3, avail / 1024));   // auch hochskalieren, bis die Breite gefüllt ist
    rx.style.transform = 'scale(' + scale + ')';
    wrap.style.width = Math.round(1024 * scale) + 'px';
    wrap.style.height = (Math.round(rx.offsetHeight * scale) + MARKROW) + 'px';   // + Streifen für Hörer-Marken (ui.css)
    renderScales(true);
    try { var tp = document.querySelector('.top'), bd = document.querySelector('.bands'), pn = document.querySelector('.panel'); document.documentElement.style.setProperty('--top-h', (tp.offsetHeight + bd.offsetHeight) + 'px'); document.documentElement.style.setProperty('--panel-h', (pn.offsetHeight + document.querySelector('.foot').offsetHeight) + 'px'); } catch (e) {}
    fillHeight(24);   // große Hysterese: Layout-Rundungen (z. B. Scrollleiste) dürfen keine Neuberechnung auslösen
  }
  /* Wasserfallhöhe „Bildschirm füllen": Empfänger soll den sichtbaren Bereich der Mitte genau ausfüllen (Desktop). */
  function fillHeight(tol) {
    if (phone || $('wfsize').value !== 'auto') return;
    tol = tol || 6;
    if (typeof wfHeight === 'undefined' || typeof wfWaiting === 'undefined' || wfWaiting > 0 || !nWaterfalls) return;
    var main = $('main'), host = wrap.parentNode, cs = getComputedStyle(host);
    var avail = main.clientHeight - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom) - 2 - MARKROW;   // 2 = Rahmen von .rx-wrap
    var extra = rx.offsetHeight - wfHeight * nWaterfalls;                                          // Skalen + Markerleisten (unskaliert)
    var want = Math.floor((avail / scale - extra) / nWaterfalls);
    want = nWaterfalls > 1 ? Math.max(70, Math.min(200, want)) : Math.max(120, Math.min(700, want));
    if (Math.abs(want - wfHeight) > tol) setWfHeight(want);
  }
  function targetView() { return (phone || viewPref === 'one') ? Views.oneband : Views.allbands; }
  function applyView() {
    var t = targetView();
    if (Number(view) !== t) { setView(t); lastSig = ''; }   // 'view' steht als Text im Cookie
    document.body.classList.toggle('allbands', Number(view) === Views.allbands);
    $('allbandschk').checked = Number(view) === Views.allbands;
    setTimeout(function () { fit(); fillHeight(); applyWfLevels(); renderListeners(); }, 300);
  }
  // Seitenpixel -> unskalierter 1024-px-Raum des Empfängerblocks. core.js rechnet
  // (pos.x - scaleEl.offsetParent.offsetLeft - 512) * khzPerPx + centerfreq; weil .rx transformiert ist,
  // ist .rx selbst der offsetParent (offsetLeft = 0), also muss x relativ zu .rx sein.
  function toRx(pageX, pageY) {
    var r = rx.getBoundingClientRect();
    return { x: (pageX - (r.left + window.pageXOffset)) / scale, y: (pageY - (r.top + window.pageYOffset)) / scale };
  }
  function overrideCore() {
    window.mouseFreq = function (e) { e = e || window.event; return toRx(e.pageX, e.pageY); };
    window.touchFreq = function (ev) {
      ev.preventDefault();
      for (var i = 0; i < ev.touches.length; i++) {
        var p = toRx(ev.touches[i].pageX, 0);
        setFreq((p.x - 512) * khzPerPx + centerfreq - (hi + lo) / 2);
      }
    };
    if (window.touchPassband) window.touchPassband = function (ev) {
      ev.preventDefault();
      if (ev.touches.length !== 1) return;
      setFreq(dragStartVal + (ev.touches[0].pageX - dragStartX) / scale * khzPerPx);
    };
    // Raster-Einrasten: gilt für Klick/Tipp/Ziehen, nicht für getippte Frequenzen (setFreqText setzt das Flag)
    _setfreq0 = window.setFreq;
    var _setfreq = _setfreq0;
    window.setFreqExact = _setfreq0;                   // ohne Raster (↑ ↓, getippte Frequenz)
    function rasterMode() { return mode === 'FM' || mode === 'AM'; }   // SSB/CW haben kein Kanalraster
    window.setFreq = function (f) {
      if (snap > 0 && !freqTextLock && rasterMode()) {
        var off = isCw() ? (hi + lo) / 2 : 0;
        f = Math.round((f + off) / snap) * snap - off;
      }
      _setfreq(f);
    };
    var _freqstep = window.freqStep;
    window.freqStep = function (st) {
      if (snap > 0 && rasterMode()) {
        var n = Math.abs(st) === 1 ? 1 : Math.abs(st) === 2 ? 4 : 20;
        var off = isCw() ? (hi + lo) / 2 : 0;
        var cur = Math.round((freq + off) / snap) * snap;
        _setfreq(cur + (st > 0 ? 1 : -1) * n * snap - off);
      } else _freqstep(st);
    };
  }

  /* ================= Eigene, scharfe Frequenzskala ================= */
  function niceStep(khzPerScreenPx) {
    var steps = [1, 2, 5, 10, 12.5, 20, 25, 50, 100, 200, 250, 500, 1000];
    for (var i = 0; i < steps.length; i++) if (steps[i] / khzPerScreenPx >= 64) return steps[i];
    return 1000;
  }
  function fmtScale(khz, step) {
    var dec = step >= 1000 ? 0 : step >= 100 ? 1 : step >= 10 ? 2 : 3;
    return (khz / 1000).toFixed(dec).replace('.', ',');
  }
  function renderScales(force) {
    if (typeof nWaterfalls === 'undefined' || !bi || !bi[0] || nWaterfalls === 0) { scalesBox.innerHTML = ''; return; }
    var sig = scale + '|' + nWaterfalls + '|' + lang;
    for (var k = 0; k < nWaterfalls; k++) { var b0 = id2band(k), e0 = bi[b0]; sig += '|' + b0 + ':' + e0.zoom + ':' + e0.start; }
    var cont = $('waterfalls'); sig += '|' + cont.offsetHeight;
    if (!force && sig === lastSig) return;
    lastSig = sig;
    var html = '';
    for (var i = 0; i < nWaterfalls; i++) {
      var b = id2band(i), e = bi[b], clip = $('clipscale' + i);
      if (!clip || clip.offsetParent === null) continue;          // ausgeblendetes Band: keine Skala zeichnen
      clip.style.visibility = 'hidden';                          // Bitmap-Skala ausblenden (eigene Skala)
      var top = Math.round((clip.offsetTop + clip.offsetParent.offsetTop) * scale);
      var fl = e.effcenterfreq - e.effsamplerate / 2;            // linke Kante in kHz
      var kpp = e.effsamplerate / (1024 * scale);                // kHz pro Bildschirmpixel
      var step = niceStep(kpp), fine = step / (step >= 100 ? 5 : 4);
      var s = '<div class="fscale" data-wf="' + i + '" style="top:' + top + 'px;width:' + Math.round(1024 * scale) + 'px">';
      var f0 = Math.ceil(fl / fine) * fine;
      for (var f = f0; f < fl + e.effsamplerate; f += fine) {
        var x = (f - fl) / kpp, major = Math.abs(f / step - Math.round(f / step)) < 1e-6;
        s += '<i class="' + (major ? 'maj' : 'min') + '" style="left:' + x.toFixed(1) + 'px"></i>';
        if (major) s += '<b style="left:' + x.toFixed(1) + 'px">' + fmtScale(f, step) + '</b>';
      }
      var m = BANDMETA[e.name];
      s += '<span class="fscale-band">' + (m ? m[lang] + ' · ' + m.range : e.name) + '</span></div>';
      html += s;
    }
    scalesBox.innerHTML = html;
  }
  function scalePointer(ev) {
    var el = ev.target.closest ? ev.target.closest('.fscale') : null;
    if (!el) return;
    var i = Number(el.dataset.wf), b = id2band(i), e = bi[b];
    if (Number(view) !== Views.oneband && band !== b) setBand(b);
    var r = el.getBoundingClientRect();
    var f = e.effcenterfreq - e.effsamplerate / 2 + (ev.clientX - r.left) / (1024 * scale) * e.effsamplerate;
    setFreq(f - (hi + lo) / 2);
    ev.preventDefault();
  }

  /* ================= Wasserfall-Pegel (unabhängig vom S-Meter) =================
     Admin-Vorgaben aus ui.json (global + je Band), Hörer kann lokal nachregeln (localStorage). */
  var ui = { waterfall: { brightness: 1, contrast: 1, bands: {} }, smeter: { offset: 0, bands: {} } }, lastFilter = {}, wfUser = null, bandsOff = {};
  function bandOff(name) { return !!bandsOff[name]; }
  try { wfUser = JSON.parse(localStorage.getItem('crab_wf')); } catch (e) {}
  function wfLevel(bandName) {
    var w = ui.waterfall || {}, b = (w.bands && w.bands[bandName]) || {};
    var br = b.brightness != null ? b.brightness : (w.brightness != null ? w.brightness : 1);
    var co = b.contrast != null ? b.contrast : (w.contrast != null ? w.contrast : 1);
    if (wfUser) { br *= wfUser.brightness || 1; co *= wfUser.contrast || 1; }
    return { brightness: br, contrast: co };
  }
  function applyWfLevels() {
    if (typeof nWaterfalls === 'undefined') return;
    for (var i = 0; i < nWaterfalls; i++) {
      var d = $('wfdiv' + i); if (!d) continue;
      var off = nWaterfalls > 1 && bandOff(bi[id2band(i)].name), want = off ? 'none' : '';
      ['wfdiv', 'clipscale', 'blackbar'].forEach(function (pre) { var el = $(pre + i); if (el && el.style.display !== want) el.style.display = want; });
      if (off) continue;
      var l = wfLevel(bi[id2band(i)].name);
      var f = 'brightness(' + l.brightness.toFixed(2) + ') contrast(' + l.contrast.toFixed(2) + ')';
      if (lastFilter[i] !== f || d.style.filter !== f) { d.style.filter = f; lastFilter[i] = f; }
    }
  }
  function initAdvanced() {
    // Helligkeit/Kontrast je Hörer
    var sb = $('wfbright'), sc = $('wfcontrast');
    sb.value = wfUser ? wfUser.brightness : 1; sc.value = wfUser ? wfUser.contrast : 1;
    function save() { wfUser = { brightness: Number(sb.value), contrast: Number(sc.value) }; try { localStorage.setItem('crab_wf', JSON.stringify(wfUser)); } catch (e) {} applyWfLevels(); }
    sb.oninput = save; sc.oninput = save;
    $('wfreset').onclick = function () { wfUser = null; try { localStorage.removeItem('crab_wf'); } catch (e) {} sb.value = 1; sc.value = 1; applyWfLevels(); };
    // Sticky
    var st = false; try { st = localStorage.getItem('crab_sticky') === '1'; } catch (e) {}
    $('stickychk').checked = st; document.body.classList.toggle('sticky', st);
    $('stickychk').onchange = function () { document.body.classList.toggle('sticky', this.checked); try { localStorage.setItem('crab_sticky', this.checked ? '1' : '0'); } catch (e) {} };
    // A/B-VFO, .0, Speicher
    var vfoB = null;
    function cur() { return { band: band, freq: freq, mode: mode, lo: lo, hi: hi }; }
    function apply(v) {
      if (band !== v.band) setBand(v.band);
      setMode(v.mode.toLowerCase(), v.lo, v.hi);
      freqTextLock = true; setFreq(v.freq); freqTextLock = false;
      document.freqform.frequency.value = dialFreq().toFixed(2);
      setWaterfall(band, freq);
    }
    $('vfoab').onclick = function () { var a = cur(); if (vfoB) apply(vfoB); else toast(lang === 'de' ? 'VFO B ist leer – erst A=B drücken' : 'VFO B empty – press A=B first'); vfoB = a; };
    $('vfoaeqb').onclick = function () { vfoB = cur(); toast(lang === 'de' ? 'VFO B gesetzt: ' + (dialFreq() / 1000).toFixed(4) + ' MHz' : 'VFO B set'); };
    $('dot0').onclick = function () { setFreqText(String(Math.round(dialFreq()))); };
    $('memstore').onclick = function () { memStore(memSlots.length); toast(lang === 'de' ? 'Gemerkt' : 'Stored'); };
  }
  function loadUi() {
    var x = new XMLHttpRequest();
    x.open('GET', 'ui.json?' + Date.now());
    x.onload = function () {
      try { ui = JSON.parse(x.responseText); } catch (e) {}
      bandsOff = {}; (ui.bands_off || []).forEach(function (n) { bandsOff[n] = true; });
      buildBandBar(); buildPresets(); lastSig = '';
      if (bandOff(bandinfo[band].name)) { for (var k = 0; k < nbands; k++) if (!bandOff(bandinfo[k].name)) { setBand(k); break; } }
      if (ui.snap != null && snapUser === null) { snap = Number(ui.snap); $('snapsel').value = String(snap); }
      if (ui.waterfall && ui.waterfall.mode != null && $('wfmode').value !== String(ui.waterfall.mode)) { $('wfmode').value = String(ui.waterfall.mode); setWfMode(ui.waterfall.mode); }
      applyWfLevels();
    };
    x.send();
  }

  /* ================= Hörer-Marker (aus der Sekunden-Aktualisierung des Servers) ================= */
  var lmarks;
  // Der Server nennt Hörer ohne Namen nach ihrer IP-Adresse (auch IPv6). Die zeigen wir nie an, sondern „Hörer".
  var IPRE = /^\s*\(?(?:\d{1,3}(?:\.\d{1,3}){3}|[0-9a-f]{0,4}(?::[0-9a-f]{0,4}){2,7})\)?\s*$/i;
  function dispName(n) { return (!n || IPRE.test(n)) ? (lang === 'en' ? 'listener' : 'Hörer') : n; }
  function renderListeners() {
    if (!lmarks || typeof nWaterfalls === 'undefined' || !bi || !bi[0]) return;
    var html = '', me = document.usernameform.username.value;
    for (var i = 0; i < nWaterfalls; i++) {
      var b = id2band(i), e = bi[b], clip = $('clipscale' + i);
      if (!clip || clip.offsetParent === null) continue;
      var top = Math.round((clip.offsetTop + clip.offsetParent.offsetTop) * scale) - 20;
      // gehörtes Band: Durchlassbereich als Balken in der Skala (nur Anzeige; Bandbreite stellt man unten ein)
      // und eingestellte Frequenz als dünne Linie von der Skala durch den ganzen Wasserfall
      var B = _crab.bands[b];
      if (b === band && B && B.canvas) {
        var tx = freq2x(freq, b) * scale, lr = lmarks.getBoundingClientRect(), cr = B.canvas.getBoundingClientRect();
        var xl = freq2x(freq + Math.min(lo, 0), b) * scale, xh = freq2x(freq + Math.max(hi, 0), b) * scale;
        if (xh > 0 && xl < 1024 * scale) html += '<i class="pband" style="left:' + Math.max(0, xl).toFixed(1) + 'px;width:' + Math.max(2, Math.min(1024 * scale, xh) - Math.max(0, xl)).toFixed(1) + 'px;top:' + (top + 20) + 'px"></i>';
        if (tx >= 0 && tx <= 1024 * scale) html += '<i class="tline" style="left:' + tx.toFixed(1) + 'px;top:' + (top + 20) + 'px;height:' + Math.max(0, Math.round(cr.bottom - lr.top - top - 20)) + 'px"></i>';
      }
      var items = [];
      for (var j = 0; j < lsNames.length; j++) {
        if (lsBands[j] !== b || !lsNames[j]) continue;
        var fabs = e.centerfreq - e.samplerate / 2 + lsFreqs[j] * e.samplerate;
        var x = freq2x(fabs, b) * scale;
        if (x < 0 || x > 1024 * scale) continue;
        var mine = (lsNames[j] === me && b === band && Math.abs(fabs - freq) < e.samplerate / 200);
        items.push({ x: x, name: dispName(lsNames[j]), c: listenerColours[j % 8], mine: mine });
      }
      items.sort(function (p, q) { return p.x - q.x; });
      // Hörer auf (fast) derselben Frequenz zu einer Marke zusammenfassen: Farbstreifen aus allen Hörerfarben
      var k = 0;
      while (k < items.length) {
        var grp = [items[k]], m = k + 1;
        while (m < items.length && items[m].x - grp[grp.length - 1].x <= 5) { grp.push(items[m]); m++; }
        k = m;
        var cx = grp.reduce(function (acc, it) { return acc + it.x; }, 0) / grp.length;
        var mineAny = grp.some(function (it) { return it.mine; });
        if (grp.length === 1) {
          html += '<em class="' + (mineAny ? 'me' : '') + '" style="left:' + cx.toFixed(0) + 'px;top:' + top + 'px;--c:' + grp[0].c + '">' + escapeHtml(grp[0].name) + '</em>';
        } else {
          var stops = grp.map(function (it, n) { var a0 = (100 * n / grp.length).toFixed(1), a1 = (100 * (n + 1) / grp.length).toFixed(1); return it.c + ' ' + a0 + '% ' + a1 + '%'; }).join(',');
          var names = grp.map(function (it) { return escapeHtml(it.name); });
          var label = names.length <= 3 ? names.join(' · ') : names.slice(0, 2).join(' · ') + ' +' + (names.length - 2);
          html += '<em class="multi' + (mineAny ? ' me' : '') + '" style="left:' + cx.toFixed(0) + 'px;top:' + top + 'px;--gv:linear-gradient(to bottom,' + stops + ');--gh:linear-gradient(to right,' + stops + ');--n:' + grp.length + '" title="' + names.join(', ') + '">' + label + '</em>';
        }
      }
    }
    lmarks.innerHTML = html;
  }
  function escapeHtml(t) { return String(t).replace(/[&<>"]/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]; }); }
  function initName() {
    var inp = $('myname'), hidden = document.usernameform.username;
    var c = readCookie('username');
    if (c) { hidden.value = decodeURIComponent(c); inp.value = hidden.value; }
    inp.onchange = function () {
      var v = inp.value.trim().slice(0, 20) || 'Hörer';
      hidden.value = v; saveName();
    };
  }
  function serverStats() {
    // Gesamtlast des Rechners (alle Kerne), wenn der Server sie liefert; sonst die Last des Serverprozesses
    if (typeof _crab !== 'undefined' && _crab.sysPct != null) return _crab.sysPct;
    var t = $('stats').textContent || '', m = /CPUload=([\d.]+)%/.exec(t);
    return m ? Number(m[1]) : null;
  }

  /* ================= Deep-Link, Marker, Logbuch, Sysop, Web-App ================= */
  function toast(msg) {
    var t = $('toast'); if (!t) { t = document.createElement('div'); t.id = 'toast'; t.className = 'toast'; document.body.appendChild(t); }
    t.textContent = msg; t.classList.add('show'); clearTimeout(t._h); t._h = setTimeout(function () { t.classList.remove('show'); }, 1800);
  }
  function tuneLink() { return location.origin + location.pathname + '?tune=' + dialFreq().toFixed(3) + mode.toLowerCase() + '&pb=' + lo.toFixed(2) + ',' + hi.toFixed(2); }   // pb = Durchlassbereich lo,hi in kHz
  // Teilen: auf Handys das System-Menü (WhatsApp, Telegram, …), sonst Link + kurzer Text in die Zwischenablage
  function copyLink() {
    var url = tuneLink(), f = (dialFreq() / 1000).toFixed(4).replace('.', ',') + ' MHz ' + mode;
    var sn = window.STATION_NAME || 'crabSDR', ws = /websdr/i.test(sn); var text = lang === 'de' ? f + (ws ? ' auf ' : ' auf dem WebSDR ') + sn : f + ' on ' + sn + (ws ? '' : ' WebSDR');
    function ok() { toast(lang === 'de' ? 'Link kopiert – einfach einfügen' : 'Link copied – just paste it'); }
    if (navigator.share && (phone || matchMedia('(pointer: coarse)').matches)) {
      navigator.share({ title: (window.STATION_NAME || 'crabSDR') + ' · ' + f, text: text, url: url }).catch(function () {});
      return;
    }
    var both = text + '\n' + url;
    if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(both).then(ok, function () { window.prompt('Link', url); });
    else window.prompt('Link', url);
  }
  // Marker: eigene markers.json statt Server-Abfrage; showMarks() aus base.js zeichnet sie unter dem Wasserfall
  var markers = [];
  function loadMarkers() {
    var x = new XMLHttpRequest(); x.open('GET', 'markers.json?' + Date.now());
    x.onload = function () { try { markers = JSON.parse(x.responseText); } catch (e) { markers = []; } for (var i = 0; i < nWaterfalls; i++) loadMarks(id2band(i)); };
    x.send();
  }
  window.loadMarks = function (b) {
    var e = bi[b]; if (!e) return;
    var lo_ = e.effcenterfreq - e.effsamplerate / 2, hi_ = e.effcenterfreq + e.effsamplerate / 2;
    marks = markers.filter(function (m) { return m.freq >= lo_ && m.freq <= hi_; })
                 .sort(function (a, c) { return a.freq - c.freq; })
                 .map(function (m) { return { freq: m.freq, mode: (m.mode || 'fm').toUpperCase(), text: m.text }; });
    showMarks(b);
  };
  // Logbuch
  // Sysop
  function sysCall(ep, param, value) {
    var url = ep + (param ? (ep.indexOf('?') < 0 ? '?' : '&') + param + '=' + encodeURIComponent(value) : '');
    var x = new XMLHttpRequest(); x.open('GET', url);
    x.onload = function () { $('sysmsg').textContent = (x.status === 200 ? 'OK: ' : 'Fehler ' + x.status + ': ') + ep + (x.status === 403 ? ' – kein Sysop-Zugang von dieser Adresse' : ''); };
    x.onerror = function () { $('sysmsg').textContent = 'Fehler: ' + ep; };
    x.send();
  }
  function initPwa() {
    if ('serviceWorker' in navigator && window.isSecureContext) { try { navigator.serviceWorker.register('sw.js'); } catch (e) {} }
  }

  /* ================= Bandbreiten-Vorgaben je Betriebsart (kHz, symmetrisch bzw. SSB einseitig) ================= */
  var BWPRESETS = { FM: [7, 10, 12, 15], AM: [4, 6, 8, 10], USB: [1.8, 2.4, 2.7, 3.2], LSB: [1.8, 2.4, 2.7, 3.2], CW: [0.1, 0.25, 0.4, 0.8] };
  var lastBwMode = '';
  function setBw(kHz) {
    var m = mode;
    if (m === 'FM' || m === 'AM') { lo = -kHz / 2; hi = kHz / 2; }
    else if (m === 'USB') { lo = 0.3; hi = 0.3 + kHz; }
    else if (m === 'LSB') { hi = -0.3; lo = -0.3 - kHz; }
    else { var c = (lo + hi) / 2; lo = c - kHz / 2; hi = c + kHz / 2; }   // CW um die Mitte
    updateBw();
  }
  function buildBwPresets() {
    if (mode === lastBwMode) return;
    lastBwMode = mode;
    var box = $('bwpresets'); box.innerHTML = '';
    (BWPRESETS[mode] || []).forEach(function (k) {
      var b = document.createElement('button'); b.type = 'button'; b.className = 'btn';
      var tag = mode === 'FM' ? ({ 10: lang === 'de' ? 'schmal' : 'narrow', 12: 'Standard', 15: lang === 'de' ? 'breit' : 'wide' })[k] : '';
      b.innerHTML = (k < 1 ? (k * 1000) + ' Hz' : k + ' k') + (tag ? '<small>' + tag + '</small>' : ''); b.onclick = function () { setBw(k); };
      b.title = mode === 'FM' ? ({ 7: 'Icom FIL3', 10: 'FM-N, 12,5-kHz-Raster, ±2,5 kHz Hub (Icom FIL2)', 12: 'Standard: passt für ±5 kHz Hub im 12,5-kHz-Raster', 15: 'FM breit, ±5 kHz Hub, 25-kHz-Raster (Icom FIL1)' })[k] || '' : '';
      box.appendChild(b);
    });
  }
  function markBwPreset() {
    var cur = hi - lo, bs = document.querySelectorAll('#bwpresets .btn');
    (BWPRESETS[mode] || []).forEach(function (k, i) { if (bs[i]) bs[i].classList.toggle('active', Math.abs(cur - k) < 0.05); });
  }

  /* ================= Sprache ================= */
  function applyLang() {
    var els = document.querySelectorAll('[data-de]');
    for (var i = 0; i < els.length; i++) {
      var t = els[i].getAttribute('data-' + lang);
      if (t !== null) els[i].textContent = t;
    }
    var ph = document.querySelectorAll('[data-ph-de]');
    for (var q = 0; q < ph.length; q++) ph[q].placeholder = ph[q].getAttribute('data-ph-' + lang);
    $('langbtn').textContent = lang === 'de' ? 'EN' : 'DE';
    document.documentElement.lang = lang;
    buildBandBar(); buildPresets(); renderScales(true);
  }

  /* ================= Bandleiste ================= */
  function buildBandBar() {
    var bar = $('bandbar'); bar.innerHTML = '';
    for (var i = 0; i < nbands; i++) {
      var name = bandinfo[i].name, m = BANDMETA[name] || { de: name, en: name, range: '', note_de: '', note_en: '' };
      var b = document.createElement('button');
      b.type = 'button'; b.className = 'band' + (bandOff(name) ? ' soon off' : ''); b.dataset.band = i;
      b.innerHTML = '<b>' + m[lang] + ' <small>' + m.range + '</small>' + (m.test ? ' <i class="testtag">Test</i>' : '') + '</b>';
      b.title = bandOff(name) ? (lang === 'de' ? 'zur Zeit kein Empfänger' : 'no receiver at the moment') : (lang === 'de' ? m.note_de : m.note_en) + (m.test ? (lang === 'de' ? ' – Testbetrieb, Frequenzanzeige noch nicht geprüft' : ' – test operation, frequency readout not yet verified') : '');
      if (!bandOff(name)) b.onclick = (function (n) { return function () { gotoBand(n); }; })(i);
      bar.appendChild(b);
    }
    for (var j = 0; j < SOON.length; j++) {
      var s = SOON[j], d = document.createElement('div');
      d.className = 'band soon';
      d.innerHTML = '<b>' + s[lang] + ' <small>' + s.range + '</small></b>';
      d.title = lang === 'de' ? s.note_de : s.note_en;
      bar.appendChild(d);
    }
    var r = $('bandradios'); r.innerHTML = '';
    for (var k = 0; k < nbands; k++) {
      var inp = document.createElement('input');
      inp.type = 'radio'; inp.name = 'group0'; inp.value = bandinfo[k].name;
      inp.onclick = (function (n) { return function () { gotoBand(n); }; })(k);
      r.appendChild(inp);
    }
  }
  function gotoBand(n) {
    if (bandOff(bandinfo[n].name)) return;
    setBand(n);
    if (phone) zoomToFreq(n, PHONE_ZOOM, freq);
    lastSig = ''; renderScales(true); renderListeners(); applyWfLevels();   // sofort auf das neue Band umstellen
  }

  /* ================= Bereiche (IARU-Region-1-Bandplan): Wasserfall auf einen Ausschnitt zoomen =================
     Voreinstellung für 2 m und 70 cm; eine Station überschreibt sie mit segments.json im Stationsordner:
     [{"lo": 144150, "hi": 144400, "label": "SSB", "mode": "usb", "call": 144300}, …] (kHz). */
  var BANDPLAN = [
    { lo: 144000, hi: 144150, label: 'CW / EME', mode: 'cw', call: 144050 },
    { lo: 144150, hi: 144400, label: 'SSB', mode: 'usb', call: 144300 },
    { lo: 144400, hi: 144500, label: 'Baken', mode: 'cw' },
    { lo: 144500, hi: 144800, label: 'Allmode (SSTV, RTTY, FAX)', mode: 'usb', call: 144500 },
    { lo: 144800, hi: 144990, label: 'APRS / Packet', mode: 'fm', call: 144800 },
    { lo: 144975, hi: 145200, label: 'Relaiseingaben', mode: 'fm' },
    { lo: 145200, hi: 145600, label: 'FM-Simplex', mode: 'fm', call: 145500 },
    { lo: 145575, hi: 145800, label: 'Relaisausgaben', mode: 'fm' },
    { lo: 145800, hi: 146000, label: 'Satelliten', mode: 'fm', call: 145800 },
    { lo: 430000, hi: 431975, label: 'Relaiseingaben (7,6 MHz)', mode: 'fm' },
    { lo: 432000, hi: 432100, label: 'CW / EME', mode: 'cw', call: 432050 },
    { lo: 432100, hi: 432400, label: 'SSB', mode: 'usb', call: 432200 },
    { lo: 432400, hi: 432500, label: 'Baken', mode: 'cw' },
    { lo: 432500, hi: 433000, label: 'Allmode / Simplex', mode: 'fm', call: 432500 },
    { lo: 433000, hi: 433400, label: 'Relaiseingaben (1,6 MHz)', mode: 'fm' },
    { lo: 433400, hi: 433600, label: 'FM-Simplex', mode: 'fm', call: 433500 },
    { lo: 433600, hi: 434000, label: 'Allmode / Digital', mode: 'fm' },
    { lo: 434600, hi: 435000, label: 'Relaisausgaben (1,6 MHz)', mode: 'fm' },
    { lo: 435000, hi: 438000, label: 'Satelliten', mode: 'usb' },
    { lo: 438650, hi: 439425, label: 'Relaisausgaben (7,6 MHz)', mode: 'fm' },
    { lo: 10368000, hi: 10368200, label: 'Baken', mode: 'cw' },
    { lo: 10368200, hi: 10368400, label: 'SSB / CW', mode: 'usb', call: 10368200 }
  ];
  var segments = null, segBand = -1;
  function bandSegments(b) {
    var e = bi[b]; if (!e) return [];
    var lo_ = e.centerfreq - e.samplerate / 2, hi_ = e.centerfreq + e.samplerate / 2;
    return (segments || BANDPLAN).filter(function (s) { return s.hi > lo_ && s.lo < hi_ && (!s.band || s.band === e.name); })
      .map(function (s) { return { lo: Math.max(s.lo, lo_), hi: Math.min(s.hi, hi_), label: s.label, mode: s.mode, call: s.call }; })
      .filter(function (s) { return s.hi - s.lo >= 10; });
  }
  function buildSegments() {
    var sel = $('segsel'); if (!sel) return;
    var list = bandSegments(band); segBand = band;
    sel.innerHTML = ''; var o0 = document.createElement('option'); o0.value = ''; o0.textContent = lang === 'en' ? 'Range…' : 'Bereich…'; sel.appendChild(o0);
    list.forEach(function (s, i) { var o = document.createElement('option'); o.value = String(i); o.textContent = s.label + '  ' + (s.lo / 1000).toFixed(3) + '–' + (s.hi / 1000).toFixed(3); sel.appendChild(o); });
    sel.hidden = !list.length;
  }
  function zoomSegment(s) {
    var e = bi[band], width = (s.hi - s.lo) * 1.04, z = 0;
    while (z < _crabMaxZoom(e) && e.samplerate / Math.pow(2, z + 1) >= width) z++;
    if (s.mode && MODEFILTER[s.mode]) { var f = MODEFILTER[s.mode]; setMode(s.mode, f[0], f[1]); }
    var target = s.call && s.call >= s.lo && s.call <= s.hi ? s.call : (s.lo + s.hi) / 2;
    if (Math.abs(freq - target) > 0.01 && (freq < s.lo || freq > s.hi)) setFreqText(String(target));
    zoomToFreq(band, z, (s.lo + s.hi) / 2);
  }

  /* ================= Schnellwahl ================= */
  function buildPresets() {
    presets = presets.filter(function (p) { return !bandOff(p.band); });
    var box = $('presets'); box.innerHTML = '';
    var first = document.createElement('option'); first.value = ''; first.textContent = lang === 'en' ? 'Quick tune…' : 'Schnellwahl…'; box.appendChild(first);
    var groups = {};
    for (var i = 0; i < presets.length; i++) {
      var p = presets[i], idx = -1;
      for (var k = 0; k < nbands; k++) if (bandinfo[k].name === p.band) idx = k;
      if (idx < 0) continue;
      var g = groups[p.band];
      if (!g) { g = document.createElement('optgroup'); g.label = (BANDMETA[p.band] ? BANDMETA[p.band][lang] + ' · ' + BANDMETA[p.band].range : p.band); groups[p.band] = g; box.appendChild(g); }
      var o = document.createElement('option');
      o.value = String(i); o.dataset.idx = String(idx);
      o.textContent = (lang === 'en' && p.label_en ? p.label_en : p.label) + '  ' + (p.freq / 1000).toFixed(3) + ' ' + p.mode.toUpperCase();
      g.appendChild(o);
    }
    box.onchange = function () {
      this.blur();   // Fokus freigeben, sonst schluckt das Auswahlfeld die Pfeiltasten
      var o = this.options[this.selectedIndex];
      if (o && o.value !== '') tunePreset(presets[Number(o.value)], Number(o.dataset.idx));
      this.selectedIndex = 0;
    };
  }
  // Frequenz in kHz (beliebiges Band) einstellen, Betriebsart optional – für Chat-Links, Aktivitätsliste, Relais-Tabelle
  function bandOf(kHz) {
    for (var k = 0; k < nbands; k++) {
      var e = bi[k]; if (!e || bandOff(e.name)) continue;
      if (kHz >= e.effcenterfreq - e.effsamplerate / 2 && kHz <= e.effcenterfreq + e.effsamplerate / 2) return k;
    }
    return -1;
  }
  function tuneTo(kHz, md) {
    var k = bandOf(kHz); if (k < 0) return;
    if (band !== k) gotoBand(k);
    if (md && MODEFILTER[md]) { var f = MODEFILTER[md]; setMode(md, f[0], f[1]); }
    setFreqText(String(kHz));
    if (phone) zoomToFreq(band, PHONE_ZOOM, kHz);
  }
  function tunePreset(p, idx) {
    if (band !== idx) setBand(idx);
    var f = MODEFILTER[p.mode] || MODEFILTER.fm;
    setMode(p.mode, f[0], f[1]);
    setFreqText(String(p.freq));
    if (phone) zoomToFreq(idx, PHONE_ZOOM, p.freq);
  }

  /* ================= Anzeige aus dem Zustand von core.js ================= */
  function refresh() {
    var f = dialFreq(), txt = (f / 1000).toFixed(4).replace('.', ',');
    $('freqmhz').innerHTML = txt + '<small>MHz</small>';
    $('mfreq').textContent = txt;
    renderListeners(); buildBwPresets(); markBwPreset();
    var mb = document.querySelectorAll('#modes .btn');
    for (var i = 0; i < mb.length; i++) mb[i].classList.toggle('active', mb[i].dataset.mode === mode);
    var bb = document.querySelectorAll('#bandbar .band[data-band]');
    for (var j = 0; j < bb.length; j++) bb[j].classList.toggle('active', Number(bb[j].dataset.band) === band);
    var ms = $('mmode'); if (ms.value !== mode.toLowerCase()) ms.value = mode.toLowerCase();
    renderScales(false);
    applyWfLevels();
    phoneBar();
    if (segBand !== band) buildSegments();
    var ag = $('agccontrol'); if (ag) ag.classList.toggle('off', mode === 'FM');
  }

  /* ================= S-Meter nach IARU Region 1 für VHF/UHF: S9 = −93 dBm, 6 dB je S-Stufe, S1 = −141 dBm =================
     Rohwert des Servers (dB, relativ) -> dBm:  roh − Stick-Verstärkung − WebSDR-gain + K[Band].
     K (ui.json smeter.cal) wird einmal gemessen und bezieht sich auf 0 dB Verstärkung; die aktuellen Verstärkungen
     liefert gains.json (Wächter), deshalb bleibt die Anzeige nach einer Verstärkungsänderung richtig.
     Bänder ohne K: alter Offset (smeter.bands/offset) als Übergang. Balken: −145 … −25 dBm. */
  var gains = {}, SNR_SPAN = 60, peak = 0, peakTimer = 0;   // Balken: 0–60 dB über dem Rauschen
  function loadGains() {
    var x = new XMLHttpRequest(); x.open('GET', 'gains.json?' + Date.now());
    x.onload = function () { try { gains = JSON.parse(x.responseText).bands || {}; } catch (e) {} };
    x.send();
  }
  function smeterDbm(raw, bn) {
    var sm = ui.smeter || {}, cal = sm.cal || {};
    if (cal[bn] != null) { var g = gains[bn] || {}; return raw - (g.rtl_gain || 0) - (g.ws_gain || 0) + cal[bn]; }
    return raw + ((sm.bands && sm.bands[bn] != null) ? sm.bands[bn] : (sm.offset || 0));
  }
  function meter() {
    var s = 0;
    try { s = crabAudio.smeter(); } catch (e) { return; }
    var bn = bi && bi[band] ? bi[band].name : '';
    // Hauptwert: Rauschabstand (SNR) – Signal über dem Rauschen des Kanals, den der Server mitmisst; stimmt ohne Kalibrierung
    var snr = (typeof _crab !== 'undefined' && _crab.level > -150 && _crab.floor > -150) ? Math.max(0, _crab.level - _crab.floor) : 0;
    var pct = function (v) { return Math.max(0, Math.min(100, v / SNR_SPAN * 100)); };
    $('gfill').style.width = pct(snr) + '%';
    $('mfill').style.width = pct(snr) + '%';
    if (snr > peak || --peakTimer <= 0) { peak = snr; peakTimer = 15; }
    $('gpeak').style.left = pct(peak) + '%';
    $('gsnr').textContent = '+' + Math.round(snr);
    // Nebenwert: S-Stufe und dBm nur, wenn die Station kalibriert ist (ui.json smeter.cal für dieses Band)
    var cal = (ui.smeter && ui.smeter.cal) || {}, abs = '';
    if (cal[bn] != null) {
      var dbm = smeterDbm(s / 100 - 127, bn);
      abs = (dbm <= -93 ? 'S' + Math.max(0, Math.min(9, 1 + Math.floor((dbm + 141) / 6))) : 'S9+' + Math.round(dbm + 93)) + ' · ' + dbm.toFixed(0) + ' dBm';
    }
    $('gabs').textContent = abs ? '· ' + abs : '';
    var mt = $('mtext'); if (mt) mt.textContent = '+' + Math.round(snr) + ' dB SNR' + (abs ? ' · ' + abs : '');
  }

  /* ================= Ton freischalten ================= */
  function audioState() { return document.ct ? document.ct.state : 'none'; }
  function unlockAudio() {
    if (document.ct && document.ct.state === 'suspended') document.ct.resume().then(updateAudioBtn, updateAudioBtn);
    else updateAudioBtn();
  }
  function updateAudioBtn() { document.body.classList.toggle('audio-ready', audioState() === 'running'); }

  /* ================= Frequenzliste: eigene Relais (relais.json), per APRS gehörte Relais, Marker ================= */
  function initFreqList() {
    var box = $('flist'), body = $('flistbody'), data = { own: [], aprs: [] };
    function get(url, cb) { var x = new XMLHttpRequest(); x.open('GET', url + (url.indexOf('?') < 0 ? '?' : '&') + '_=' + Date.now()); x.onload = function () { try { cb(JSON.parse(x.responseText)); } catch (e) { cb(null); } }; x.onerror = function () { cb(null); }; x.send(); }
    function shift(o) { if (!o) return ''; var n = Number(o); if (!n) return ''; return (n > 0 ? '+' : '−') + (Math.abs(n) / 1000).toFixed(Math.abs(n) % 1000 ? 1 : 0).replace('.', ',') + ' MHz'; }
    function render() {
      var rows = [], seen = {};
      function add(r) { var k = Math.round(r.khz * 10); if (seen[k] && r.src !== 'own') return; if (bandOf(r.khz) < 0) return; seen[k] = true; rows.push(r); }
      (data.own || []).forEach(function (r) { add({ khz: Number(r.freq || r.khz), call: r.call || '', info: r.info || r.qth || '', shift: r.shift != null ? String(r.shift) : '', tone: r.tone || '', km: r.km, mode: (r.mode || 'fm').toLowerCase(), src: 'own' }); });
      (data.aprs || []).forEach(function (r) { add({ khz: r.khz, call: r.call, info: r.comment, shift: r.offset, tone: r.tone, km: r.km, mode: 'fm', src: 'aprs' }); });
      markers.forEach(function (m) { add({ khz: m.freq, call: '', info: m.text, shift: '', tone: '', km: null, mode: (m.mode || 'fm').toLowerCase(), src: 'marker' }); });
      rows.sort(function (a, b) { return a.khz - b.khz; });
      var html = '', lastBand = -1, de = lang === 'de';
      rows.forEach(function (r) {
        var b = bandOf(r.khz);
        if (b !== lastBand) { lastBand = b; var m = BANDMETA[bi[b].name]; html += '<tr class="band"><td colspan="7">' + escapeHtml(m ? m[lang] + ' · ' + m.range : bi[b].name) + '</td></tr>'; }
        html += '<tr data-khz="' + r.khz + '" data-mode="' + r.mode + '"><td class="mhz">' + (r.khz / 1000).toFixed(4).replace('.', ',') + '</td><td>' + escapeHtml(r.call) + '</td><td class="info">' + escapeHtml(r.info) + '</td><td>' + shift(r.shift) + '</td><td>' + escapeHtml(r.tone) + '</td><td>' + (r.km != null ? r.km : '') + '</td><td class="src">' + (r.src === 'aprs' ? 'APRS' : r.src === 'own' ? (de ? 'Liste' : 'list') : (de ? 'Marker' : 'marker')) + '</td></tr>';
      });
      body.innerHTML = html || '<tr><td colspan="7">' + (de ? 'Noch keine Einträge.' : 'No entries yet.') + '</td></tr>';
    }
    function open_() { box.hidden = false; get('relais.json', function (d) { data.own = Array.isArray(d) ? d : []; render(); }); get('digi/relais.json', function (d) { data.aprs = d && d.relais || []; render(); }); render(); }
    $('flistbtn').onclick = open_;
    $('flistclose').onclick = function () { box.hidden = true; };
    box.addEventListener('click', function (e) { if (e.target === box) box.hidden = true; });
    body.addEventListener('click', function (e) { var tr = e.target.closest('tr[data-khz]'); if (!tr) return; tuneTo(Number(tr.dataset.khz), tr.dataset.mode); box.hidden = true; });
    document.addEventListener('keydown', function (e) { if (e.key === 'Escape') box.hidden = true; });
  }

  /* ================= Aufnahme (WAV, nutzt recStart/recStop aus core.js) ================= */
  var recTimer = null, recT0 = 0;
  function fmtMs(s) { s = Math.floor(s); return Math.floor(s / 60) + ':' + ('0' + s % 60).slice(-2); }
  function recLabel() { return '● ' + (lang === 'de' ? 'Aufnahme' : 'Record'); }
  function toggleRecord() {
    var b = $('recbtn'), out = $('reclink');
    if (recTimer) {
      clearInterval(recTimer); recTimer = null; b.classList.remove('on'); b.textContent = recLabel();
      var dur = (Date.now() - recT0) / 1000;
      try { recStop(); } catch (e) { toast(lang === 'de' ? 'Aufnahme fehlgeschlagen' : 'Recording failed'); return; }
      if (typeof recUrl === 'undefined' || !recUrl) return;
      var ts = new Date(recT0), p2 = function (n) { return ('0' + n).slice(-2); };
      var fname = String(window.STATION_NAME || 'crabSDR').replace(/[^A-Za-z0-9-]+/g, '_') + '_' + ts.getFullYear() + p2(ts.getMonth() + 1) + p2(ts.getDate()) + '-' + p2(ts.getHours()) + p2(ts.getMinutes()) + p2(ts.getSeconds()) + '_' + dialFreq().toFixed(1) + 'kHz_' + mode + '.wav';
      out.innerHTML = '<a href="' + recUrl + '" download="' + fname + '">⬇ ' + (lang === 'de' ? 'speichern' : 'save') + ' (' + fmtMs(dur) + ')</a>';
      // direkt herunterladen (das Bedienfeld mit dem Link ist auf dem Desktop ausgeblendet)
      var a = document.createElement('a'); a.href = recUrl; a.download = fname; document.body.appendChild(a); a.click(); a.remove();
      toast((lang === 'de' ? 'Aufnahme gespeichert: ' : 'Recording saved: ') + fname);
    } else {
      if (audioState() !== 'running') { toast(lang === 'de' ? 'Erst den Ton einschalten' : 'Start audio first'); return; }
      out.innerHTML = ''; recT0 = Date.now();
      try { recStart(); } catch (e) { toast(lang === 'de' ? 'Aufnahme nicht möglich' : 'Recording not possible'); return; }
      b.classList.add('on');
      recTimer = setInterval(function () {
        var s = (Date.now() - recT0) / 1000;
        b.textContent = '■ ' + fmtMs(s);
        if (s >= 1800) toggleRecord();   // nach 30 min automatisch beenden (Speicher im Browser)
      }, 500);
      b.textContent = '■ 0:00';
    }
  }

  /* ================= Tastatur (ersetzt keydown aus core.js) ================= */
  var chatShow = null;
  function initKeys() {
    var help = $('keyhelp');
    function showHelp(o) { help.hidden = !o; }
    $('keyhelpclose').onclick = function () { showHelp(false); };
    help.addEventListener('click', function (e) { if (e.target === help) showHelp(false); });
    window.onkeydown = function (e) {
      var t = e.target, tag = t && t.nodeName;
      if (e.key === 'Escape') { showHelp(false); return true; }
      if ((tag === 'INPUT' && (t.type === 'text' || t.type === 'search') && t.name !== 'frequency') || tag === 'TEXTAREA' || tag === 'SELECT') return true;
      if (tag === 'INPUT' && t.name === 'frequency' && !/^Arrow/.test(e.key)) return true;   // im Frequenzfeld nur Pfeile abfangen
      if (e.ctrlKey || e.metaKey || e.altKey) { if (/^Arrow(Left|Right)$/.test(e.key)) { freqStep(e.key === 'ArrowLeft' ? -3 : 3); e.preventDefault(); } return true; }
      var st = e.shiftKey ? 2 : 1, k = e.key, hit = true;
      if (k === 'ArrowLeft' || k === 'j' || k === 'J') freqStep(-st);
      else if (k === 'ArrowRight' || k === 'k' || k === 'K') freqStep(st);
      else if (k === 'ArrowUp' || k === 'ArrowDown') {   // FM/AM mit Raster: ein Kanal wie ← →; SSB/CW: nächster voller kHz (Umschalt 10 kHz)
        var up = k === 'ArrowUp';
        if (snap > 0 && (mode === 'FM' || mode === 'AM')) freqStep(up ? st : -st); else freqSnap(up ? 1 : -1, e.shiftKey ? 10 : 1);
      }
      else if (/^[1-9]$/.test(k)) { var n = Number(k) - 1; if (n < nbands && !bandOff(bandinfo[n].name)) gotoBand(n); }
      else if (k === 'b' || k === 'B') { for (var i = 1; i <= nbands; i++) { var nb = (band + (e.shiftKey ? -i : i) + nbands * 2) % nbands; if (!bandOff(bandinfo[nb].name)) { gotoBand(nb); break; } } }
      else if ('fFaAuUlLcC'.indexOf(k) >= 0) { var md = { f: 'fm', a: 'am', u: 'usb', l: 'lsb', c: 'cw' }[k.toLowerCase()], ff = MODEFILTER[md]; setMode(md, ff[0], ff[1]); }
      else if (k === '+') setZoom(0);
      else if (k === '-') setZoom(1);
      else if (k === 'z' || k === 'Z') setZoom(e.shiftKey ? 2 : 4);
      else if (k === 'm' || k === 'M') { var mc = $('mutecheckbox'); if (mc) mc.click(); else try { crabAudio.mute(); } catch (x) {} toast(lang === 'de' ? 'Stumm umgeschaltet' : 'Mute toggled'); }
      else if (k === 'g' || k === 'G') { var fi = document.freqform.frequency; fi.value = ''; fi.focus(); }
      else if (k === 'r' || k === 'R') toggleRecord();
      else if (k === 't' || k === 'T') { if (chatShow) chatShow(true); setTimeout(function () { $('chatform').chat.focus(); }, 60); }
      else if (k === 'p' || k === 'P') $('flistbtn').click();
      else if (k === '?') showHelp(help.hidden);
      else hit = false;
      if (hit) { e.preventDefault(); return false; }
      return true;
    };
  }

  /* ================= Wasserfall läuft an: erst schnell füllen, dann Wunschtempo ================= */
  function warmWaterfall() {
    if (phone || typeof setWfSpeed !== 'function') return;
    var tip = document.createElement('div'); tip.className = 'wfwarm';
    tip.textContent = lang === 'de' ? 'Wasserfall läuft an …' : 'waterfall starting …';
    wrap.appendChild(tip);
    var want = Number($('wfspeed').value) || 2;
    function go() { try { setWfSpeed(1); } catch (e) {} }
    if (typeof wfWaiting !== 'undefined' && wfWaiting > 0) setTimeout(go, 1500); else go();
    setTimeout(function () { try { setWfSpeed(want); } catch (e) {} tip.classList.add('gone'); setTimeout(function () { tip.remove(); }, 1000); }, 25000);
  }

  /* ================= Keine Geister-Hörer =================
     Chrome friert Hintergrund-Tabs und Seiten im Zurück-Cache ein, lässt deren WebSockets aber offen:
     Der Server zählt sie weiter als Hörer (Pegel steht still), bis zu einer Stunde lang.
     Deshalb Streams selbst schließen beim Verlassen/Einfrieren und nach 2 min verborgen ohne Ton;
     kommt die Seite zurück, neu laden – über den Deep-Link, damit Band/Frequenz/Modus bleiben. */
  var streamsDropped = false, hiddenTimer = null;
  function dropStreams() {
    if (streamsDropped) return;
    streamsDropped = true;
    (window.__gws || []).forEach(function (w) { try { w.close(); } catch (e) {} });
  }
  function revive() { if (streamsDropped) { streamsDropped = false; location.replace(tuneLink()); } }
  function guardGhosts() {
    window.addEventListener('pagehide', dropStreams);
    document.addEventListener('freeze', dropStreams);
    window.addEventListener('pageshow', function (e) { if (e.persisted) revive(); });
    document.addEventListener('resume', revive);
    document.addEventListener('visibilitychange', function () {
      clearTimeout(hiddenTimer);
      if (!document.hidden) revive();
      else if (audioState() !== 'running') hiddenTimer = setTimeout(dropStreams, 120000);
    });
  }

  /* ================= Statuszeile ================= */
  var lastStatus = null;   // status.json kommt jede Minute; die CPU-Last liefert der Server alle ~10 s -> Zeile öfter neu bauen
  function loadStatus() {
    var x = new XMLHttpRequest();
    x.open('GET', 'status.json?' + Date.now()); x.timeout = 5000;
    x.onload = function () { try { lastStatus = JSON.parse(x.responseText); renderStatus(lastStatus); } catch (e) {} };
    x.send();
  }
  function fmtDuration(sec) {   // 200000 -> „2 Tage, 7 Std." / „2 days, 7 h"
    var de = lang === 'de', d = Math.floor(sec / 86400), h = Math.floor(sec % 86400 / 3600), m = Math.floor(sec % 3600 / 60);
    if (d) return d + (de ? (d === 1 ? ' Tag' : ' Tagen') : (d === 1 ? ' day' : ' days')) + (h ? ', ' + h + (de ? ' Std.' : ' h') : '');
    if (h) return h + (de ? ' Std. ' : ' h ') + m + ' min';
    return m + ' min';
  }
  function renderStatus(s) {
    var de = lang === 'de', parts = [];
    if (s.temp) parts.push('<span class="' + (s.temp > 75 ? 'bad' : s.temp > 65 ? 'warn' : 'ok') + '">CPU ' + s.temp + ' °C</span>');
    if (s.up_s != null || s.uptime) parts.push('<span>' + (de ? 'läuft seit ' : 'up ') + (s.up_s != null ? fmtDuration(s.up_s) : s.uptime) + '</span>');
    var cpu = serverStats(); if (cpu !== null) parts.push('<span title="' + (de ? 'Gesamtlast des Rechners (alle Kerne)' : 'total CPU load (all cores)') + '">' + (de ? 'Last ' : 'load ') + Math.round(cpu) + ' %</span>');
    if (s.ts) parts.push('<span>' + (de ? 'Stand ' : 'as of ') + new Date(s.ts * 1000).toLocaleTimeString(de ? 'de-DE' : 'en-GB', { hour: '2-digit', minute: '2-digit' }) + '</span>');
    $('status').innerHTML = parts.join(' · ');
  }

  /* ================= Handy / Desktop ================= */
  function applyLayout() {
    var p = isPhone();
    if (p === phone) { fit(); if (phone) setWfHeight(phoneWf()); return; }
    if (layoutInit) { location.reload(); return; }   // Wechsel Handy ↔ Desktop nach dem Start: sauber neu aufbauen
    phone = p;
    document.body.classList.toggle('phone', phone);
    if (phone) {
      initPhoneSheet();
      applyView();
      $('wfsize').value = '200'; setWfHeight(phoneWf());
      setTimeout(function () { zoomToFreq(band, PHONE_ZOOM, freq); fit(); }, 400);
    } else {
      applyView();
      $('wfsize').value = 'auto';
      setTimeout(function () { setZoom(4); fit(); fillHeight(); }, 400);
    }
    fit();
  }

  /* ================= Chat (eigener Dienst /logbuch/api, Langabfrage) ================= */
  function initChat() {
    var drawer = $('chatdrawer'), list = $('chatlist'), btn = $('chatbtn'), badge = $('chatbadge'), lastId = 0, unread = 0, open = false, restoring = false;
    var STILL = /[?&]still\b/.test(location.search);
    function fmtT(t) { var d = new Date(t * 1000); return d.toLocaleTimeString('de-DE', { hour: '2-digit', minute: '2-digit' }); }
    // „145,500", „145.5125 USB", „10368,100 cw", „145500" -> Link, der Band, Frequenz und (falls genannt) Betriebsart einstellt
    function linkFreqs(text) {
      return escapeHtml(text).replace(/\b(\d{3,5})[.,](\d{1,4})\b(\s*(?:MHz)?\s*(FM|AM|USB|LSB|CW)\b)?|\b(\d{6})\b/gi, function (m, a, b, tail, md, c) {
        var kHz = c ? Number(c) : Math.round((Number(a) + Number('0.' + b)) * 1e6) / 1000;
        if (bandOf(kHz) < 0) return m;   // nur Frequenzen, die wir auch empfangen
        var mm = md ? md.toLowerCase() : '';
        return '<a class="tune" href="?tune=' + kHz + (mm || 'fm') + '" data-khz="' + kHz + '" data-mode="' + mm + '">' + m + '</a>';
      });
    }
    list.addEventListener('click', function (ev) {
      var a = ev.target.closest && ev.target.closest('a.tune'); if (!a) return;
      ev.preventDefault(); tuneTo(Number(a.dataset.khz), a.dataset.mode);
    });
    function api(method, path, data, cb) {
      var x = new XMLHttpRequest(); x.open(method, 'logbuch/api/' + path);
      if (data) x.setRequestHeader('Content-Type', 'application/json');
      x.onload = function () { var r = null; try { r = JSON.parse(x.responseText); } catch (e) {} cb(x.status, r); };
      x.onerror = function () { cb(0, null); };
      x.send(data ? JSON.stringify(data) : null);
    }
    function setBadge() { badge.textContent = unread > 99 ? '99+' : String(unread); badge.hidden = !unread || open; }
    function add(r) {
      var d = document.createElement('div');
      d.innerHTML = '<span class="t">' + fmtT(r.t) + '</span> <b>' + escapeHtml(dispName(r.name)) + ':</b> ' + linkFreqs(r.msg);
      list.appendChild(d); list.scrollTop = list.scrollHeight;
    }
    function poll() {
      api('GET', 'chat?since=' + lastId + (lastId && !STILL ? '&wait=1' : '') + '&_=' + Date.now(), null, function (st, r) {
        if (st === 200 && r) {
          if (lastId === 0) list.innerHTML = r.lines.length ? '' : '<span class="muted">' + (lang === 'de' ? 'Noch keine Nachrichten. Schreib die erste.' : 'No messages yet. Write the first one.') + '</span>';
          else if (r.lines.length && list.firstChild && list.firstChild.className === 'muted') list.innerHTML = '';
          r.lines.forEach(add);
          if (lastId && r.lines.length && !open) { unread += r.lines.length; setBadge(); }
          if (r.id) lastId = r.id;
          setTimeout(poll, STILL ? 5000 : 300);
        } else { if (lastId === 0) list.innerHTML = '<span class="muted">Chat nicht erreichbar.</span>'; setTimeout(poll, 5000); }
      });
    }
    function show(o) {
      open = o; drawer.hidden = !o; btn.classList.toggle('open', o); document.body.classList.toggle('chat-open', o);
      if (o) { unread = 0; list.scrollTop = list.scrollHeight; if (!phone && !restoring) setTimeout(function () { drawer.querySelector('input').focus({ preventScroll: true }); }, 50); }
      setBadge(); if (!restoring) try { localStorage.setItem('crab_chat', o ? '1' : '0'); } catch (e) {}
    }
    btn.onclick = function () { show(!open); };
    chatShow = show;
    $('chatclose').onclick = function () { show(false); };
    // Ohne Namen erscheint beim Schreiben ein Namensfeld im Chat (gleicher Name wie oben im Kopf)
    var chatName = $('chatform').chatname;
    function syncNameField() { chatName.hidden = !!$('myname').value.trim(); }
    syncNameField(); $('myname').addEventListener('input', syncNameField);
    $('chatform').chat.addEventListener('focus', syncNameField);
    $('chathere').onclick = function () {
      var inp = $('chatform').chat, f = (dialFreq() / 1000).toFixed(dialFreq() % 1 ? 4 : 3).replace('.', ',');
      var bit = (lang === 'de' ? 'höre gerade ' : 'listening on ') + f + ' ' + mode;
      inp.value = inp.value ? inp.value.replace(/\s*$/, ' ') + bit : bit; inp.focus();
    };
    $('chatform').onsubmit = function () {
      var inp = this.chat, msg = inp.value.trim(); if (!msg) return false;
      if (!$('myname').value.trim() && chatName.value.trim()) {
        $('myname').value = chatName.value.trim().slice(0, 20); $('myname').onchange(); syncNameField();
      } else if (!$('myname').value.trim()) {
        chatName.hidden = false; chatName.focus(); toast(lang === 'de' ? 'Wie heißt du? Name oder Rufzeichen – dann Senden.' : 'Your name or call sign, then Send.');
        return false;
      }
      var nm = $('myname').value.trim().slice(0, 20) || 'Hörer';
      api('POST', 'chat', { name: nm, msg: msg }, function (st, r) { if (st !== 200) toast((r && r.error) || 'Senden fehlgeschlagen'); });
      inp.value = ''; return false;
    };
    // Andocken statt schweben: breit = Spalte rechts, mittel = Streifen unter dem Wasserfall, Handy = im Fluss darunter.
    // Der Griff an der Innenkante stellt Breite bzw. Höhe ein (gemerkt je Richtung).
    var root = document.documentElement, grip = drawer.querySelector('.chatgrip');
    function dock() { return isPhone() ? 'flow' : window.innerWidth >= CHAT_SIDE_MIN ? 'side' : 'bottom'; }
    function applyDock() {
      var d = dock(), b = document.body.classList;
      b.toggle('chat-side', d === 'side'); b.toggle('chat-bottom', d === 'bottom');
      var w = Number(loadPref('crab_chatw')) || 340, h = Number(loadPref('crab_chath')) || 200;
      var maxH = Math.round(window.innerHeight * 0.7);
      if (d === 'bottom') {   // Wasserfall behält mindestens 200 px
        var used = 0; ['.top', '.bands', '.panel', '.foot'].forEach(function (q) { var el = document.querySelector(q); if (el) used += el.offsetHeight; });
        maxH = window.innerHeight - used - 200;
      }
      root.style.setProperty('--chat-w', Math.max(260, Math.min(w, Math.round(window.innerWidth * 0.45))) + 'px');
      root.style.setProperty('--chat-h', Math.max(120, Math.min(h, maxH)) + 'px');
    }
    function loadPref(k) { try { return localStorage.getItem(k); } catch (e) { return null; } }
    grip.addEventListener('pointerdown', function (ev) {
      var d = dock(), r = drawer.getBoundingClientRect(), sx = ev.clientX, sy = ev.clientY, val = d === 'side' ? r.width : r.height;
      grip.setPointerCapture(ev.pointerId); drawer.classList.add('resizing');
      function mv(e) {
        val = d === 'side' ? r.width - (e.clientX - sx) : d === 'bottom' ? r.height - (e.clientY - sy) : r.height + (e.clientY - sy);
        try { localStorage.setItem(d === 'side' ? 'crab_chatw' : 'crab_chath', String(Math.round(val))); } catch (e2) {}
        applyDock();
      }
      function up() { grip.removeEventListener('pointermove', mv); grip.removeEventListener('pointerup', up); grip.removeEventListener('pointercancel', up); drawer.classList.remove('resizing'); }
      grip.addEventListener('pointermove', mv); grip.addEventListener('pointerup', up); grip.addEventListener('pointercancel', up);
      ev.preventDefault();
    });
    window.addEventListener('resize', applyDock);
    applyDock();
    // Chat soll benutzt werden: auf breiten Bildschirmen standardmäßig offen (verdeckt ja nichts mehr), sonst wie zuletzt.
    var pref = loadPref('crab_chat');
    if (/[?&]chat=1\b/.test(location.search) || pref === '1' || (pref === null && dock() === 'side')) { restoring = true; show(true); restoring = false; }
    poll();
  }

  /* ================= Start ================= */
  window.addEventListener('load', function () {
    rx = $('rx'); wrap = $('rxwrap'); scalesBox = $('scales'); lmarks = $('lmarks');
    rx.style.transformOrigin = '0 0';
    phone = isPhone(); document.body.classList.toggle('phone', phone); if (phone) initPhoneSheet();
    createCookie('view', targetView(), 3652);
    createCookie('usejava', 'nn', 3652);
    buildBandBar();                        // muss VOR crabStart() laufen: base.js greift auf freqform.group0 zu
    overrideCore();
    try { crabStart(); }
    catch (e) { console.error('WebSDR-Initialisierung fehlgeschlagen:', e); $('status').textContent = 'Fehler beim Start: ' + e; }
    document.body.classList.toggle('allbands', Number(view) === Views.allbands);
    $('allbandschk').checked = Number(view) === Views.allbands;
    $('allbandschk').onchange = function () { viewPref = this.checked ? 'all' : 'one'; try { localStorage.setItem('crab_view', viewPref); } catch (e) {} applyView(); };
    $('wfsize').onchange = function () { if (this.value === 'auto') fillHeight(); else setWfHeight(Number(this.value)); };
    if (phone) { $('wfsize').value = '200'; setWfHeight(phoneWf()); setTimeout(function () { zoomToFreq(band, PHONE_ZOOM, freq); fit(); }, 700); }
    else { $('wfsize').value = 'auto'; setTimeout(function () { fit(); fillHeight(); }, 700); }
    var x = new XMLHttpRequest();
    x.open('GET', 'presets.json?' + Date.now());
    x.onload = function () { try { presets = JSON.parse(x.responseText); } catch (e) { presets = []; } buildPresets(); };
    x.send();
    // Raster
    var sel = $('snapsel'); sel.value = String(snap);
    if (sel.value !== String(snap)) { sel.value = '12.5'; snap = 12.5; }
    sel.onchange = function () { snap = Number(this.value); snapUser = snap; try { localStorage.setItem('crab_snap', String(snap)); } catch (e) {} if (snap > 0) setFreq(freq); this.blur(); };
    loadUi(); initName(); initAdvanced(); loadMarkers(); initPwa(); initChat();
    $('linkbtn').onclick = copyLink;
    // Bereiche: eigene segments.json der Station, sonst der eingebaute Bandplan
    (function () { var sx = new XMLHttpRequest(); sx.open('GET', 'segments.json?' + Date.now());
      sx.onload = function () { if (sx.status === 200) { try { var j = JSON.parse(sx.responseText); if (Array.isArray(j) && j.length) segments = j; } catch (e) {} } buildSegments(); };
      sx.onerror = function () { buildSegments(); }; sx.send(); })();
    $('segsel').onchange = function () { var list = bandSegments(band), s = list[Number(this.value)]; this.value = ''; this.blur(); if (s) zoomSegment(s); };
    // Startwerte: FM-Bandbreite 12 kHz statt 16 kHz; auf dem Handy höherer Wasserfall (wird auf Bildschirmbreite skaliert)
    setTimeout(function () {
      try { if (typeof mode !== 'undefined' && mode === 'FM' && Math.abs((hi - lo) - 16) < 0.01) setMode('fm', -6, 6); } catch (e) {}
      try { if (phone && typeof setWfHeight === 'function') { setWfHeight(phoneWf()); var ws = $('wfsize'); if (ws) ws.value = '200'; } } catch (e) {}
    }, 1500);
    var _douu = window.showListeners; window.showListeners = function () { _douu(); renderListeners(); };
    // Sobald alle Wasserfälle laufen (auch nach setView): Höhe „Bildschirm füllen" einmal sauber setzen
    var _aws = window.wfAllStarted; window.wfAllStarted = function () { _aws(); setTimeout(function () { fit(); fillHeight(); }, 50); };
    if (typeof window.chatnewline === 'function') { var _cn = window.chatnewline; window.chatnewline = function () { var a = Array.prototype.slice.call(arguments).map(function (x) { return typeof x === 'string' ? x.replace(/(?:\d{1,3}(?:\.\d{1,3}){3}|(?:[0-9a-f]{1,4}:){2,7}[0-9a-f]{0,4})/gi, lang === 'en' ? 'listener' : 'Hörer') : x; }); return _cn.apply(this, a); }; }
    // Angezeigt wird die eingestellte Breite glatt (nicht die −6-dB-Breite).
    var _updbw = window.updateBw; window.updateBw = function () { _updbw(); var w = hi - lo; $('numericalbandwidth6').textContent = w >= 10 ? String(Math.round(w)) : String(Math.round(w * 100) / 100); };
    applyLang();
    // Hell/Dunkel: System -> dunkel -> hell -> System …
    $('themebtn').onclick = function () {
      var h = document.documentElement, cur = h.getAttribute('data-theme');
      var next = !cur ? 'dark' : cur === 'dark' ? 'light' : null;
      if (next) { h.setAttribute('data-theme', next); h.classList.remove('sysdark'); try { localStorage.setItem('crab_theme', next); } catch (e) {} }
      else { h.removeAttribute('data-theme'); h.classList.toggle('sysdark', window.matchMedia('(prefers-color-scheme: dark)').matches); try { localStorage.removeItem('crab_theme'); } catch (e) {} }
      this.textContent = !next ? '◐' : next === 'dark' ? '☾' : '☀';
    };
    (function () { var cur = document.documentElement.getAttribute('data-theme'); $('themebtn').textContent = !cur ? '◐' : cur === 'dark' ? '☾' : '☀'; })();
    $('langbtn').onclick = function () { lang = lang === 'de' ? 'en' : 'de'; try { localStorage.setItem('crab_lang', lang); } catch (e) {} applyLang(); buildPresets(); refresh(); };
    $('audiobtn').onclick = unlockAudio;
    $('maudio').onclick = unlockAudio;
    ['click', 'keydown', 'touchstart'].forEach(function (ev) { document.addEventListener(ev, unlockAudio, true); });
    // Handy-Leiste
    $('mmode').onchange = function () { var f = MODEFILTER[this.value]; setMode(this.value, f[0], f[1]); };
    $('mfreq').onclick = function () { var inp = document.freqform.frequency; openSheet(true); setTimeout(function () { inp.scrollIntoView({ block: 'center' }); inp.focus(); inp.select(); }, 250); };
    // Eigene Skala: Tippen und Ziehen
    scalesBox.addEventListener('pointerdown', function (ev) { scalesBox._down = true; scalePointer(ev); });
    scalesBox.addEventListener('pointermove', function (ev) { if (scalesBox._down) scalePointer(ev); });
    ['pointerup', 'pointercancel', 'pointerleave'].forEach(function (n) { scalesBox.addEventListener(n, function () { scalesBox._down = false; }); });
    var rt; window.addEventListener('resize', function () { clearTimeout(rt); rt = setTimeout(applyLayout, 150); });
    // Handy: Kurzleiste nur zeigen, solange das volle Bedienfeld nicht im Bild ist
    if (window.IntersectionObserver) new IntersectionObserver(function (es) { document.body.classList.toggle('panel-visible', es[0].isIntersecting); }, { threshold: 0.15 }).observe(document.querySelector('.strip'));
    if (window.ResizeObserver) { var ro = new ResizeObserver(function () { fit(); }); ro.observe(rx); ro.observe($('main')); ro.observe(document.querySelector('.panel')); }   // main: Leiste/Fuß/Fonts ändern die freie Höhe nachträglich
    fit(); layoutInit = true;
    setInterval(refresh, 250);
    setInterval(meter, 100);
    setInterval(updateAudioBtn, 1000);
    loadStatus(); setInterval(loadStatus, 60000);
    loadGains(); setInterval(loadGains, 60000);
    setInterval(function () { if (lastStatus) renderStatus(lastStatus); }, 10000);
    guardGhosts();
    $('recbtn').onclick = toggleRecord;
    initKeys(); warmWaterfall(); initFreqList();
    refresh();
  });
})();
