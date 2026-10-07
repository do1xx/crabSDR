/* core.js — Kern der crabSDR-Oberfläche.
   Zustand, Wasserfälle, Ton und Abstimmung; stellt die globalen Variablen und Funktionen bereit, die ui.js und
   index.html nutzen, und spricht das crabSDR-Protokoll (WebSocket je Band: JSON-Steuerung, Opus-Ton 0x82,
   Wasserfall-Zeilen 0x84, Pegel-Meldungen).
   Frequenzen sind hier kHz; der Server rechnet in Hz. */

// zstd-Dekoder (fzstd, MIT) synchron nachladen – wie index.html es mit bandinfo.js macht
document.write('<script src="zstd.js"><\/script>');
// Eigene Stile nur für Elemente, die der Kern selbst erzeugt (Marker unter dem Wasserfall)
document.write('<style>.dxm{position:absolute;top:30px;max-width:63px;overflow:hidden;font:10px/15px system-ui,sans-serif;color:#eee;background:#3a3a3a;border-radius:2px;cursor:pointer;white-space:nowrap;padding:0 4px}.dxm:hover{background:#555}.dxl{position:absolute;top:-14px;width:1px;height:44px;background:#556}body.look-crab .dxm{background:transparent;color:#8ab;top:4px;padding:0 2px;font-size:9px;line-height:12px}body.look-crab .dxm:hover{color:#fff}body.look-crab .dxl{top:0;height:4px;background:#3a5}</style>');

/* ===== Zustand (globale Namen, die ui.js nutzt) ===== */
var Views = { allbands: 0, oneband: 2 };
var view = 2;
var bi = [];                       // je Band: name, centerfreq, samplerate (kHz, ganzes Band), zoom, start, effcenterfreq, effsamplerate
var band = 0, freq = 0, mode = 'FM', lo = -8, hi = 8;
var nWaterfalls = 1, wfHeight = 200, wfSlow = 2, wfMode = 1, wfWaiting = 0;
var centerfreq = 0, khzPerPx = 2;      // aktuelles Band, sichtbarer Ausschnitt
var marks = [], lsNames = [], lsBands = [], lsFreqs = [];
var listenerColours = ['#f87171', '#fbbf24', '#34d399', '#60a5fa', '#c084fc', '#f472b6', '#fb923c', '#a3e635'];
var memSlots = [], recUrl = null, freqTextLock = false;
var dragStartVal = 0, dragStartX = 0, dragging = 0;
var audioStarted = false, allLoaded = false, wfStub = null, markTimer = null;
var isTouch = ('ontouchstart' in window);
var scaleEl = null, scaleEls = [], scaleHeight = 14, smeterEl = null, passbandEl = null, carrierEl = null, edgeLoEl = null, edgeHiEl = null;
var keysOn = true, dx = [], uu = [], hideMarks = false, wfModeNames = ['Spektrum', 'Wasserfall', 'schwach', 'stark'];

var _crab = {
  token: null, bands: [], fft: 4096, px: 1024, wsBase: (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + location.pathname.replace(/[^\/]*$/, ''),   // Basis-Pfad, damit /beta/ hinter nginx funktioniert
  audio: { gain: null, node: null, decoder: null, ts: 0, muted: false, volume: 1, pcm: typeof AudioDecoder !== 'function', rec: null },   // ohne WebCodecs (z. B. http:// im LAN) gleich PCM anfordern
  level: -200, floor: -200, sq: true, squelchOn: false, sqMargin: 6, listenersTimer: null, palette: null,
  dragRxX: 0, dragEdge: 0, started: false, sel: [0], bandsOff: [], features: {}, banner: '', links: {},
};

/* ===== Cookies ===== */
function readCookie(name) { var m = document.cookie.match('(?:^|; )' + name + '=([^;]*)'); return m ? m[1] : null; }
function createCookie(name, value, days) {
  var exp = days ? '; max-age=' + (days * 86400) : '';
  document.cookie = name + '=' + value + exp + '; path=/; SameSite=Lax';
}
/* Platzhalter je Browser, wenn kein Name gesetzt ist: „Hörer-47“ statt überall nur „Hörer“ – so bleiben Hörer in
   Chat und Hörerliste unterscheidbar; die Nummer merkt sich der Browser (localStorage) */
/* Sitzungskennung je Browser (zufällig, nur lokal gespeichert): der Server zählt damit Personen statt Verbindungen */
function _crabSessionId() {
  var s = null; try { s = localStorage.getItem('crab_sid'); } catch (e) {}
  if (!s) { s = ''; for (var i = 0; i < 16; i++) s += 'abcdefghijklmnopqrstuvwxyz0123456789'[Math.floor(Math.random() * 36)]; try { localStorage.setItem('crab_sid', s); } catch (e) {} }
  return s;
}
function _crabGuestName() {
  var g = null; try { g = localStorage.getItem('crab_guest'); } catch (e) {}
  if (!g) { g = 'Hörer-' + (10 + Math.floor(Math.random() * 90)); try { localStorage.setItem('crab_guest', g); } catch (e) {} }
  return g;
}
function saveName() {
  var v = (document.usernameform && document.usernameform.username.value || '').trim().slice(0, 20) || _crabGuestName();
  if (document.usernameform) document.usernameform.username.value = v;
  createCookie('username', encodeURIComponent(v), 3652);
  _crabSendAll({ type: 'set_name', name: v });
}

/* ===== Frequenz-Hilfen ===== */
function isCw() { return mode === 'CW'; }
function dialFreq() { return freq + (isCw() ? (hi + lo) / 2 : 0); }
function _crabTextFreq() {
  try { if (!freqTextLock) document.freqform.frequency.value = dialFreq().toFixed(2); } catch (e) {}
  // Tab-Titel = Frequenz und Betriebsart, damit man bei vielen Tabs sieht, welcher was hört
  try { if (_crab.started) document.title = (dialFreq() / 1000).toFixed(4) + ' MHz ' + mode + ' · ' + (window.STATION_NAME || 'crabSDR'); } catch (e) {}
}
function _crabBandRange(b) { var e = bi[b]; return { lo: e.centerfreq - e.samplerate / 2, hi: e.centerfreq + e.samplerate / 2 }; }

/* Tune-Nachricht für den Server: Modus, Mittenfrequenz/Träger und Bandbreite in Hz */
function _crabTuneMsg() {
  var m = mode.toLowerCase(), f, bw;
  if (m === 'usb' || m === 'lsb') { f = freq; bw = Math.max(Math.abs(hi), Math.abs(lo)); }
  else { f = freq + (lo + hi) / 2; bw = hi - lo; }
  var msg = { type: 'tune', freq: Math.round(f * 1000), mode: m, bandwidth: Math.max(100, Math.round(bw * 1000)) };
  if (m === 'usb' || m === 'lsb') msg.lo = Math.round(Math.min(Math.abs(hi), Math.abs(lo)) * 1000);   // untere Kante (300 Hz)
  return msg;
}
function _crabRetune() {
  var B = _crab.bands[band];
  if (_crab.decAudio) { _crab.decAudio = null; _crabStatus(''); _crabMarkDec(); }   // Nutzer stimmt ab: zurück vom Decoder-Ton zum Kanal
  if (B && B.ws && B.ws.readyState === 1) B.ws.send(JSON.stringify(_crabTuneMsg()));
  drawPassband();
}

function setFreq(f) {
  var r = _crabBandRange(band);
  f = Math.max(r.lo - lo, Math.min(r.hi - hi, Number(f) || 0));
  freq = f;
  bi[band].lastfreq = f; bi[band].vfo = dialFreq();
  _crabTextFreq();
  _crabRetune();
  _crabOwnMarker();
}
function setFreqText(s) {
  var v = parseFloat(String(s).replace(',', '.'));
  if (!isFinite(v)) return;
  var off = isCw() ? (hi + lo) / 2 : 0;
  freqTextLock = true;
  try { setFreq(v - off); } finally { freqTextLock = false; }
  try { document.freqform.frequency.value = String(s); } catch (e) {}
}
function setFreqTextLater(s) {
  var v = parseFloat(String(s).replace(',', '.'));
  if (!isFinite(v) || v < 1000) return;
  var r = _crabBandRange(band);
  if (v < r.lo || v > r.hi) return;
  freqTextLock = true;
  try { setFreq(v - (isCw() ? (hi + lo) / 2 : 0)); } finally { freqTextLock = false; }
}
function setFreqMark(b, f, m) {   // Marker-Klick: Band, Betriebsart, Frequenz
  if (m === undefined && f === undefined) { setFreq(b); return; }
  if (b !== band) setBand(b);
  var md = String(m || 'fm').toLowerCase(), ff = _crabMODEFILTER[md] || _crabMODEFILTER.fm;
  setMode(md, ff[0], ff[1]); setFreq(Number(f) - (isCw() ? (hi + lo) / 2 : 0));
}
/* ← →: feiner Schritt – SSB/CW 100 Hz, FM/AM 1 kHz; Umschalt ×5, Strg/Alt ×25 */
function freqStep(st) {
  var fine = (mode === 'USB' || mode === 'LSB' || mode === 'CW') ? 0.1 : 1;
  var n = fine * (Math.abs(st) === 1 ? 1 : Math.abs(st) === 2 ? 5 : 25);
  setFreq(Math.round((freq + (st > 0 ? n : -n)) * 1000) / 1000);
}
/* ↑ ↓: auf den nächsten vollen Schritt springen (1 kHz; mit Umschalt 10 kHz), z. B. 144 802,0 → 144 803,0 bzw. 144 810,0 */
function freqSnap(dir, stepKhz) {
  var k = stepKhz || 1, cur = freq / k, next = dir > 0 ? Math.floor(cur + 1e-6) + 1 : Math.ceil(cur - 1e-6) - 1;
  (window.setFreqExact || setFreq)(Math.round(next * k * 1000) / 1000);   // am Kanalraster vorbei
}
function setMode(m, l, h) {
  mode = String(m).toUpperCase(); lo = Number(l); hi = Number(h);
  if (_crab.started) _crabSqForMode();
  _crabTextFreq();
  updateBw();
}
function updateBw() {
  if (hi - lo < 0.1) { var c = (hi + lo) / 2; lo = c - 0.05; hi = c + 0.05; }
  var el = document.getElementById('numericalbandwidth6');
  if (el) el.textContent = (hi - lo).toFixed(2);
  _crabRetune();
}

/* ===== Bänder, Ansicht, Zoom ===== */
/* Desktop zeigt die Bänder, die der Hörer oben angewählt hat (_crab.sel, sortiert), das Handy immer nur das aktive. */
function id2band(i) { return Number(view) === Views.allbands ? _crab.sel[i] : band; }
function band2id(b) { return Number(view) === Views.allbands ? _crab.sel.indexOf(b) : (b === band ? 0 : -1); }
function freq2x(fabs, b) { var e = bi[b]; return (fabs - (e.effcenterfreq - e.effsamplerate / 2)) / (e.effsamplerate / 1024); }
/* Zoom je Band: die FFT-Größe hängt von der Abtastrate ab (4096 bei 2 MS/s, 16384 bei 8 MS/s). Der Server liefert
   immer 1024 Pixel: bis ein Pixel ein Bin ist aus dem Gesamtspektrum, darüber (fünf weitere Stufen) aus einem eigenen
   Zoom-Spektrum des Ausschnitts – die Auflösung steigt mit jeder Stufe, der Browser streckt nichts mehr. */
function _crabSrvMax(e) { return _crabMaxZoom(e); }
function _crabMaxZoom(e) { return Math.max(0, Math.round(Math.log2((e.fft || _crab.fft) / _crab.px))) + 5; }
function _crabBpp(zoom, e) { return ((e.fft || _crab.fft) / _crab.px) / Math.pow(2, zoom); }   // Bins je Pixel, < 1 im Zoom-Spektrum
function _crabScale() { return 1; }
function _crabGeom(b) {
  var e = bi[b], bpp = _crabBpp(e.zoom, e), sc = _crabScale(e.zoom, e), binkhz = e.samplerate / e.fft;
  e.effsamplerate = 1024 * bpp * binkhz / sc;
  e.effcenterfreq = e.centerfreq - e.samplerate / 2 + (e.start + 512 * bpp / sc) * binkhz;
  // Server-Ausschnitt (Stufe ≤ 2, 1024 Server-Pixel) so legen, dass der sichtbare Teil darin liegt
  var sstart = e.start - Math.round((1024 - 1024 / sc) / 2 * bpp);
  e.sstart = Math.max(0, Math.min(e.fft - 1024 * bpp, sstart)); e.szoom = Math.min(e.zoom, _crabSrvMax(e));
  if (b === band) { centerfreq = e.effcenterfreq; khzPerPx = e.effsamplerate / 1024; }
}
function zoomToFreq(b, zoom, f) {
  var e = bi[b]; zoom = Math.max(0, Math.min(_crabMaxZoom(e), Number(zoom) || 0));
  var bpp = _crabBpp(zoom, e), sc = _crabScale(zoom, e), binkhz = e.samplerate / e.fft;
  var startBin = Math.round((f - (e.centerfreq - e.samplerate / 2)) / binkhz - 512 * bpp / sc);
  e.zoom = zoom; e.start = Math.max(0, Math.min(e.fft - 1024 * bpp / sc, startBin));
  _crabGeom(b);
  var B = _crab.bands[b]; if (B) { B.prev = null; B.liveSinceHist = 0; B.clearOnNext = true; _crabSendWf(b); }   // alte Zeilen gehören zu einem anderen Ausschnitt
  if (b === band) drawPassband();
  try { loadMarks(b); } catch (e) { showMarks(b); }
}
function setZoom(n) {
  var e = bi[band], z = e.zoom;
  if (n === 0) z = Math.min(_crabMaxZoom(e), z + 1);
  else if (n === 1) z = Math.max(0, z - 1);
  else if (n === 2) z = _crabMaxZoom(e);
  else z = 0;
  // um das gehörte Signal zoomen (Mitte des Durchlassbereichs), nicht um die Mitte des Ausschnitts
  var r = _crabBandRange(band), f = freq + (lo + hi) / 2;
  zoomToFreq(band, z, (freq && f >= r.lo && f <= r.hi) ? f : e.effcenterfreq);
}
function wheelStep(e) { var d = e.deltaY || -e.wheelDelta || 0; if (d) freqStep(d > 0 ? -1 : 1); }
function setWaterfall(b, f) {
  var e = bi[b];
  if (f < e.effcenterfreq - e.effsamplerate / 2 || f > e.effcenterfreq + e.effsamplerate / 2) zoomToFreq(b, e.zoom, f);
}
function setBand(n) {
  n = Number(n); if (!bi[n] || (n === band && _crab.selected)) return;
  _crab.selected = true;
  var old = band;
  if (bi[old] && old !== n && bi[old].lastfreq != null) { bi[old].lastmode = mode; bi[old].lastlo = lo; bi[old].lasthi = hi; }
  if (_crab.started && old !== n) { var O = _crab.bands[old]; if (O && O.ws && O.ws.readyState === 1) O.ws.send(JSON.stringify({ type: 'untune' })); }
  band = n;
  try { localStorage.setItem('crab_band', bi[n].name); } catch (e) {}
  var added = _crab.sel.indexOf(n) < 0;            // Schnellwahl/Chat-Link auf ein nicht angezeigtes Band → einblenden
  if (added) _crabSelSet(_crab.sel.concat([n]), true);
  var e = bi[n];
  if (_crab.started && e.lastfreq == null) _crabBandMode(n);   // erster Besuch: Betriebsart des Bandes aus der Konfiguration
  freq = (e.lastfreq != null) ? e.lastfreq : e.vfo - (isCw() ? (lo + hi) / 2 : 0);
  _crabGeom(n);
  try { var rs = document.freqform.group0; if (rs) { if (rs.length) { for (var i = 0; i < rs.length; i++) rs[i].checked = (i === n); } else rs.checked = true; } } catch (err) {}
  if (Number(view) !== Views.allbands || (added && _crab.started)) _crabBuildWaterfalls();
  _crabEnsureSockets();
  if (added && _crab.started && _crab.histReady) _crabHistNow();
  _crabTextFreq();
  _crabAudioFlush();
  _crabRetune();
  showMarks(n);
  if (typeof showListeners === 'function') showListeners();
  _crabDecButtons();
}
/* Betriebsart eines Bandes (config.toml: mode im Band), wenn sie von der aktuellen abweicht */
function _crabBandMode(n) {
  if (!bi[n] || bi[n].modeDone) return;             // je Band nur beim ersten Besuch
  bi[n].modeDone = true;
  var m = bi[n].mode && String(bi[n].mode).toLowerCase();
  if (!m || !_crabMODEFILTER[m] || m.toUpperCase() === mode) return;
  var mf = m === 'fm' ? [-8, 8] : _crabMODEFILTER[m]; setMode(m, mf[0], mf[1]);
}
function setView(v) {
  view = Number(v); createCookie('view', view, 3652);
  _crabBuildWaterfalls();
  _crabEnsureSockets();
  drawPassband();
}
function setWfHeight(px) {
  wfHeight = Math.max(30, Math.min(2000, Number(px) || 200));
  for (var i = 0; i < nWaterfalls; i++) {
    var B = _crab.bands[id2band(i)]; if (!B || !B.canvas) continue;
    var old = B.canvas, nc = document.createElement('canvas'); nc.width = 1024; nc.height = wfHeight; nc.style.display = 'block';
    nc.getContext('2d').drawImage(old, 0, 0);
    B.wfdiv.style.height = wfHeight + 'px'; B.wfdiv.replaceChild(nc, old); B.canvas = nc; B.ctx = nc.getContext('2d');
  }
  drawPassband();
  // neue Höhe → passenden Verlauf nachholen (gebündelt, fillHeight ruft das beim Start mehrfach)
  clearTimeout(_crab.hTimer); _crab.hTimer = setTimeout(_crabHistNow, 600);
}
function setWfSpeed(v) { var o = wfSlow; wfSlow = Math.max(1, Number(v) || 2); if (o !== wfSlow && _crab.started) for (var b = 0; b < bi.length; b++) if (band2id(b) >= 0) _crabSendWf(b); }
function setWfMode(v) { wfMode = Number(v) || 0; for (var b = 0; b < bi.length; b++) if (_crab.bands[b]) _crab.bands[b].clearOnNext = true; }

/* ===== Wasserfall-DOM: je sichtbarem Band ein Block mit clipscale / wfdiv / blackbar ===== */
function _crabBuildWaterfalls() {
  var cont = document.getElementById('waterfalls'); if (!cont) return;
  nWaterfalls = Number(view) === Views.allbands ? _crab.sel.length : 1;
  wfWaiting = nWaterfalls;
  cont.innerHTML = '';
  scaleEls = [];
  for (var i = 0; i < nWaterfalls; i++) {
    var b = id2band(i), B = _crab.bands[b];
    var block = document.createElement('div'); block.id = 'wfblock' + i; block.style.cssText = 'position:relative;width:1024px;';
    var wf = document.createElement('div'); wf.id = 'wfdiv' + i; wf.style.cssText = 'position:relative;height:' + wfHeight + 'px;overflow:hidden;background:#000;cursor:crosshair;';
    var cv = document.createElement('canvas'); cv.width = 1024; cv.height = wfHeight; cv.style.display = 'block'; wf.appendChild(cv);
    var clip = document.createElement('div'); clip.id = 'clipscale' + i; clip.style.cssText = 'position:relative;height:' + scaleHeight + 'px;overflow:hidden;background:#0b0c10;';
    var bar = document.createElement('div'); bar.id = 'blackbar' + i; bar.style.cssText = 'position:relative;height:' + 16 + 'px;background:#000;overflow:visible;';
    block.appendChild(clip); block.appendChild(wf); block.appendChild(bar);
    cont.appendChild(block);
    B.block = block; B.wfdiv = wf; B.canvas = cv; B.ctx = cv.getContext('2d'); B.bar = bar; B.clip = clip; B.started = false; B.waitReleased = false; B.clearOnNext = true; B.prev = null; B.acc = null; B.nacc = 0;
    scaleEls.push(clip);
    (function (bb) { setTimeout(function () { try { loadMarks(bb); } catch (e) { showMarks(bb); } }, 0); })(b);
    (function (bb) {
      wf.addEventListener('mousedown', function (ev) { scaleMouseDown(ev, bb); });
      wf.addEventListener('touchstart', function (ev) { if (bb !== band) setBand(bb); if (ev.touches.length >= 2) { _crabPinch(ev, true); return; } touchFreq(ev); }, { passive: false });
      wf.addEventListener('touchmove', function (ev) { if (ev.touches.length >= 2) _crabPinch(ev, false); }, { passive: false });
      wf.addEventListener('touchend', function () { _crab.pinch = null; });
      wf.addEventListener('wheel', function (ev) { ev.preventDefault(); if (bb !== band) setBand(bb); wheelStep(ev); }, { passive: false });
    })(b);
  }
  scaleEl = scaleEls[band2id(band)] || scaleEls[0];
  drawPassband();
}

/* ===== Passband-Anzeige (Elemente aus index.html) ===== */
function drawPassband() {
  var id = band2id(band), B = _crab.bands[band];
  var yb = document.getElementById('yellowbar'), ya = document.getElementById('yellowbara'), ca = document.getElementById('carrier'),
      el = document.getElementById('edgelower'), eu = document.getElementById('edgeupper');
  if (!yb || !ya || !ca || !el || !eu) return;
  var show = id >= 0 && B && B.block;
  [ya, ca, el, eu].forEach(function (x) { x.style.display = show ? '' : 'none'; });
  if (!show) { _crabParkDraw(); return; }
  var top = B.block.offsetTop + scaleHeight - 6;
  var xl = freq2x(freq + Math.min(lo, 0), band), xh = freq2x(freq + Math.max(hi, 0), band), xc = freq2x(freq, band);
  ya.style.position = 'absolute'; ya.style.left = '0px'; ya.style.top = top + 'px'; ya.style.width = '1024px'; ya.style.height = '18px'; ya.style.zIndex = 3;
  yb.style.left = xl + 'px'; yb.style.width = Math.max(1, Math.round(xh - xl)) + 'px'; yb.style.top = '0px';
  ca.style.position = 'absolute'; ca.style.left = (xc - 1) + 'px'; ca.style.top = '0px';
  el.style.left = (xl - 11) + 'px'; el.style.top = top + 'px'; el.style.zIndex = 4;
  eu.style.left = xh + 'px'; eu.style.top = top + 'px'; eu.style.zIndex = 4;
  _crabParkDraw();
}

/* Parkmarken: letzte Einstellung der angezeigten, aber gerade nicht gehörten Bänder */
/* Parkmarken sind aus; ui.json {"park_markers": true} schaltet sie wieder ein. Die letzte
   Frequenz je Band bleibt trotzdem gespeichert: Rückkehr ins Band stellt sie wieder ein. */
function _crabParkX(b) { var e = bi[b]; return (_crab.parkMarkers && e && e.lastfreq != null && b !== band && band2id(b) >= 0) ? freq2x(e.lastfreq, b) : null; }
function _crabParkDraw() {
  for (var b = 0; b < bi.length; b++) {
    var B = _crab.bands[b]; if (!B || !B.block) continue;
    var x = _crabParkX(b), el = B.park;
    if (x == null || x < 0 || x > 1024 || !B.wfdiv || !B.block.contains(B.wfdiv)) { if (el) el.style.display = 'none'; continue; }
    if (!el || !B.block.contains(el)) {
      el = document.createElement('div'); el.className = 'x1park';
      el.style.cssText = 'position:absolute;width:0;border-left:1px dashed rgba(26,155,98,.9);pointer-events:none;z-index:2;';
      var t = document.createElement('span');
      t.style.cssText = 'position:absolute;top:6px;left:3px;font:600 10px/1.4 ui-monospace,Menlo,monospace;color:#fff;background:rgba(26,155,98,.85);padding:0 4px;border-radius:3px;white-space:nowrap;';
      el.appendChild(t); B.block.appendChild(el); B.park = el;
    }
    var e = bi[b], lbl = (e.lastfreq / 1000).toFixed(4).replace('.', ',') + (e.lastmode ? ' ' + e.lastmode : '');
    el.style.display = ''; el.style.left = Math.round(x) + 'px'; el.style.top = B.wfdiv.offsetTop + 'px'; el.style.height = B.wfdiv.offsetHeight + 'px';
    var sp = el.firstChild; if (sp.textContent !== lbl) sp.textContent = lbl;
    sp.style.left = x > 960 ? '' : '3px'; sp.style.right = x > 960 ? '3px' : '';
    el.title = 'zuletzt gehört – anklicken, um zurückzukehren';
  }
}

/* ===== Maus / Touch ===== */
function cancelEvent(e) { if (e && e.preventDefault) e.preventDefault(); return false; }
function mouseFreq(e) { e = e || window.event; var rx = document.getElementById('rx'), r = rx.getBoundingClientRect(); return { x: e.clientX - r.left, y: e.clientY - r.top }; }
function _crabFreqAtX(x) { return (x - 512) * khzPerPx + centerfreq; }
function scaleMouseDown(e, b) {
  if (e.button !== undefined && e.button !== 0) return;
  if (b !== undefined && b !== band) {
    var px = _crabParkX(b), mx = mouseFreq(e).x;
    if (px != null && Math.abs(mx - px) <= 8) {           // auf die Parkmarke geklickt: Einstellung zurückholen
      var e0 = bi[b], m0 = e0.lastmode, l0 = e0.lastlo, h0 = e0.lasthi;
      setBand(b);
      if (m0 && (m0 !== mode || l0 !== lo || h0 !== hi)) setMode(m0.toLowerCase(), l0, h0);
      drawPassband();
      return cancelEvent(e);
    }
    setBand(b);
  }
  var p = mouseFreq(e);
  setFreq(_crabFreqAtX(p.x) - (hi + lo) / 2);
  dragging = 1; dragStartVal = freq; dragStartX = e.pageX; _crab.dragRxX = p.x;
  return cancelEvent(e);
}
function dragPassband(e) { dragging = 1; dragStartVal = freq; dragStartX = e.pageX; _crab.dragRxX = mouseFreq(e).x; return cancelEvent(e); }
function dragEdgeLo(e) { dragging = 2; dragStartVal = lo; _crab.dragRxX = mouseFreq(e).x; return cancelEvent(e); }
function dragEdgeHi(e) { dragging = 3; dragStartVal = hi; _crab.dragRxX = mouseFreq(e).x; return cancelEvent(e); }
function mouseup(e) { dragging = 0; }
function touchFreq(ev) {
  ev.preventDefault();
  for (var i = 0; i < ev.touches.length; i++) { var p = mouseFreq(ev.touches[i]); setFreq(_crabFreqAtX(p.x) - (hi + lo) / 2); }
}
/* Zwei-Finger-Zoom im Wasserfall (Handy): auseinander = Stufe rein, zusammen = Stufe raus, um die gehörte Frequenz */
function _crabPinch(ev, start) {
  ev.preventDefault();
  var a = ev.touches[0], b = ev.touches[1], d = Math.hypot(a.pageX - b.pageX, a.pageY - b.pageY);
  if (start || !_crab.pinch) { _crab.pinch = d; return; }
  var r = d / _crab.pinch;
  if (r > 1.25) { setZoom(0); _crab.pinch = d; } else if (r < 0.8) { setZoom(1); _crab.pinch = d; }
}
function touchPassband(ev) { ev.preventDefault(); if (ev.touches.length !== 1) return; setFreq(dragStartVal + (ev.touches[0].pageX - dragStartX) * khzPerPx); }
function keydown(e) { return true; }
document.addEventListener('mousemove', function (e) {
  if (!dragging) return;
  var x = mouseFreq(e).x, dk = (x - _crab.dragRxX) * khzPerPx;
  if (dragging === 1) setFreq(dragStartVal + dk);
  else if (dragging === 2) { lo = Math.min(hi - 0.1, dragStartVal + dk); updateBw(); }
  else if (dragging === 3) { hi = Math.max(lo + 0.1, dragStartVal + dk); updateBw(); }
});
document.addEventListener('mouseup', mouseup);

/* ===== Marker (marks) unter dem Wasserfall, Hörerliste ===== */
var _crabMODEFILTER = { fm: [-6, 6], am: [-4, 4], usb: [0.3, 2.7], lsb: [-2.7, -0.3], cw: [-0.95, -0.55] };
function _crabEsc(t) { return String(t).replace(/[&<>"]/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]; }); }
function showMarks(b) {
  var id = band2id(b), B = _crab.bands[b]; if (id < 0 || !B || !B.bar) return;
  var e = bi[b], lo_ = e.effcenterfreq - e.effsamplerate / 2, hi_ = e.effcenterfreq + e.effsamplerate / 2, html = '';
  var lastLabelX = -1e9, lastLabelText = '';
  if (!hideMarks) for (var i = 0; i < marks.length; i++) {
    var m = marks[i]; if (m.freq < lo_ || m.freq > hi_) continue;
    var x = freq2x(m.freq, b), md = String(m.mode || 'FM').toUpperCase(), txt = m.text || '';
    html += '<div class="dxl" style="left:' + x.toFixed(1) + 'px"></div>';
    // Beschriftungen dicht nebeneinander (z. B. Freenet 1–6 im 12,5-kHz-Raster) nur einmal zeichnen, Linien bleiben
    if (Math.abs(x - lastLabelX) < 44) continue;
    if (txt === lastLabelText && Math.abs(x - lastLabelX) < 40) continue;
    lastLabelText = txt; lastLabelX = x;
    html += '<div class="dxm" style="left:' + (x - 6).toFixed(1) + 'px" title="' + _crabEsc(txt) + ' ' + (m.freq / 1000).toFixed(4) + ' MHz ' + md + '" onclick="setFreqMark(' + b + ',' + m.freq + ',\'' + md + '\')">' + _crabEsc(txt) + '</div>';
  }
  B.bar.innerHTML = html;
}
function loadMarks(b) { showMarks(b); }
function showListeners() {
  var n = document.getElementById('numusers'); if (n) n.textContent = String(_crab.nlisteners != null ? _crab.nlisteners : lsNames.length);
  var u = document.getElementById('users'); if (u) u.textContent = lsNames.filter(Boolean).join(', ');
}
/* Hörerliste eines Bandes aus dem WebSocket übernehmen (sofort bei Änderung); andere Bänder bleiben, wie sie sind */
function _crabListenersFromWs(b, list) {
  var e = bi[b], names = [], bands = [], fr = [], ids = [];
  for (var i = 0; i < lsNames.length; i++) if (lsBands[i] !== b) { names.push(lsNames[i]); bands.push(lsBands[i]); fr.push(lsFreqs[i]); ids.push((_crab.uuIds || [])[i]); }
  list.forEach(function (l) {   // [id, name, freqHz, mode]
    if (!l[2]) return;
    names.push(l[1] || 'Hörer'); bands.push(b); fr.push((l[2] / 1000 - (e.centerfreq - e.samplerate / 2)) / e.samplerate); ids.push(l[0]);
  });
  lsNames = names; lsBands = bands; lsFreqs = fr; _crab.uuIds = ids;
  showListeners();
}
/* Eigene Marke sofort nachziehen (die Bestätigung des Servers kommt kurz danach) */
function _crabOwnMarker() {
  var B = _crab.bands[band]; if (!B || B.myId == null || !_crab.uuIds) return;
  var e = bi[band], f = dialFreq(), found = false;
  for (var i = 0; i < _crab.uuIds.length; i++) if (_crab.uuIds[i] === B.myId && lsBands[i] === band) { lsFreqs[i] = (f - (e.centerfreq - e.samplerate / 2)) / e.samplerate; found = true; }
  if (found) showListeners();
}
function _crabPollListeners() {
  var x = new XMLHttpRequest(); x.open('GET', 'api/listeners?_=' + Date.now()); x.timeout = 4000;
  x.onload = function () {
    try {
      var r = JSON.parse(x.responseText), names = [], bands = [], fr = [];
      (r.listeners || []).forEach(function (l) {
        var b = -1; for (var k = 0; k < bi.length; k++) if (bi[k].name === l.band) b = k;
        if (b < 0 || !l.freq) return;
        var e = bi[b]; names.push(l.name || 'Hörer'); bands.push(b); fr.push((l.freq / 1000 - (e.centerfreq - e.samplerate / 2)) / e.samplerate);
      });
      var ids = (r.listeners || []).filter(function (l) { var b = -1; for (var k = 0; k < bi.length; k++) if (bi[k].name === l.band) b = k; return b >= 0 && l.freq; }).map(function (l) { return l.id; });
      lsNames = names; lsBands = bands; lsFreqs = fr; _crab.uuIds = ids; _crab.nlisteners = r.n;
      showListeners();
    } catch (e) {}
  };
  x.send();
}

/* ===== Speicherplätze ===== */
function memStore(i) {
  memSlots[i] = { band: band, freq: freq, mode: mode, lo: lo, hi: hi, label: (dialFreq() / 1000).toFixed(4) + ' ' + mode };
  memShow();
}
function memRecall(i) {
  var m = memSlots[i]; if (!m) return;
  if (m.band !== band) setBand(m.band);
  setMode(m.mode.toLowerCase(), m.lo, m.hi); setFreq(m.freq);
}
function memErase(i) { memSlots.splice(i, 1); memShow(); }
function memLabel(i, t) { if (memSlots[i]) memSlots[i].label = t; }
function memShow() {
  var box = document.getElementById('memSlots'); if (!box) return;
  var h = '<table>';
  for (var i = 0; i < memSlots.length; i++) {
    var m = memSlots[i];
    h += '<tr><td><input type="button" value="' + _crabEsc(m.label) + '" onclick="memRecall(' + i + ')"></td><td><input type="text" value="' + _crabEsc(m.label) + '" onchange="memLabel(' + i + ',this.value)"></td><td><input type="button" value="×" onclick="memErase(' + i + ')"></td></tr>';
  }
  box.innerHTML = memSlots.length ? h + '</table>' : '';
}

/* ===== Ton: WebSocket-Opus → WebCodecs-Decoder → AudioWorklet (Jitter-Puffer) → Gain ===== */
var _crabWORKLET = `class J extends AudioWorkletProcessor {
  constructor() {
    super();
    this.buf = new Float32Array(48000 * 3); this.r = 0; this.w = 0; this.n = 0; this.on = false; this.under = 0;
    this.min = 48000 * 0.2; this.max = 48000 * 0.8; this.target = this.min;   // Vorlauf 200 ms, wächst bei Aussetzern bis 800 ms
    this.quiet = 0;                                                           // Blöcke seit dem letzten Aussetzer
    this.port.onmessage = e => {
      const d = e.data;
      if (d === 'flush') { this.r = this.w = this.n = 0; this.on = false; return; }
      if (d && d.target) { this.min = d.target; this.target = Math.max(this.target, this.min); return; }
      for (let i = 0; i < d.length; i++) { if (this.n >= this.buf.length) { this.r = (this.r + 1) % this.buf.length; this.n--; } this.buf[this.w] = d[i]; this.w = (this.w + 1) % this.buf.length; this.n++; }
      // Deutlich mehr Vorlauf als Ziel (Stau im Netz löst sich, alles kommt auf einmal): auf Ziel kürzen, sonst bleibt die Verzögerung hoch
      if (this.n > this.target + 48000 * 0.3) { const k = this.n - this.target; this.r = (this.r + k) % this.buf.length; this.n -= k; }
    };
  }
  process(inputs, outputs) {
    const o = outputs[0][0];
    if (!this.on && this.n >= this.target) this.on = true;
    if (!this.on || this.n < o.length) {
      o.fill(0);
      if (this.on && this.n < o.length) {   // Aussetzer: Vorlauf um 100 ms vergrößern
        this.on = false; this.under++; this.quiet = 0;
        this.target = Math.min(this.max, this.target + 48000 * 0.1);
        this.port.postMessage({ under: this.under, n: this.n, target: this.target });
      }
      return true;
    }
    for (let i = 0; i < o.length; i++) { o[i] = this.buf[this.r]; this.r = (this.r + 1) % this.buf.length; }
    this.n -= o.length;
    // 30 s ohne Aussetzer: Vorlauf um 50 ms zurücknehmen (375 Blöcke/s), damit die Verzögerung nicht dauerhaft hoch bleibt
    if (this.target > this.min && ++this.quiet >= 375 * 30) { this.quiet = 0; this.target = Math.max(this.min, this.target - 48000 * 0.05); }
    return true;
  }
}
registerProcessor('jcrab', J);`;
var _crabOPUSHEAD = new Uint8Array([0x4f, 0x70, 0x75, 0x73, 0x48, 0x65, 0x61, 0x64, 1, 1, 0x38, 0x01, 0x80, 0xbb, 0, 0, 0, 0, 0]);
function _crabAudioInit() {
  if (document.ct) return;
  var C = window.AudioContext || window.webkitAudioContext; if (!C) return;
  var ctx = new C({ sampleRate: 48000 }); document.ct = ctx;
  var A = _crab.audio;
  A.gain = ctx.createGain(); A.gain.gain.value = A.muted ? 0 : A.volume; A.gain.connect(ctx.destination);
  if (ctx.audioWorklet) {
    var url = URL.createObjectURL(new Blob([_crabWORKLET], { type: 'application/javascript' }));
    ctx.audioWorklet.addModule(url).then(function () {
      A.node = new AudioWorkletNode(ctx, 'jcrab', { numberOfInputs: 0, numberOfOutputs: 1, outputChannelCount: [1] });
      A.node.port.onmessage = function (e) { if (e.data && e.data.under != null) { _crab.underruns = e.data.under; _crab.bufTarget = e.data.target; console.info('Ton-Aussetzer #' + e.data.under + ', Vorlauf jetzt ' + Math.round(e.data.target / 48) + ' ms'); } };
      A.node.connect(A.gain); audioStarted = true;
    }).catch(function (e) { console.error('AudioWorklet:', e); _crabScriptFallback(ctx); });
  } else _crabScriptFallback(ctx);
  _crabDecoderInit();
  var pcm = !A.decoder;
  if (pcm !== A.pcm) { A.pcm = pcm; _crabSendAll({ type: 'set_codec', audio: pcm ? 'raw' : 'opus' }); }   // Verbindungen stehen schon: Server umstellen
}
function _crabScriptFallback(ctx) {
  var A = _crab.audio, q = [], qn = 0, sp = ctx.createScriptProcessor(4096, 0, 1);
  sp.onaudioprocess = function (ev) { var o = ev.outputBuffer.getChannelData(0), i = 0; while (i < o.length && q.length) { var c = q[0], take = Math.min(c.length, o.length - i); o.set(c.subarray(0, take), i); i += take; qn -= take; if (take === c.length) q.shift(); else q[0] = c.subarray(take); } for (; i < o.length; i++) o[i] = 0; };
  sp.connect(A.gain); A.node = { port: { postMessage: function (d) { if (d === 'flush') { q = []; qn = 0; } else if (d.length) { q.push(d); qn += d.length; if (qn > 48000) { q.shift(); } } } } };
  audioStarted = true;
}
function _crabAudioPush(f32) {
  var A = _crab.audio; _crab.audioFrames = (_crab.audioFrames || 0) + 1; if (A.node) A.node.port.postMessage(f32);
  if (A.rec) A.rec.push(f32);
}
function _crabAudioFlush() { var A = _crab.audio; if (A.node) A.node.port.postMessage('flush'); }   // Zeitstempel bleiben monoton (Decoder verwirft sonst Pakete)
function _crabDecoderInit() {
  var A = _crab.audio; if (typeof AudioDecoder !== 'function') return;
  try {
    A.decoder = new AudioDecoder({
      output: function (ad) { var f = new Float32Array(ad.numberOfFrames); ad.copyTo(f, { planeIndex: 0 }); ad.close(); _crabAudioPush(f); },
      error: function (e) { console.error('Opus:', e); }
    });
    A.decoder.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 1, description: _crabOPUSHEAD });
  } catch (e) { A.decoder = null; }
}
function _crabOnOpus(bytes) {
  var A = _crab.audio;
  if (!A.decoder || A.decoder.state !== 'configured') { _crabDecoderInit(); if (!A.decoder || A.decoder.state !== 'configured') return; }
  try { A.decoder.decode(new EncodedAudioChunk({ type: 'key', timestamp: A.ts, data: bytes })); A.ts += 20000; }
  catch (e) { console.warn('Opus decode:', e); _crabDecoderInit(); }
}
function _crabOnPcm(bytes) {
  var n = bytes.byteLength >> 1, f = new Float32Array(n), dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  for (var i = 0; i < n; i++) f[i] = dv.getInt16(i * 2, true) / 32768;
  _crabAudioPush(f);
}
var crabAudio = {
  smeter: function () { return Math.round((_crab.level + 127) * 100); },
  setvolume: function (lin) { var A = _crab.audio; A.volume = Math.max(0, Number(lin) || 0); if (A.gain) A.gain.gain.value = A.muted ? 0 : A.volume; },
  mute: function () { setMute(!_crab.audio.muted); }
};
function setMute(on) { var A = _crab.audio; A.muted = (on === undefined) ? !A.muted : !!on; if (A.gain) A.gain.gain.value = A.muted ? 0 : A.volume; var c = document.getElementById('mutecheckbox'); if (c) c.checked = A.muted; }
/* Regelung AM/SSB/CW: schnell/mittel/langsam (Server set_agc); bleibt im Browser gespeichert; bei FM ohne Wirkung */
/* Auswahlfelder geben nach der Wahl den Fokus frei (sonst schlucken sie die Pfeiltasten) */
function setAgc(v) {
  v = (v === 'fast' || v === 'slow') ? v : 'medium'; _crab.agcMode = v;
  try { localStorage.setItem('crab_agc', v); } catch (e) {}
  var s = document.getElementById('agcsel'); if (s && s.value !== v) s.value = v;
  _crabSendAll({ type: 'set_agc', mode: v });
}
function _crabAgcInit() { var v = 'medium'; try { v = localStorage.getItem('crab_agc') || 'medium'; } catch (e) {} _crab.agcMode = v; var s = document.getElementById('agcsel'); if (s) s.value = v; }
function _crabSqMsg() { return { type: 'set_squelch', mode: _crab.squelchOn ? 'auto' : 'off', margin: _crab.sqMargin, hang_ms: 250 }; }   // Haltezeit nur für kurze Einbrüche; bei Trägerende schließt der Server sofort
function setSquelch(on) {
  _crab.squelchOn = !!on; _crabSendAll(_crabSqMsg());
  if (mode === 'FM' && _crab.sqUser) try { localStorage.setItem('crab_sq_fm', on ? '1' : '0'); } catch (e) {}
  var cb = document.getElementById('squelchcheckbox'); if (cb && cb.checked !== !!on) cb.checked = !!on;
  var sc = document.getElementById('squelchcontrol'); if (sc) sc.classList.toggle('off', !on);
}
/* Schwelle der Rauschsperre in dB über dem Rauschboden des Kanals (Regler neben dem Squelch-Schalter, bleibt gespeichert) */
function setSquelchLevel(v) {
  v = Math.max(0, Math.min(30, Math.round(Number(v) || 0)));
  _crab.sqMargin = v; try { localStorage.setItem('crab_sq_db', String(v)); } catch (e) {}
  var r = document.getElementById('squelchlevel'), t = document.getElementById('squelchdb');
  if (r && Number(r.value) !== v) r.value = v; if (t) t.textContent = v + ' dB';
  if (_crab.squelchOn) _crabSendAll(_crabSqMsg());
}
function _crabSqInit() { var v = 6; try { var st = localStorage.getItem('crab_sq_db'); if (st !== null) v = Number(st); } catch (e) {} if (!(v >= 0 && v <= 30)) v = 6; _crab.sqMargin = v; setSquelchLevel(v); }
/* Rauschsperre folgt der Betriebsart: FM startet mit Sperre (wie am Funkgerät; eigene Wahl bleibt gespeichert),
   AM/SSB/CW ohne – dort sucht man gerade die schwachen Signale. */
function _crabSqForMode() {
  var fm = mode === 'FM'; if (_crab.sqFm === fm) return; _crab.sqFm = fm;
  var want = false;
  if (fm) { want = true; try { want = localStorage.getItem('crab_sq_fm') !== '0'; } catch (e) {} }
  _crab.sqUser = false; setSquelch(want); _crab.sqUser = true;
}

/* ===== Aufnahme (WAV aus dem decodierten Ton) ===== */
function recStart() { _crab.audio.rec = []; recUrl = null; }
function recStop() {
  var chunks = _crab.audio.rec || []; _crab.audio.rec = null;
  var n = 0; chunks.forEach(function (c) { n += c.length; });
  var buf = new ArrayBuffer(44 + n * 2), dv = new DataView(buf), p = 0;
  function str(s) { for (var i = 0; i < s.length; i++) dv.setUint8(p++, s.charCodeAt(i)); }
  str('RIFF'); dv.setUint32(p, 36 + n * 2, true); p += 4; str('WAVE'); str('fmt '); dv.setUint32(p, 16, true); p += 4;
  dv.setUint16(p, 1, true); p += 2; dv.setUint16(p, 1, true); p += 2; dv.setUint32(p, 48000, true); p += 4; dv.setUint32(p, 96000, true); p += 4;
  dv.setUint16(p, 2, true); p += 2; dv.setUint16(p, 16, true); p += 2; str('data'); dv.setUint32(p, n * 2, true); p += 4;
  chunks.forEach(function (c) { for (var i = 0; i < c.length; i++) { dv.setInt16(p, Math.max(-1, Math.min(1, c[i])) * 32767, true); p += 2; } });
  if (recUrl) URL.revokeObjectURL(recUrl);
  recUrl = URL.createObjectURL(new Blob([buf], { type: 'audio/wav' }));
  return recUrl;
}
function recClick() { if (_crab.audio.rec) { recStop(); var s = document.getElementById('reccontrol'); if (s) s.innerHTML = '<a href="' + recUrl + '" download="aufnahme.wav">speichern</a>'; } else recStart(); }

/* ===== Wasserfall zeichnen ===== */
function _crabPalette() {
  if (_crab.palette) return _crab.palette;
  var stops = [[0, 0, 0, 0], [0.18, 0, 0, 70], [0.38, 0, 40, 170], [0.55, 0, 190, 200], [0.72, 230, 230, 0], [0.88, 255, 90, 0], [1, 255, 255, 255]];
  var p = new Uint8ClampedArray(256 * 4);
  for (var i = 0; i < 256; i++) {
    var t = i / 255, k = 0; while (k < stops.length - 2 && t > stops[k + 1][0]) k++;
    var a = stops[k], b = stops[k + 1], u = (t - a[0]) / (b[0] - a[0]);
    p[i * 4] = a[1] + (b[1] - a[1]) * u; p[i * 4 + 1] = a[2] + (b[2] - a[2]) * u; p[i * 4 + 2] = a[3] + (b[3] - a[3]) * u; p[i * 4 + 3] = 255;
  }
  return (_crab.palette = p);
}
function _crabFloor(line) {   // 20. Perzentil (Rauschboden) über eine Stichprobe
  var s = []; for (var i = 0; i < line.length; i += 8) s.push(line[i]);
  s.sort(function (a, b) { return a - b; }); return s[Math.floor(s.length * 0.2)] || 0;
}
function _crabDrawLine(b, line) {
  var B = _crab.bands[b]; if (!B || !B.ctx) return;
  // Tempo: wfSlow Zeilen zu einer mitteln
  if (!B.acc || B.acc.length !== line.length) { B.acc = new Float32Array(line.length); B.nacc = 0; }
  for (var i = 0; i < line.length; i++) B.acc[i] += line[i];
  if (++B.nacc < wfSlow) return;
  var n = B.nacc; B.nacc = 0;
  var fl = _crabFloor(line), pal = _crabPalette(), lo0, span;
  var dark = true;
  if (wfMode === 2) { lo0 = fl - 4; span = 28; } else if (wfMode === 3) { lo0 = fl + 6; span = 70; } else if (dark) { lo0 = fl - 4; span = 50; } else { lo0 = fl - 10; span = 55; }
  B.floorDb = fl - 140;
  var ctx = B.ctx, w = B.canvas.width, h = B.canvas.height;
  if (B.clearOnNext) { ctx.fillStyle = '#000'; ctx.fillRect(0, 0, w, h); B.clearOnNext = false; }
  B.liveSinceHist = (B.liveSinceHist || 0) + 1;
  if (wfMode === 0) {   // Spektrum-Linie
    ctx.fillStyle = '#000'; ctx.fillRect(0, 0, w, h); ctx.strokeStyle = '#34d399'; ctx.lineWidth = 1; ctx.beginPath();
    for (var x = 0; x < w; x++) { var v = (B.acc[x] / n - lo0) / span, y = h - 1 - Math.max(0, Math.min(1, v)) * (h - 2); if (x === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y); }
    ctx.stroke();
  } else {
    ctx.drawImage(B.canvas, 0, 1);
    var img = ctx.createImageData(w, 1), d = img.data;
    for (var px = 0; px < w; px++) {
      var t = Math.max(0, Math.min(255, Math.round((B.acc[px] / n - lo0) / span * 255))) * 4;
      d[px * 4] = pal[t]; d[px * 4 + 1] = pal[t + 1]; d[px * 4 + 2] = pal[t + 2]; d[px * 4 + 3] = 255;
    }
    ctx.putImageData(img, 0, 0);
  }
  for (var k = 0; k < line.length; k++) B.acc[k] = 0;
}
function _crabOnWaterfall(b, u8) {
  if (_crab.bands[b]) _crab.bands[b].lastLine = Date.now();
  if (u8.length < 5) return;
  var zoom = u8[1], start = u8[2] | (u8[3] << 8), delta = u8[4] & 1, B = _crab.bands[b], e = bi[b];
  if (zoom !== e.szoom || start !== e.sstart) return;    // veraltete Zeile nach Zoomwechsel
  var payload; try { payload = _crabZstd(u8.subarray(5)); } catch (err) { return; }
  if (!payload || payload.length !== 1024) return;
  if (delta) { if (!B.prev) return; for (var i = 0; i < 1024; i++) B.prev[i] = (B.prev[i] + payload[i]) & 255; }
  else B.prev = new Uint8Array(payload);
  if (!B.started) { B.started = true; if (!B.waitReleased && --wfWaiting <= 0) { wfWaiting = 0; wfAllStarted(); } }
  var sc = _crabScale(e.zoom, e);
  if (sc === 1) _crabDrawLine(b, B.prev);
  else {   // Stufe 3/4: sichtbaren Teil des Server-Ausschnitts strecken
    var off = e.start - e.sstart, out = new Uint8Array(1024);
    for (var j = 0; j < 1024; j++) out[j] = B.prev[Math.min(1023, off + (j / sc | 0))];
    _crabDrawLine(b, out);
  }
}
function _crabHistNow() {
  _crab.histReady = true;
  for (var b = 0; b < bi.length; b++) if (band2id(b) >= 0 && _crab.bands[b]) { _crab.bands[b].liveSinceHist = 0; _crabSendWf(b); }
}
/* Verlauf beim Anmelden: viele Zeilen am Stück (älteste zuerst) → Wasserfall sofort voll, neueste oben */
function _crabOnWaterfallHist(b, u8) {
  if (u8.length < 8) return;
  var zoom = u8[1], start = u8[2] | (u8[3] << 8), n = u8[4] | (u8[5] << 8), delta = u8[6] & 1, B = _crab.bands[b], e = bi[b];
  if (zoom !== e.szoom || start !== e.sstart || !B || !B.ctx || wfMode === 0) return;
  var raw; try { raw = _crabZstd(u8.subarray(7)); } catch (err) { return; }
  if (!raw || raw.length !== n * 1024) return;
  if (delta) for (var q = 1; q < n; q++) { var o0 = q * 1024, p0 = o0 - 1024; for (var k = 0; k < 1024; k++) raw[o0 + k] = (raw[p0 + k] + raw[o0 + k]) & 255; }
  var sc = _crabScale(e.zoom, e), off = e.start - e.sstart, w = B.canvas.width, h = Math.min(B.canvas.height, n);
  var pal = _crabPalette(), img = B.ctx.createImageData(w, h), d = img.data, line = new Uint8Array(1024);
  for (var r = 0; r < h; r++) {           // Zeile r im Bild = (n-1-r)-te im Verlauf (neueste oben)
    var src = raw.subarray((n - 1 - r) * 1024, (n - r) * 1024);
    if (sc === 1) line.set(src); else for (var j = 0; j < 1024; j++) line[j] = src[Math.min(1023, off + (j / sc | 0))];
    var fl = _crabFloor(line), lo0, span;
    if (wfMode === 2) { lo0 = fl - 4; span = 28; } else if (wfMode === 3) { lo0 = fl + 6; span = 70; } else { lo0 = fl - 4; span = 50; }
    for (var px = 0; px < w; px++) {
      var t = Math.max(0, Math.min(255, Math.round((line[px] - lo0) / span * 255))) * 4, o = (r * w + px) * 4;
      d[o] = pal[t]; d[o + 1] = pal[t + 1]; d[o + 2] = pal[t + 2]; d[o + 3] = 255;
    }
  }
  B.ctx.fillStyle = '#000'; B.ctx.fillRect(0, 0, w, B.canvas.height);
  B.ctx.putImageData(img, 0, 0); B.clearOnNext = false; B.acc = null; B.nacc = 0;
  if (!B.started) { B.started = true; if (!B.waitReleased && --wfWaiting <= 0) { wfWaiting = 0; wfAllStarted(); } }
  var tip = document.querySelector('.wfwarm'); if (tip) tip.remove();   // „Wasserfall läuft an …" ist dann überflüssig
}
/* Verlauf als fertiges JPEG (neueste Zeile oben): einfach in den Wasserfall legen, Live-Zeilen schieben es weiter */
function _crabOnWaterfallJpeg(b, u8) {
  if (u8.length < 8 || typeof createImageBitmap !== 'function') return;
  var zoom = u8[1], start = u8[2] | (u8[3] << 8), B = _crab.bands[b], e = bi[b];
  if (zoom !== e.szoom || start !== e.sstart || !B || !B.ctx || wfMode === 0 || _crabScale(e.zoom, e) !== 1) return;
  createImageBitmap(new Blob([u8.subarray(6)], { type: 'image/jpeg' })).then(function (bmp) {
    if (zoom !== e.szoom || start !== e.sstart) return;          // inzwischen gezoomt
    var live = B.liveSinceHist || 0;                                // schon gezeichnete Live-Zeilen oben freilassen
    B.ctx.drawImage(bmp, 0, live);
    B.clearOnNext = false;
    if (!B.started) { B.started = true; if (!B.waitReleased && --wfWaiting <= 0) { wfWaiting = 0; wfAllStarted(); } }
    var tip = document.querySelector('.wfwarm'); if (tip) tip.remove();
  }, function () {});
}
function wfAllStarted() { allLoaded = true; drawPassband(); _crabFillSoon(80); }

/* ===== zstd-Dekoder (nur was der Server nutzt: Rohblöcke und komprimierte Blöcke ohne Wörterbuch) — eigene Umsetzung ===== */
function _crabZstd(src) {
  if (window._crabZstdImpl) return window._crabZstdImpl(src);
  // Fallback: Server-Zeilen sind bei zstd-Level 1 meist ein einzelner Block; ohne Dekoder-Implementierung nur Rohblöcke
  var p = 4, hdr = src[p++], fcs = hdr >> 6, single = (hdr >> 5) & 1, dictId = hdr & 3;
  if (!single) p++;
  p += [0, 1, 2, 4][dictId];
  p += fcs === 0 ? (single ? 1 : 0) : [1, 2, 4, 8][fcs];
  var out = [];
  for (;;) {
    var bh = src[p] | (src[p + 1] << 8) | (src[p + 2] << 16); p += 3;
    var last = bh & 1, type = (bh >> 1) & 3, size = bh >> 3;
    if (type === 0) { for (var i = 0; i < size; i++) out.push(src[p + i]); p += size; }
    else if (type === 1) { for (var j = 0; j < size; j++) out.push(src[p]); p++; }
    else throw new Error('komprimierter zstd-Block ohne Dekoder');
    if (last) break;
  }
  return new Uint8Array(out);
}

/* ===== WebSocket je Band ===== */
function _crabSendWf(b) {
  var B = _crab.bands[b], e = bi[b];
  if (B && B.ws && B.ws.readyState === 1 && band2id(b) >= 0) B.ws.send(JSON.stringify({ type: 'set_waterfall', zoom: e.szoom, start_bin: e.sstart, hist_rows: (wfMode === 0 || !_crab.histReady) ? 0 : wfHeight, slow: wfSlow, fmt: _crabScale(e.zoom, e) === 1 ? 'jpeg' : 'rows', mode: wfMode }));   // gestreckt: Rohzeilen, das JPEG kann der Browser nicht strecken
}
function _crabSendAll(msg) { _crab.bands.forEach(function (B) { if (B.ws && B.ws.readyState === 1) B.ws.send(JSON.stringify(msg)); }); }
function _crabToken(cb) {
  if (_crab.token) return cb(_crab.token);
  if (_crab.tokenWait) { _crab.tokenWait.push(cb); return; }
  _crab.tokenWait = [cb]; cb = function (t) { var w = _crab.tokenWait || []; _crab.tokenWait = null; w.forEach(function (f) { f(t); }); };
  // angemeldet (account.js, geprüft): eigenes Hören-Token – öffnet auch Mitglieder- und Admin-Bänder
  if (window.crabAccount && !_crab.accountChecked) {
    _crab.accountChecked = true;
    crabAccount.ready(function () {
      var t = crabAccount.token(); if (t) { _crab.token = t; cb(t); return; }
      var waiting = _crab.tokenWait || []; _crab.tokenWait = null;          // alle, die inzwischen warten, bekommen das Gast-Token
      _crabToken(function (g) { waiting.forEach(function (f) { f(g); }); });
    });
    return;
  }
  var x = new XMLHttpRequest(); x.open('POST', 'api/auth/guest'); x.setRequestHeader('Content-Type', 'application/json');
  x.onload = function () { try { _crab.token = JSON.parse(x.responseText).token; cb(_crab.token); } catch (e) { _crabStatus('Kein Gastzugang: ' + x.status); } };
  x.onerror = function () { _crabStatus('Server nicht erreichbar'); setTimeout(function () { _crabToken(cb); }, 3000); };
  x.send('{}');
}
function _crabStatus(t) { var s = document.getElementById('stats'); if (s) s.textContent = t; }
function _crabEnsureSockets() {
  for (var b = 0; b < bi.length; b++) {
    var want = band2id(b) >= 0, B = _crab.bands[b];
    if (want && !B.ws) _crabConnect(b);
    else if (!want && B.ws) { B.closing = true; try { B.ws.close(); } catch (e) {} B.ws = null; }
    else if (want && B.ws && B.ws.readyState === 1) { _crabSendWf(b); if (b === band) B.ws.send(JSON.stringify(_crab.decAudio ? { type: 'listen_decoder', id: _crab.decAudio } : _crabTuneMsg())); }
  }
}
function _crabConnect(b) {
  var B = _crab.bands[b]; B.closing = false;
  _crabToken(function (tok) {
    if (B.ws || band2id(b) < 0) return;
    var ws = new WebSocket(_crab.wsBase + 'ws/' + encodeURIComponent(bi[b].name) + '?token=' + encodeURIComponent(tok));
    ws.binaryType = 'arraybuffer'; B.ws = ws;
    ws.onopen = function () {
    ws.send(JSON.stringify({ type: 'session', id: _crabSessionId() }));   // eine Person = eine Kennung über alle Bänder
      B.retry = 0;
      ws.send(JSON.stringify({ type: 'set_codec', audio: _crab.audio.pcm ? 'raw' : 'opus' }));
      var nm = (document.usernameform && document.usernameform.username.value || '').trim() || _crabGuestName(); ws.send(JSON.stringify({ type: 'set_name', name: nm }));
      if (_crab.squelchOn) ws.send(JSON.stringify(_crabSqMsg()));
      if (_crab.agcMode && _crab.agcMode !== 'medium') ws.send(JSON.stringify({ type: 'set_agc', mode: _crab.agcMode }));
      B.prev = null; B.clearOnNext = true; _crabSendWf(b);
      if (b === band) { _crabAudioFlush(); ws.send(JSON.stringify(_crab.decAudio ? { type: 'listen_decoder', id: _crab.decAudio } : _crabTuneMsg())); }
    };
    ws.onmessage = function (ev) {
      if (typeof ev.data === 'string') {
        var m; try { m = JSON.parse(ev.data); } catch (e) { return; }
        if (m.type === 'error') { _crabStatus(m.msg || 'Abgelehnt'); console.warn('crabSDR:', m.msg); B.retry = 5; return; }   // Station voll o. ä.: Hinweis, langsam neu versuchen
        if (m.type === 'config') { bi[b].centerfreq = m.center_freq / 1000; bi[b].samplerate = m.sample_rate / 1000; if (m.fft_size && m.fft_size !== bi[b].fft) { bi[b].fft = m.fft_size; bi[b].zoom = Math.min(bi[b].zoom, _crabMaxZoom(bi[b])); } _crabGeom(b); B.myId = m.client_id; }
        return;
      }
      var u8 = new Uint8Array(ev.data), tag = u8[0];
      if (tag === 0x84) _crabOnWaterfall(b, u8);
      else if (tag === 0x85) _crabOnWaterfallHist(b, u8);
      else if (tag === 0x86) _crabOnWaterfallJpeg(b, u8);
      else if (tag === 0x82) { if (b === band) _crabOnOpus(u8.subarray(1)); }
      else if (tag === 0x02) { if (b === band) _crabOnPcm(u8.subarray(1)); }
      else if (tag === 0x03) {
        var j; try { j = JSON.parse(new TextDecoder().decode(u8.subarray(1))); } catch (e) { return; }
        if (j.type === 'level') { if (b === band) { _crab.level = j.db; _crab.floor = j.floor; _crab.sq = j.sq; } }
        else if (j.type === 'listeners') { _crabListenersFromWs(b, j.list || []); }
        else if (j.type === 'decoder') { if (j.kind === 'sync') { _crab.decLock = _crab.decLock || {}; _crab.decLock[j.id] = !!(j.data && j.data.on); _crabMarkDec(); } }   // FreeDV-Lock → Knopf „DV“ leuchtet
        else if (j.type === 'system_stats') { if (b === band) { _crab.sysPct = (j.sys_pct != null) ? j.sys_pct : null; _crabStatus('Past 10 seconds: CPUload=' + (j.cpu_pct != null ? j.cpu_pct.toFixed(1) : '0.0') + '%, ' + j.clients + ' users; ' + j.channels + ' channels'); } }
      }
    };
    ws.onclose = function () {
      if (B.ws === ws) B.ws = null;
      if (B.closing || band2id(b) < 0) return;
      B.retry = Math.min(5, (B.retry || 0) + 1);
      setTimeout(function () { if (!B.ws && band2id(b) >= 0 && !document.hidden) _crabConnect(b); }, 1000 * B.retry);
    };
    ws.onerror = function () {};
  });
}

/* ===== Einstellungen der Oberfläche (ui.json) ===== */
function _crabLoadLook() {
  try {
    var x = new XMLHttpRequest(); x.open('GET', 'ui.json?' + Date.now(), false); x.send();
    var u = JSON.parse(x.responseText);
    _crab.features = u.features || {}; _crab.banner = u.banner || ''; _crab.links = u.links || {};
    // Verbund-Chat: Hinweis im Chat-Kopf, dass Zeilen auch auf den anderen Stationen erscheinen
    if (_crab.features.chat_verbund) { var ch = document.querySelector('.chathead small'); if (ch) { ch.textContent = 'Verbund: alle crabSDR-Stationen'; ch.title = 'Nachrichten gehen über crabsdr.de an alle Stationen mit Verbund-Chat'; } }
    _crab.bandsOff = Array.isArray(u.bands_off) ? u.bands_off.map(String) : [];
    _crab.parkMarkers = u.park_markers === true;
  } catch (e) {}
  document.body.classList.add('look-crab');
  if (!document.getElementById('crabmark')) {
    var foot = document.querySelector('.foot');
    var h1 = document.querySelector('.brand h1');
    if (h1 && !document.getElementById('crabbrand') && _crab.features.own_logo) { var cb = document.createElement('a'); cb.id = 'crabbrand'; cb.className = 'crabbrand'; cb.href = 'https://github.com/do1xx/crabSDR'; cb.target = '_blank'; cb.rel = 'noopener';
      cb.title = 'läuft mit crabSDR – freie Software (MIT)'; cb.innerHTML = '<img src="crabsdr-mark.svg" alt="crabSDR"><span>crabSDR</span>'; h1.appendChild(cb); }
    if (foot) { var a = document.createElement('a'); a.id = 'crabmark'; a.className = 'crabmark'; a.href = 'admin/';   // Sysop-Bereich (Anmeldung); Projektseite steht auf der Info-Seite
      a.title = 'crabSDR – Sysop-Bereich'; a.setAttribute('aria-label', 'crabSDR – Sysop-Bereich'); a.innerHTML = '<span class="crabemoji" aria-hidden="true">🦀</span>'; foot.appendChild(a); }
  }
  if (!document.getElementById('crab-look-css')) {
    var l = document.createElement('link'); l.id = 'crab-look-css'; l.rel = 'stylesheet'; l.href = 'look.css?v=9'; document.head.appendChild(l);
  }
}

/* Offline-Anzeige je Wasserfall (traurige Krabbe): Verbindung seit > 4 s weg, oder verbunden, aber seit > 4 s keine
   Wasserfall-Zeile – dann liefert der Empfänger nichts (Stick fehlt, abgestürzt, rtl_tcp hängt). */
function _crabOfflineCheck() {
  _crabSelMark(); _crabParkDraw();
  var now = Date.now();
  for (var b = 0; b < _crab.bands.length; b++) {
    var B = _crab.bands[b]; if (!B || !B.wfdiv) continue;
    var shown = B.wfdiv.offsetParent !== null;
    var up = B.ws && B.ws.readyState === 1;
    if (up) { B.downSince = 0; if (!B.upSince) B.upSince = now; } else { B.upSince = 0; if (!B.downSince) B.downSince = now; }
    var why = null;
    if (!up && B.downSince && now - B.downSince > 4000) why = 'Verbindung zum Server unterbrochen – versuche es weiter …';
    else if (up && B.upSince && now - B.upSince > 4000 && now - (B.lastLine || 0) > 4000) why = 'Kein Signal vom Empfänger – Stick nicht angeschlossen oder gestört';
    var el = B.wfdiv.querySelector('.crab-offline');
    // Band ohne Daten nicht ewig als „startet noch“ zählen, sonst passt ui.js die Wasserfallhöhen nie an
    // (fillHeight wartet auf alle) und untere Wasserfälle rutschen unter die Bedienleiste
    if (why && !B.started && !B.waitReleased && wfWaiting > 0) {
      B.waitReleased = true;
      if (--wfWaiting <= 0) { wfWaiting = 0; wfAllStarted(); }
    }
    if (why && shown) {
      if (!el) { el = document.createElement('div'); el.className = 'crab-offline'; B.wfdiv.appendChild(el); }
      if (el.dataset.why !== why) {
        el.dataset.why = why;
        el.innerHTML = '<img src="crabsdr-offline.svg" alt="">' + '<span>' + why + '</span>';
      }
    } else if (el) el.remove();
  }
}

/* ===== Bandauswahl (Desktop): Reiter oben schalten den Wasserfall des Bandes an und aus =====
   Angezeigtes Band anklicken = ausblenden (das letzte bleibt), anderes anklicken = nur einblenden.
   Hören und Abstimmen: Klick in den jeweiligen Wasserfall. Nicht aktive Bänder zeigen ihre letzte Einstellung als
   Parkmarke; Klick auf die Marke holt Frequenz und Betriebsart zurück. Auswahl bleibt im Browser gespeichert. */
function _crabSelSet(list, noRebuild) {
  var seen = {}, out = [];
  list.forEach(function (b) { b = Number(b); if (bi[b] && !seen[b] && !_crabIsOff(b)) { seen[b] = 1; out.push(b); } });
  if (!out.length) out = [band];
  out.sort(function (a, b) { return a - b; });
  _crab.sel = out;
  try { localStorage.setItem('crab_bands', JSON.stringify(out.map(function (b) { return bi[b].name; }))); } catch (e) {}
  _crabSelMark();
  if (noRebuild || !_crab.started || Number(view) !== Views.allbands) return;
  _crabBuildWaterfalls();
  _crabEnsureSockets();
  if (_crab.histReady) _crabHistNow();
  _crabFillSoon(60);
}
function _crabIsOff(b) { return (_crab.bandsOff || []).indexOf(bi[b].name) >= 0; }
/* Auswahl laden; ohne Deep-Link startet die Seite auf dem zuletzt gehörten Band (sonst auf dem ersten angezeigten).
   Gibt das Startband zurück. */
function _crabSelLoad(start, fromLink) {
  var names = null, last = null;
  try { names = JSON.parse(localStorage.getItem('crab_bands')); last = localStorage.getItem('crab_band'); } catch (e) {}
  var list = [], idx = function (nm) { for (var b = 0; b < bi.length; b++) if (bi[b].name === nm) return b; return -1; };
  if (Array.isArray(names)) names.forEach(function (nm) { var b = idx(nm); if (b >= 0 && !_crabIsOff(b)) list.push(b); });
  if (!fromLink && list.length) { var lb = idx(last); start = list.indexOf(lb) >= 0 ? lb : Math.min.apply(null, list); }
  if (list.indexOf(start) < 0) list.push(start);
  _crab.sel = []; _crabSelSet(list, true);
  return start;
}
function _crabSelMark() {
  var bb = document.querySelectorAll('#bandbar .band[data-band]'), multi = Number(view) === Views.allbands;
  document.body.classList.toggle('bandpick', multi);
  for (var i = 0; i < bb.length; i++) {
    var n = Number(bb[i].dataset.band), on = multi && _crab.sel.indexOf(n) >= 0;
    if (bb[i].classList.contains('shown') !== on) bb[i].classList.toggle('shown', on);
    if (multi && !bb[i].classList.contains('off')) bb[i].title = on ? (_crab.sel.length > 1 ? 'Wasserfall ausblenden' : 'Wasserfall wird angezeigt') : 'Wasserfall einblenden';
  }
}
document.addEventListener('click', function (ev) {
  var t = ev.target && ev.target.closest ? ev.target.closest('#bandbar .band[data-band]') : null;
  if (!t || Number(view) !== Views.allbands || !_crab.started || t.classList.contains('off')) return;
  ev.stopPropagation(); ev.preventDefault();          // Fangphase: gotoBand() aus ui.js läuft nicht
  var n = Number(t.dataset.band), i = _crab.sel.indexOf(n);
  if (i < 0) _crabSelSet(_crab.sel.concat([n]));        // nur einblenden – Empfänger und Marke bleiben, wo sie sind
  else if (_crab.sel.length > 1) {
    var rest = _crab.sel.filter(function (b) { return b !== n; });
    if (n === band) setBand(rest[0]);
    _crabSelSet(rest);
  }
  drawPassband();
}, true);
/* Raster sichtbar machen (steckte bei den ausgeblendeten Profi-Knöpfen) und feinere/gröbere Stufen anbieten */
document.addEventListener('DOMContentLoaded', function () {
  var st = document.createElement('style');
  st.textContent = 'label.snap.adv{display:flex!important}' +
    'body.bandpick .viewsel label.toggle:first-child{display:none!important}' +
    'body.bandpick .band[data-band]:not(.off) b::before{content:"";display:inline-block;width:7px;height:7px;border-radius:50%;border:1.5px solid currentColor;margin-right:6px;vertical-align:1px;opacity:.55}' +
    'body.bandpick .band.shown b::before{background:var(--accent,#1a9b62);border-color:var(--accent,#1a9b62);opacity:1}' +
    'body.bandpick .band.shown b{color:var(--text,inherit)}';
  document.head.appendChild(st);
  var sel = document.getElementById('snapsel'); if (!sel) return;
  var want = [['0', 'aus'], ['1', '1 kHz'], ['2.5', '2,5 kHz'], ['5', '5 kHz'], ['6.25', '6,25 kHz'], ['8.33', '8,33 kHz'], ['10', '10 kHz'], ['12.5', '12,5 kHz'], ['20', '20 kHz'], ['25', '25 kHz']];
  var have = {}; for (var i = 0; i < sel.options.length; i++) have[sel.options[i].value] = sel.options[i];
  sel.innerHTML = '';
  want.forEach(function (w) { var o = have[w[0]] || document.createElement('option'); o.value = w[0]; if (!have[w[0]]) o.textContent = w[1]; sel.appendChild(o); });
});
/* Desktop: gestapelte Ansicht ist jetzt die Bandauswahl (Standard); ?view=one erzwingt weiter ein Band */
try { if (!/[?&]view=one\b/.test(location.search) && window.innerWidth > 700) localStorage.setItem('crab_view', 'all'); } catch (e) {}

/* ===== Bildschirm teilen (Desktop): die angezeigten Wasserfälle teilen sich die freie Höhe gleichmäßig =====
   ui.js begrenzt bei mehreren Bändern auf 70–200 px je Wasserfall; deshalb übernimmt der Kern die
   Höhe (Auswahl „Bildschirm teilen“ in #wfsize, dann lässt fillHeight der Oberfläche die Finger davon). */
function _crabShareOn() { var ws = document.getElementById('wfsize'); return !!ws && ws.value === 'share' && !document.body.classList.contains('phone'); }
function _crabFill() {
  if (!_crabShareOn() || !nWaterfalls) return;          // wartet nicht auf Daten: ein Band ohne Stick darf nicht blockieren
  var main = document.getElementById('main'), rx = document.getElementById('rx'), wrap = document.getElementById('rxwrap');
  if (!main || !rx || !wrap) return;
  var host = wrap.parentNode, cs = getComputedStyle(host), sc = rx.getBoundingClientRect().width / 1024 || 1;
  var avail = main.clientHeight - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom) - 2 - 22;   // 22 = Streifen für Hörer-Marken
  var extra = rx.offsetHeight - wfHeight * nWaterfalls;              // Skalen + Markerleisten (unskaliert)
  var want = Math.floor((avail / sc - extra) / nWaterfalls);
  want = Math.max(60, Math.min(1200, want));
  if (Math.abs(want - wfHeight) > 4) { setWfHeight(want); window.dispatchEvent(new Event('resize')); }   // resize → fit() in ui.js: Skalen/Rahmen nachziehen
}
function _crabFillSoon(ms) { clearTimeout(_crab.fillT); _crab.fillT = setTimeout(_crabFill, ms || 120); }
function _crabShareInit() {
  var ws = document.getElementById('wfsize'); if (!ws || document.body.classList.contains('phone')) return;
  if (!ws.querySelector('option[value="share"]')) { var o = document.createElement('option'); o.value = 'share'; o.textContent = 'Bildschirm teilen'; ws.insertBefore(o, ws.firstChild); }
  ws.value = 'share';
  window.addEventListener('resize', function () { _crabFillSoon(250); });
  var main = document.getElementById('main');
  if (main && window.ResizeObserver) new ResizeObserver(function () { _crabFillSoon(150); }).observe(main);
  _crabFillSoon(300);
}

/* ===== Bedienfeld (crab-Look, Desktop): zwei ruhige Zeilen statt starrem Raster =====
   Oben: Frequenz, Schritt, Betriebsart, Bandbreite, S-Meter. Unten klein: Eingabe, Schnellwahl, Raster, Teilen,
   Rauschsperre/Stumm/Lautstärke/Aufnahme, Zoom, Decoder. Die vorhandenen Bedienelemente aus index.html werden
   nur umgehängt (IDs und Handler bleiben), Zeilen brechen um statt zu überlappen. */
function _crabPanelBuild() {
  if (document.getElementById('x1bar') || document.body.classList.contains('phone')) return;
  var strip = document.querySelector('.panel .strip'); if (!strip) return;
  var q = function (sel) { return strip.querySelector(sel); };
  var el = function (tag, cls, title) { var d = document.createElement(tag); if (cls) d.className = cls; if (title) d.title = title; return d; };
  var bar = el('div', 'x1bar'); bar.id = 'x1bar';
  // drei feste Zeilen: Hauptzeile (Frequenz, Betriebsart, Bandbreite, Signal) · Abstimmen · Ton und Anzeige
  var r1 = el('div', 'x1row x1main'), r2 = el('div', 'x1row x1tools'), r3 = el('div', 'x1row x1tools');
  var put = function (row, node, cls, title, label) {
    if (!node) return null; var g = el('div', 'x1grp ' + (cls || ''), title);
    if (label) { var l = el('span', 'x1lbl'); l.textContent = label; g.appendChild(l); }
    g.appendChild(node); row.appendChild(g); return g;
  };
  var sep = function (row) { row.appendChild(el('span', 'x1sep')); };
  // Zeile 1
  var fg = el('div', 'x1grp x1freq'); fg.appendChild(document.getElementById('freqmhz'));
  var steps = q('.steps'), share = document.getElementById('linkbtn');
  if (steps) { fg.appendChild(steps); }
  r1.appendChild(fg);
  put(r1, document.getElementById('modes'), 'x1seg', 'Betriebsart');
  var bw = el('div', 'x1grp x1bw', 'Bandbreite'), bwc = q('.bw'), bwp = document.getElementById('bwpresets');
  if (bwc && bwp) { var minus = bwc.children[0], val = bwc.querySelector('.bwval'), plus = bwc.children[bwc.children.length - 1];
    var seg = el('div', 'x1seg'); seg.appendChild(minus); seg.appendChild(bwp); seg.appendChild(plus); bw.appendChild(seg); if (val) bw.appendChild(val); }
  r1.appendChild(bw);
  put(r1, q('.smeter'), 'x1meter');
  // Zeile 2: Abstimmen
  var ff = document.querySelector('form[name=freqform]'); put(r2, ff, 'x1khz', 'Frequenz in kHz eintippen, Enter', 'Frequenz');
  var segsel = document.getElementById('segsel');
  var pr = q('.presetrow'); if (pr) { pr.classList.add('x1seg'); put(r2, pr, 'x1quick', 'Schnellwahl und Liste der Relais und Frequenzen', 'Schnellwahl'); }
  put(r2, segsel, 'x1range', 'Bereich des Bandplans: Frequenz, Betriebsart und Ausschnitt', 'Bereich');
  var sn = q('label.snap'); if (sn) { var snSel = sn.querySelector('select'); put(r2, snSel || sn, 'x1snap', 'Klicks im Wasserfall rasten in diesem Raster ein', 'Raster'); if (snSel) sn.remove(); }
  if (share) put(r2, share, 'x1share', 'Link zu dieser Frequenz kopieren');
  // Zeile 3: Ton und Anzeige
  var tg = q('.toggles');
  put(r3, tg, 'x1audio', null, 'Ton');
  if (tg) { var sc = document.getElementById('squelchcontrol'); var sqlabel = tg.querySelector('#squelchcheckbox'); if (sc && sqlabel) tg.insertBefore(sc, sqlabel.parentNode.nextSibling); }
  put(r3, document.getElementById('agccontrol'), 'x1agc', 'Regelung (AM/SSB/CW): wie schnell die Lautstärke nachgeführt wird');
  put(r3, document.getElementById('volumecontrol'), 'x1vol', 'Lautstärke', 'Lautstärke');
  put(r3, document.getElementById('recbtn'), 'x1rec', 'Ton als WAV aufnehmen (Taste R)');
  sep(r3);
  var wb = q('.wfbtns'); if (wb) { wb.classList.add('x1seg'); put(r3, wb, 'x1zoom', 'Wasserfall: Zoom (Tasten + und −)', 'Zoom'); }
  // Decoder: live aus /api/decoders (Zustand, letzte Treffer, abstimmen); mit Verweis auf die Digital-Seite
  var dec = el('div', 'x1grp x1dec'), db = el('button', 'btn x1decbtn', 'Decoder'); db.type = 'button'; db.innerHTML = 'Decoder <span class="x1caret">▾</span>';
  var pop = el('div', 'x1pop'); pop.id = 'x1decpop'; pop.innerHTML = '<div class="x1pophead">Decoder</div><div class="x1popempty">lädt …</div>';
  db.onclick = function (ev) { ev.stopPropagation(); dec.classList.toggle('open'); if (dec.classList.contains('open')) _crabDecLoad(); };
  document.addEventListener('click', function (ev) { if (!dec.contains(ev.target)) dec.classList.remove('open'); });
  pop.addEventListener('click', function (ev) {
    var a = ev.target.closest ? ev.target.closest('[data-decaudio]') : null;
    if (a) { ev.preventDefault(); _crabListenDecoder(a.dataset.decaudio, a.dataset.band, Number(a.dataset.freq)); dec.classList.remove('open'); return; }
    var b = ev.target.closest ? ev.target.closest('[data-tune]') : null; if (!b) return;
    ev.preventDefault(); _crabTuneTo(Number(b.dataset.tune), b.dataset.mode); dec.classList.remove('open');
  });
  dec.appendChild(db); dec.appendChild(pop); if (_crab.features.decoders !== false) { dec.classList.add('x1right'); r3.appendChild(dec); }
  setInterval(function () { if (dec.classList.contains('open')) _crabDecLoad(); }, 5000);
  bar.appendChild(r1); bar.appendChild(r2); bar.appendChild(r3);
  strip.parentNode.insertBefore(bar, strip);
  strip.classList.add('x1old');
  document.body.classList.add('x1panel');
  _crabFillSoon(200);
}

/* Decoder-Menü füllen: Zustand je Decoder + die letzten Treffer */
function _crabDecText(e) {
  var d = e.data || {};
  if (e.plugin === 'aprs') return (d.src || '') + (d.direct ? '' : ' (über ' + (d.digi || 'Digi') + ')') + (d.comment || d.status ? ' · ' + (d.comment || d.status) : '');
  if (e.plugin === 'ft8') return (d.msg || '') + (d.snr != null ? '  ' + d.snr + ' dB' : '');
  if (e.plugin === 'sstv') return e.kind === 'vis' ? 'Bild beginnt: ' + (d.mode || '') : 'Bild ' + (d.mode || '') + (d.lines ? ' · ' + d.lines + '/' + d.of + ' Zeilen' : '');
  return d.text || d.msg || d.message || JSON.stringify(d).slice(0, 80);
}
function _crabAgo(t) { var s = Math.max(0, Date.now() / 1000 - t); return s < 90 ? 'gerade' : s < 5400 ? 'vor ' + Math.round(s / 60) + ' min' : 'vor ' + Math.round(s / 3600) + ' h'; }
var _crabDIGI = { aprs: 'aprs', ft8: 'ft8', sstv: 'sstv', pocsag: 'pocsag', freedv: 'freedv' };
function _crabDecLoad() {
  var pop = document.getElementById('x1decpop'); if (!pop) return;
  var get = function (u, cb) { var x = new XMLHttpRequest(); x.open('GET', u); if (window.crabAccount) crabAccount.header(x); x.onload = function () { try { cb(JSON.parse(x.responseText)); } catch (e) { cb(null); } }; x.onerror = function () { cb(null); }; x.send(); };
  get('api/decoders', function (st) {
    get('api/decoders/events?limit=300', function (ev) {
      var list = (st && st.decoders) || [], evs = (ev && ev.events) || [];
      var h = '<div class="x1pophead">Decoder</div>';
      if (!list.length) { pop.innerHTML = h + '<div class="x1popempty">Noch keine Decoder eingerichtet.</div>'; return; }
      h += list.map(function (d) {
        var mine = evs.filter(function (e) { return e.id === d.id; }).slice(-3).reverse();
        var run = d.state === 'läuft', mode = d.mode === 'usb' ? 'usb' : d.mode === 'lsb' ? 'lsb' : d.mode === 'am' ? 'am' : 'fm';
        var digi = _crab.features.digital !== false && _crab.features.digital && _crabDIGI[d.plugin] ? '<a class="btn" href="digi/?tab=' + _crabDIGI[d.plugin] + '">Alle</a>' : '';
        return '<div class="x1decitem"><div class="x1dechead"><b>' + _crabEsc(d.label) + '</b><span class="x1decst ' + (run ? 'ok' : 'warn') + '">' + _crabEsc(d.state) + '</span></div>' +
          '<div class="x1decmeta">' + (d.freq / 1e6).toFixed(3).replace('.', ',') + ' MHz · ' + d.events + ' Treffer' + (d.last_event ? ' · zuletzt ' + _crabAgo(d.last_event) : '') + '</div>' +
          (mine.length ? '<div class="x1declast">' + mine.map(function (e) { return '<div><span>' + new Date(e.t * 1000).toTimeString().slice(0, 5) + '</span>' + _crabEsc(_crabDecText(e)) + '</div>'; }).join('') + '</div>' : '') +
          '<div class="x1decbtns"><button class="btn" type="button" data-tune="' + d.freq + '" data-mode="' + mode + '">Hören</button>' + (d.audio ? '<button class="btn btn-accent" type="button" data-decaudio="' + _crabEsc(d.id) + '" data-band="' + _crabEsc(d.band) + '" data-freq="' + d.freq + '">🔊 ' + _crabEsc(d.plugin === 'freedv' ? 'FreeDV' : d.label) + ' dekodiert</button>' : '') + digi + '</div></div>';
      }).join('');
      pop.innerHTML = h;
    });
  });
}
/* Dekodierten Ton eines Decoders hören (z. B. FreeDV): Band des Decoders einblenden, Frequenz anzeigen, dann statt des
   Kanals den Decoder-Ton bestellen. Jede eigene Abstimmung (Klick, Taste, Schnellwahl) holt den normalen Kanal zurück. */
function _crabListenDecoder(id, bandName, hz) {
  var b = -1; for (var i = 0; i < bi.length; i++) if (bi[i].name === bandName) b = i;
  if (b >= 0) { if (b !== band) setBand(b); setMode('usb', 0.3, 2.7); if (hz) setFreqText(String(hz / 1000)); }   // Anzeige: USB auf der Decoder-Frequenz
  _crab.decAudio = id; _crabAudioInit(); _crabAudioFlush();
  var B = _crab.bands[band]; if (B && B.ws && B.ws.readyState === 1) B.ws.send(JSON.stringify({ type: 'listen_decoder', id: id }));
  _crabMarkDec();
  _crabStatus('🔊 DV: FreeDV dekodiert – zum Zurückschalten abstimmen oder USB wählen');
}
/* Betriebsarten-Knöpfe für Decoder mit Ton (FreeDV) im aktuellen Band: neben FM/AM/USB, springen auf die Decoder-Frequenz */
function _crabDecButtons() {
  var row = document.getElementById('modes'); if (!row) return;
  Array.prototype.forEach.call(row.querySelectorAll('.btn[data-dec]'), function (b) { b.remove(); });
  var name = bi[band] && bi[band].name;
  (_crab.audioDecs || []).forEach(function (d) {
    if (d.band !== name) return;
    var b = document.createElement('button'); b.type = 'button'; b.className = 'btn'; b.dataset.mode = 'FREEDV'; b.dataset.dec = d.id;
    b.textContent = d.plugin === 'freedv' ? 'DV' : d.label; b.title = 'FreeDV (Codec 2) · ' + d.label + ' · ' + (d.freq / 1e6).toFixed(3).replace('.', ',') + ' MHz · dekodierte Sprache';
    b.onclick = function () { _crabListenDecoder(d.id, d.band, d.freq); };
    row.appendChild(b);
  });
  _crabMarkDec();
}
function _crabMarkDec() {
  Array.prototype.forEach.call(document.querySelectorAll('#modes .btn'), function (b) {
    if (b.dataset.dec) { b.classList.toggle('active', b.dataset.dec === _crab.decAudio); b.classList.toggle('lock', !!(_crab.decLock && _crab.decLock[b.dataset.dec])); }
    else if (_crab.decAudio) b.classList.remove('active');
  });
}
function _crabLoadAudioDecs() {
  var x = new XMLHttpRequest(); x.open('GET', 'api/decoders'); if (window.crabAccount) crabAccount.header(x);
  x.onload = function () {
    try { _crab.audioDecs = JSON.parse(x.responseText).decoders.filter(function (d) { return d.audio; }); } catch (e) { _crab.audioDecs = []; }
    _crabDecButtons();
    // Anfangszustand des Locks aus den letzten Treffern (danach kommt er live über den WebSocket)
    if (_crab.audioDecs.length) { var y = new XMLHttpRequest(); y.open('GET', 'api/decoders/events?limit=100'); if (window.crabAccount) crabAccount.header(y);
      y.onload = function () { try { var L = {}; JSON.parse(y.responseText).events.forEach(function (e) { if (e.kind === 'sync') L[e.id] = !!(e.data && e.data.on); }); _crab.decLock = L; _crabMarkDec(); } catch (e) {} }; y.send(); }
  };
  x.send();
}
function _crabListenDecoderById(id) {
  var x = new XMLHttpRequest(); x.open('GET', 'api/decoders'); if (window.crabAccount) crabAccount.header(x);
  x.onload = function () { try { var d = JSON.parse(x.responseText).decoders.filter(function (y) { return y.id === id; })[0]; if (d) _crabListenDecoder(d.id, d.band, d.freq); } catch (e) {} };
  x.send();
}

/* Auf eine Frequenz (Hz) abstimmen, Band suchen und einblenden, Betriebsart setzen */
function _crabTuneTo(hz, m) {
  var khz = hz / 1000;
  for (var b = 0; b < bi.length; b++) { var r = _crabBandRange(b); if (khz >= r.lo && khz <= r.hi && !_crabIsOff(b)) {
    if (b !== band) setBand(b);
    var f = m === 'usb' ? [0.3, 2.7] : m === 'lsb' ? [-2.7, -0.3] : m === 'am' ? [-4, 4] : [-6, 6];
    setMode(m, f[0], f[1]); setFreqText(String(khz)); return; } }
}

/* Schalter aus [ui] (ui.json features/banner/links): Reiter, Chat, Statuszeile, Aufnahme; Hinweis-Banner; Links unten */
function _crabApplyFeatures() {
  var F = _crab.features || {}, off = function (el) { if (el) el.style.display = 'none'; };
  document.querySelectorAll('[data-feature]').forEach(function (el) { if (F[el.dataset.feature] === false) off(el); });
  if (F.chat === false) { off(document.getElementById('chatbtn')); off(document.getElementById('chatdrawer')); document.body.classList.remove('chat-open', 'chat-side'); }
  if (F.status === false) off(document.getElementById('status'));
  if (F.recording === false) { off(document.getElementById('recbtn')); off(document.getElementById('reclink')); }
  if (_crab.banner && !document.getElementById('x1banner')) {
    var bn = document.createElement('div'); bn.id = 'x1banner'; bn.className = 'x1banner'; bn.textContent = _crab.banner;
    var top = document.querySelector('.top'); if (top && top.parentNode) top.parentNode.insertBefore(bn, top.nextSibling);
  }
  var L = _crab.links || {}, foot = document.querySelector('.foot');
  if (foot && (L.impressum || L.datenschutz) && !document.getElementById('x1links')) {
    var ln = document.createElement('span'); ln.id = 'x1links'; ln.className = 'x1links';
    ln.innerHTML = (L.impressum ? '<a href="' + _crabEsc(L.impressum) + '">Impressum</a>' : '') + (L.datenschutz ? '<a href="' + _crabEsc(L.datenschutz) + '">Datenschutz</a>' : '');
    var cm = document.getElementById('crabmark'); foot.insertBefore(ln, cm || null);
  }
  // Admin-Link nur bei Aufruf über eine private Adresse (Tailscale, LAN, WireGuard)
  if (L.admin && /^(100\.|10\.|192\.168\.|127\.)/.test(location.hostname) && !document.getElementById('x1admin')) {
    var lb = document.getElementById('langbtn'), ad = document.createElement('a'); ad.id = 'x1admin'; ad.className = 'btn btn-ghost'; ad.href = L.admin; ad.textContent = 'Admin';
    if (lb && lb.parentNode) lb.parentNode.insertBefore(ad, lb);
  }
}

/* ===== Start ===== */
function crabStart() {
  if (typeof bandinfo === 'undefined' || !bandinfo.length) { _crabStatus('Keine Bänder (bandinfo.js fehlt)'); return; }
  _crab.fft = bandinfo[0].fft_size || 4096;
  bi = bandinfo.map(function (x, i) { return { name: x.name, centerfreq: Number(x.centerfreq), samplerate: Number(x.samplerate), tuningstep: 0.03125, maxlinbw: 8,
    vfo: Number(x.vfo || x.centerfreq), fft: Number(x.fft_size) || _crab.fft, minzoom: 0, realband: i, zoom: 0, start: 0, effcenterfreq: Number(x.centerfreq), effsamplerate: Number(x.samplerate), lastfreq: null, mode: x.mode || null }; });
  _crab.bands = bi.map(function () { return { ws: null }; });
  for (var b = 0; b < bi.length; b++) _crabGeom(b);
  var cv = readCookie('view'); view = (cv === null || cv === '') ? Views.oneband : Number(cv);
  var un = readCookie('username'); if (document.usernameform) document.usernameform.username.value = (un && decodeURIComponent(un) !== 'Hörer') ? decodeURIComponent(un) : _crabGuestName();
  // Deep-Link ?tune=<kHz><modus>
  var start = { b: 0, f: null, m: null };
  var mt = /[?&]tune=([\d.]+)(fm|am|usb|lsb|cw|data)?/i.exec(location.search);
  if (mt) { var f0 = parseFloat(mt[1]); for (var k = 0; k < bi.length; k++) { var r = _crabBandRange(k); if (f0 >= r.lo && f0 <= r.hi) { start.b = k; start.f = f0; start.m = mt[2] ? mt[2].toLowerCase() : null; } } }
  if (start.m) { var mf = start.m === 'fm' ? [-8, 8] : _crabMODEFILTER[start.m]; mode = start.m.toUpperCase(); lo = mf[0]; hi = mf[1]; }
  // Durchlassbereich aus dem Link (&pb=lo,hi in kHz), z. B. SSB 0.30,2.70
  var mp = /[?&]pb=(-?[\d.]+),(-?[\d.]+)/.exec(location.search);
  if (mt && mp) { var plo = parseFloat(mp[1]), phi = parseFloat(mp[2]); if (isFinite(plo) && isFinite(phi) && phi > plo && phi - plo <= 50) { lo = plo; hi = phi; } }
  _crabLoadLook();
  _crabAudioInit();
  start.b = _crabSelLoad(start.b, !!mt);
  if (!start.m) _crabBandMode(start.b); else bi[start.b].modeDone = true;   // Link mit Betriebsart hat Vorrang
  band = start.b; _crabGeom(band);
  setView(view);
  _crab.started = true;
  setBand(start.b);
  if (start.f != null) setFreqText(String(start.f));
  // Weitere Einstellungen aus dem Link (docs/STREAM.md): &sq=auto:8|off|-85 &agc=slow &vol=-10 (dB) &mute=1 &name=… &ui=min
  (function () {
    var qs = location.search;
    var ms = /[?&]sq=(off|auto(?::(\d+))?|-?\d+)/i.exec(qs);
    if (ms) { _crab.sqUser = true; if (/^off$/i.test(ms[1])) setSquelch(false); else { if (ms[2]) setSquelchLevel(Number(ms[2])); setSquelch(true); } }
    var ma = /[?&]agc=(fast|medium|slow|off)/i.exec(qs); if (ma && typeof setAgc === 'function') setAgc(ma[1].toLowerCase());
    var mv = /[?&]vol=(-?\d+)/.exec(qs);
    if (mv) { var db = Math.max(-20, Math.min(6, Number(mv[1]))); var vr = document.getElementById('volumecontrol2'); if (vr) vr.value = db; crabAudio.setvolume(Math.pow(10, db / 10)); }
    if (/[?&]mute=1\b/.test(qs)) setMute(true);
    var mn = /[?&]name=([^&]+)/.exec(qs);
    if (mn && document.usernameform) { try { document.usernameform.username.value = decodeURIComponent(mn[1].replace(/\+/g, ' ')).slice(0, 32); } catch (e) {} }
    if (/[?&]ui=min\b/.test(qs)) document.body.classList.add('min');   // nur Frequenz, Betriebsart, Pegel, Ton – für viele Tabs
    _crabLoadAudioDecs();   // FreeDV & Co. als Betriebsarten-Knopf
    var mdc = /[?&]dec=([^&]+)/.exec(qs); if (mdc) setTimeout(function () { _crabListenDecoderById(decodeURIComponent(mdc[1])); }, 1200);   // ?dec=<id>: Decoder-Ton (z. B. FreeDV)
  })();
  _crabTextFreq();
  updateBw();
  window.addEventListener('mousemove', function () {});
  _crabPollListeners(); _crab.listenersTimer = setInterval(_crabPollListeners, 5000);
  _crabSqInit(); _crabAgcInit(); _crabSqForMode();
  setTimeout(function () { _crabApplyFeatures(); _crabPanelBuild(); _crabShareInit(); }, 0);      // nach dem Ladeteil der Oberfläche (die setzt #wfsize erst nach crabStart auf „auto“)
  setInterval(_crabOfflineCheck, 1000);
  setTimeout(_crabSelMark, 200);
  setTimeout(function () { if (!_crab.histReady) _crabHistNow(); }, 1200);   // Höhe steht (fillHeight) → Verlauf einmal holen
  document.addEventListener('visibilitychange', function () { if (!document.hidden) _crabEnsureSockets(); });
  memShow();
}
