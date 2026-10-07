/* Seite „Digital": zeigt, was die Decoder-Plugins empfangen (APRS, FT8, SSTV) und wie weit die Station hört.
   Daten: ../api/decoders (welche Decoder laufen), digi/aprs.json, ft8.json, sstv.json, relais.json (vom Server aus den
   Datenordnern der Decoder), ../positionen.json (feste Standorte ohne Positionsbake), ../logbuch/api/log (Reichweite).
   Standort und Name der Station kommen aus ../ui.json (common.js). */
crabReady(function (UI) {
  'use strict';
  var ST = UI.station || {}, NAME = ST.name || 'WebSDR';
  var HOME = (ST.lat != null && ST.lon != null) ? [ST.lat, ST.lon] : [51.16, 10.45];   // ohne Standort: Mitte Deutschlands
  function $(id) { return document.getElementById(id); }
  function esc(t) { return String(t == null ? '' : t).replace(/[&<>"]/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]; }); }
  function hm(t) { return t ? new Date(t * 1000).toLocaleTimeString('de-DE', { hour: '2-digit', minute: '2-digit', second: '2-digit' }) : '–'; }
  function ago(t) { var s = Math.max(0, Date.now() / 1000 - t); return s < 90 ? Math.round(s) + ' s' : s < 5400 ? Math.round(s / 60) + ' min' : Math.round(s / 3600) + ' h'; }
  function km(lat, lon) { var R = 6371, dLat = (lat - HOME[0]) * Math.PI / 180, dLon = (lon - HOME[1]) * Math.PI / 180; var a = Math.sin(dLat / 2) ** 2 + Math.cos(HOME[0] * Math.PI / 180) * Math.cos(lat * Math.PI / 180) * Math.sin(dLon / 2) ** 2; return 2 * R * Math.asin(Math.sqrt(a)); }

  function fmtMHz(hz) { return (hz / 1e6).toFixed(3).replace('.', ','); }
  // Hell/Dunkel schaltet common.js um, hier nur die Karte nachziehen
  document.addEventListener('crab:theme', function () { setTiles(); });

  // Karte: MapLibre GL mit den freien OpenFreeMap-Vektorkacheln (wie mapme.sh), dunkel/hell je nach Thema
  var STYLES = { dark: 'https://tiles.openfreemap.org/styles/dark', light: 'https://tiles.openfreemap.org/styles/positron' };
  function isDark() { var h = document.documentElement; return h.getAttribute('data-theme') === 'dark' || (!h.getAttribute('data-theme') && h.classList.contains('sysdark')); }
  var map = new maplibregl.Map({ container: 'map', style: STYLES[isDark() ? 'dark' : 'light'], center: [HOME[1], HOME[0]], zoom: 7, attributionControl: { compact: true }, preserveDrawingBuffer: true });   // preserveDrawingBuffer: für den Bild-Export der Reichweite
  map.addControl(new maplibregl.NavigationControl({ showCompass: false }), 'top-right');
  var homeMarker = (function () { var el = document.createElement('div'); el.className = 'mk mk-home'; el.title = NAME + ' · Empfänger'; return new maplibregl.Marker({ element: el }).setLngLat([HOME[1], HOME[0]]).addTo(map); })();
  var markerList = [];
  var markers = {
    clearLayers: function () { markerList.forEach(function (m) { m.remove(); }); markerList = []; },
    add: function (lat, lon, cls, html) {
      var el = document.createElement('div'); el.className = 'mk ' + cls;
      var m = new maplibregl.Marker({ element: el }).setLngLat([lon, lat]).setPopup(new maplibregl.Popup({ offset: 10, closeButton: false, className: 'tip' }).setHTML(html)).addTo(map);
      el.onmouseenter = function () { m.togglePopup(); }; el.onmouseleave = function () { if (m.getPopup().isOpen()) m.togglePopup(); };
      markerList.push(m);
    }
  };
  function setTiles() { map.setStyle(STYLES[isDark() ? 'dark' : 'light']); }

  // Maidenhead-Locator-Raster: Großfelder (JO), Felder (JO53), ab Zoom 9 Kleinfelder (JO53RB) – je nach Zoomstufe
  var gridOn = true; try { gridOn = localStorage.getItem('crab_grid') !== '0'; } catch (e) {}
  function locName(lon, lat, lvl) {
    var x = lon + 180, y = lat + 90, A = 'ABCDEFGHIJKLMNOPQRSTUVWX';
    var s = A[Math.floor(x / 20)] + A[Math.floor(y / 10)];
    if (lvl >= 1) s += Math.floor((x % 20) / 2) + '' + Math.floor(y % 10);
    if (lvl >= 2) s += A[Math.floor(((x % 2) / 2) * 24)] + A[Math.floor((y % 1) * 24)];
    return s;
  }
  function gridGeo() {
    var z = map.getZoom(), b = map.getBounds(), lvl = z < 4.5 ? 0 : z < 9 ? 1 : 2;
    var dLon = [20, 2, 2 / 24][lvl], dLat = [10, 1, 1 / 24][lvl];
    var w = Math.max(-180, b.getWest()), e = Math.min(180, b.getEast()), so = Math.max(-90, b.getSouth()), n = Math.min(90, b.getNorth());
    var lines = [], labels = [];
    function vline(x, l) { lines.push({ type: 'Feature', properties: { lvl: l }, geometry: { type: 'LineString', coordinates: [[x, so], [x, n]] } }); }
    function hline(y, l) { lines.push({ type: 'Feature', properties: { lvl: l }, geometry: { type: 'LineString', coordinates: [[w, y], [e, y]] } }); }
    for (var x = Math.floor((w + 180) / dLon) * dLon - 180; x <= e; x += dLon) vline(+x.toFixed(6), lvl);
    for (var y = Math.floor((so + 90) / dLat) * dLat - 90; y <= n; y += dLat) hline(+y.toFixed(6), lvl);
    if (lvl > 0) {   // Großfeld-Linien immer kräftig dazu
      for (var fx = Math.floor((w + 180) / 20) * 20 - 180; fx <= e; fx += 20) vline(fx, 0);
      for (var fy = Math.floor((so + 90) / 10) * 10 - 90; fy <= n; fy += 10) hline(fy, 0);
    }
    for (var cx = Math.floor((w + 180) / dLon) * dLon - 180; cx < e; cx += dLon)
      for (var cy = Math.floor((so + 90) / dLat) * dLat - 90; cy < n; cy += dLat)
        labels.push({ type: 'Feature', properties: { name: locName(cx + dLon / 2, cy + dLat / 2, lvl), lvl: lvl }, geometry: { type: 'Point', coordinates: [cx + dLon / 2, cy + dLat / 2] } });
    return { lines: { type: 'FeatureCollection', features: lines }, labels: { type: 'FeatureCollection', features: labels.slice(0, 600) } };
  }
  function styleFont() {
    var ls = (map.getStyle() || {}).layers || [];
    for (var i = 0; i < ls.length; i++) if (ls[i].layout && ls[i].layout['text-font']) return ls[i].layout['text-font'];
    return ['Noto Sans Regular'];
  }
  // Gehörte FT8-Felder (JO53 usw.): Fläche in der Akzentfarbe hervorheben
  var worked = {};
  function fieldPoly(g) {
    g = String(g || '').toUpperCase(); if (!/^[A-R]{2}\d{2}/.test(g)) return null;
    var lon = (g.charCodeAt(0) - 65) * 20 - 180 + Number(g[2]) * 2, lat = (g.charCodeAt(1) - 65) * 10 - 90 + Number(g[3]);
    return { type: 'Feature', properties: { name: g.slice(0, 4) }, geometry: { type: 'Polygon', coordinates: [[[lon, lat], [lon + 2, lat], [lon + 2, lat + 1], [lon, lat + 1], [lon, lat]]] } };
  }
  function workedGeo() { return { type: 'FeatureCollection', features: Object.keys(worked).map(fieldPoly).filter(Boolean) }; }
  function updateWorked() { if (map.getSource('grid-worked')) map.getSource('grid-worked').setData(workedGeo()); }
  function addGrid() {
    if (map.getSource('grid-lines')) return;
    var g = gridGeo(), dark = isDark(), col = dark ? '#ffffff' : '#000000';
    map.addSource('grid-worked', { type: 'geojson', data: workedGeo() });
    map.addLayer({ id: 'grid-worked', type: 'fill', source: 'grid-worked', paint: { 'fill-color': '#34d399', 'fill-opacity': dark ? 0.22 : 0.28 } });
    map.addLayer({ id: 'grid-worked-line', type: 'line', source: 'grid-worked', paint: { 'line-color': '#34d399', 'line-opacity': 0.9, 'line-width': 1.6 } });
    map.addSource('grid-lines', { type: 'geojson', data: g.lines });
    map.addSource('grid-labels', { type: 'geojson', data: g.labels });
    map.addLayer({ id: 'grid-lines', type: 'line', source: 'grid-lines', paint: { 'line-color': col, 'line-opacity': ['case', ['==', ['get', 'lvl'], 0], 0.45, 0.22], 'line-width': ['case', ['==', ['get', 'lvl'], 0], 1.4, 0.7] } });
    map.addLayer({ id: 'grid-labels', type: 'symbol', source: 'grid-labels', layout: { 'text-field': ['get', 'name'], 'text-font': styleFont(), 'text-size': 12, 'text-allow-overlap': false }, paint: { 'text-color': col, 'text-opacity': 0.5, 'text-halo-color': dark ? '#000' : '#fff', 'text-halo-width': 1 } });
    setGridVisible();
  }
  function updateGrid() { if (!map.getSource('grid-lines')) return; var g = gridGeo(); map.getSource('grid-lines').setData(g.lines); map.getSource('grid-labels').setData(g.labels); }
  function setGridVisible() { ['grid-lines', 'grid-labels'].forEach(function (id) { if (map.getLayer(id)) map.setLayoutProperty(id, 'visibility', gridOn ? 'visible' : 'none'); }); $('gridbtn').classList.toggle('active', gridOn); }
  map.on('style.load', addGrid); map.on('moveend', updateGrid);
  $('gridbtn').onclick = function () { gridOn = !gridOn; try { localStorage.setItem('crab_grid', gridOn ? '1' : '0'); } catch (e) {} setGridVisible(); };

  // Welche Decoder gibt es? Reiter nur für vorhandene (öffentliche) Decoder, Reichweite sobald APRS oder FT8 da ist
  var DECS = { aprs: [], ft8: [], sstv: [], pocsag: [], freedv: [] }, TABS = [];
  function freqs(list) { return list.map(function (d) { return fmtMHz(d.freq); }).join(' · '); }
  function bandOf(list) { var b = list[0] && list[0].band; var bi = (window.bandinfo || []).filter(function (x) { return x.name === b; })[0]; return bi && bi.label ? bi.label : b || ''; }
  function buildTabs() {
    var T = { aprs: ['APRS', 'AFSK 1200 Bd · direwolf'], ft8: ['FT8', '15-s-Zyklen · jt9'], sstv: ['SSTV', 'Bilder · Martin, Scottie, Robot, PD'], pocsag: ['Pager', 'POCSAG · multimon-ng'], freedv: ['FreeDV', 'Codec 2 · 700D, 700E, 1600'] }, h = [];
    TABS = [];
    ['aprs', 'ft8'].forEach(function (k) { if (DECS[k].length) { TABS.push(k); h.push('<button class="band" data-dec="' + k + '"><b>' + T[k][0] + ' <small>' + freqs(DECS[k]) + ' MHz</small></b><small>' + T[k][1] + (bandOf(DECS[k]) ? ' · ' + esc(bandOf(DECS[k])) : '') + '</small></button>'); } });
    if (DECS.aprs.length || DECS.ft8.length) { TABS.push('reichweite'); h.push('<button class="band" data-dec="reichweite"><b>Reichweite <small>so weit hört die Station</small></b><small>direkt empfangen · ' + (DECS.aprs.length ? 'APRS · ' : '') + (DECS.ft8.length ? 'FT8 · ' : '') + (UI.features && UI.features.logbook ? 'Logbuch · ' : '') + '90 Tage</small></button>'); }
    ['pocsag', 'freedv'].forEach(function (k) { if (DECS[k].length) { TABS.push(k); h.push('<button class="band" data-dec="' + k + '"><b>' + T[k][0] + ' <small>' + freqs(DECS[k]) + ' MHz</small></b><small>' + T[k][1] + '</small></button>'); } });
    if (DECS.sstv.length) { TABS.push('sstv'); h.push('<button class="band" data-dec="sstv"><b>SSTV <small>' + freqs(DECS.sstv) + ' MHz</small></b><small>' + T.sstv[1] + '</small></button>'); }
    $('dectabs').innerHTML = h.join('');
  }
  var dec = 'aprs';
  try { dec = localStorage.getItem('crab_dec') || 'aprs'; } catch (e) {}
  (function () { var m = /[?&]tab=(aprs|ft8|reichweite|sstv|pocsag|freedv)\b/.exec(location.search); if (m) dec = m[1]; })();   // Deep-Link, z. B. digi/?tab=reichweite
  function startTabs() {
    buildTabs();
    if (!TABS.length) { $('stations').innerHTML = $('packets').innerHTML = '<span class="muted">Auf dieser Station laufen keine öffentlichen Decoder.</span>'; $('status').textContent = ''; return false; }
    if (TABS.indexOf(dec) < 0) dec = TABS[0];
    return true;
  }
  function setDec(d) { dec = d; document.body.classList.toggle('dec-nomap', d === 'pocsag' || d === 'freedv'); document.body.classList.toggle('dec-aprs', d === 'aprs'); document.body.classList.toggle('dec-reichweite', d === 'reichweite'); renderReichweite.fitted = false; ensureRange(); try { localStorage.setItem('crab_dec', d); } catch (e) {} var bs = document.querySelectorAll('.band[data-dec]'); for (var i = 0; i < bs.length; i++) bs[i].classList.toggle('active', bs[i].dataset.dec === d); document.body.classList.toggle('sstv', d === 'sstv'); $('gallery').hidden = d !== 'sstv'; load(); }
  function wireTabs() { var bs = document.querySelectorAll('.band[data-dec]'); for (var i = 0; i < bs.length; i++) { bs[i].onclick = function () { setDec(this.dataset.dec); }; bs[i].classList.toggle('active', bs[i].dataset.dec === dec); } }

  // Pager: Liste der Rufe (RIC, Funktion, Text), keine Karte
  function renderPocsag(d) {
    var pg = d.pages || [];
    $('sttitle').textContent = 'Rufe'; $('pktitle').textContent = 'Zuletzt'; $('stcount').textContent = pg.length ? '(' + pg.length + ')' : ''; $('pkcount').textContent = '';
    var byRic = {}; pg.forEach(function (p) { byRic[p.ric] = (byRic[p.ric] || 0) + 1; });
    $('stations').innerHTML = pg.length ? pg.slice(0, 200).map(function (p) {
      return '<div class="pk"><span class="tm">' + new Date(p.t * 1000).toLocaleTimeString('de-DE', { hour: '2-digit', minute: '2-digit' }) + '</span> <b>' + p.ric + '</b> <small>F' + p.func + ' · ' + p.baud + ' Bd</small><br>' + (p.text ? esc(p.text) : '<span class="muted">' + (p.type === 'numeric' ? 'numerisch' : 'Tonruf') + '</span>') + '</div>';
    }).join('') : '<span class="muted">noch kein Ruf empfangen – die Lauscher warten auf ' + freqs(DECS.pocsag) + ' MHz</span>';
    var top = Object.keys(byRic).sort(function (a, b) { return byRic[b] - byRic[a]; }).slice(0, 20);
    $('packets').innerHTML = top.length ? '<div class="muted">häufigste RICs</div>' + top.map(function (r) { return '<div class="pk"><b>' + r + '</b> <small>' + byRic[r] + '×</small></div>'; }).join('') : '';
    $('status').innerHTML = '<span>Pager ' + freqs(DECS.pocsag) + ' · multimon-ng</span><span class="' + (Date.now() / 1000 - d.ts < 300 ? 'ok' : 'warn') + '">Stand ' + ago(d.ts) + '</span>';
  }
  // FreeDV: Sync je Mode, Textkanal, Knopf zum Hören des dekodierten Tons
  function renderFreedv(d) {
    var ms = d.modes || [], act = d.active, decs = DECS.freedv;
    $('sttitle').textContent = 'Modes'; $('pktitle').textContent = 'Hören'; $('stcount').textContent = ''; $('pkcount').textContent = '';
    $('stations').innerHTML = ms.map(function (m) { return '<div class="pk"><b>' + esc(m.mode) + '</b> ' + (m.sync ? '<span class="ok">Sync · ' + m.snr + ' dB</span>' : '<span class="muted">kein Signal</span>') + (act === m.mode ? ' <small>· aktiv</small>' : '') + '</div>'; }).join('') || '<span class="muted">Decoder läuft an</span>';
    $('packets').innerHTML = decs.map(function (x) {
      var base = location.origin + location.pathname.replace(/digi\/?(index\.html)?$/, '');
      return '<div class="pk"><a class="btn" href="' + base + '?dec=' + encodeURIComponent(x.id) + '">🔊 ' + esc(x.label) + ' dekodiert hören</a><br><small class="muted">VLC: ' + base + 'stream/decoder/' + encodeURIComponent(x.id) + '.ogg</small></div>';
    }).join('') + '<div class="muted">FreeDV ist digitale Sprache über SSB (Codec 2). Mit Sync hörst du die dekodierte Stimme statt des Modem-Rauschens; das Rufzeichen kommt über den Textkanal und erscheint in den Treffern.</div>';
    $('status').innerHTML = '<span>FreeDV ' + freqs(decs) + ' · libcodec2</span><span class="' + (Date.now() / 1000 - d.ts < 30 ? 'ok' : 'warn') + '">Stand ' + ago(d.ts) + '</span>';
  }

  function renderFt8(d) {
    var st = d.stations || [], dx = d.decodes || [];
    $('sttitle').textContent = 'Stationen'; $('pktitle').textContent = 'Decodes';
    $('stcount').textContent = st.length ? '(' + st.length + ')' : ''; $('pkcount').textContent = dx.length ? '(' + dx.length + ')' : '';
    markers.clearLayers();
    var withPos = st.filter(function (s) { return s.lat != null; });
    worked = {}; withPos.forEach(function (s) { if (s.grid && s.grid.length >= 4) worked[String(s.grid).slice(0, 4).toUpperCase()] = 1; }); updateWorked();
    withPos.forEach(function (s) {
      markers.add(s.lat, s.lon, 'mk-ft8', '<b>' + esc(s.call) + '</b> ' + esc(s.grid) + '<br>' + s.snr + ' dB · ' + ago(s.last) + ' · ' + km(s.lat, s.lon).toFixed(0) + ' km');
    });
    $('stations').innerHTML = st.length ? st.map(function (s) {
      return '<div class="st"><b>' + esc(s.call) + '</b><span class="muted">' + esc(s.grid || '') + '</span><span class="muted">' + s.n + '× · ' + s.snr + ' dB</span><span class="muted">' + ago(s.last) + '</span>' + (s.lat != null ? '<span class="dist">' + km(s.lat, s.lon).toFixed(0) + ' km</span>' : '') + '</div>';
    }).join('') : '<span class="muted">noch nichts gehört</span>';
    $('packets').innerHTML = dx.length ? dx.slice(0, 150).map(function (x) {
      return '<div class="pk"><span class="t">' + hm(x.t) + '</span><span class="lvl">' + (x.snr > 0 ? '+' : '') + x.snr + ' dB · ' + x.dt.toFixed(1) + ' s · ' + x.f + ' Hz</span><b>' + esc(x.msg) + '</b></div>';
    }).join('') : '<span class="muted">noch keine Decodes (alle 15 s wird ein Zyklus decodiert)</span>';
    $('status').innerHTML = '<span>FT8 ' + freqs(DECS.ft8) + ' · jt9</span><span class="' + (Date.now() / 1000 - d.ts < 120 ? 'ok' : 'warn') + '">Stand ' + hm(d.ts) + '</span><span>' + withPos.length + ' Stationen mit Locator</span>';
  }

  // APRS: Umschalter direkt / alle (gemerkt)
  var aprsMode = 'direct'; try { aprsMode = localStorage.getItem('crab_aprs_mode') || 'direct'; } catch (e) {}
  var lastAprs = null;
  function markMode() { Array.prototype.forEach.call(document.querySelectorAll('#aprsmode button'), function (b) { b.classList.toggle('on', b.dataset.m === aprsMode); }); }
  Array.prototype.forEach.call(document.querySelectorAll('#aprsmode button'), function (b) {
    b.onclick = function () { aprsMode = b.dataset.m; try { localStorage.setItem('crab_aprs_mode', aprsMode); } catch (e) {} markMode(); if (lastAprs) render(lastAprs); };
  });
  markMode(); document.body.classList.toggle('dec-aprs', dec === 'aprs'); document.body.classList.toggle('dec-reichweite', dec === 'reichweite');
  // Feste Standorte (../positionen.json: {CALL: {lat, lon, info, ungefaehr}}) für Stationen ohne eigene Positionsbake
  var FIXED = {};
  function withFixed(st) {
    st.forEach(function (s) {
      var v = FIXED[s.src];
      if (s.lat == null && v && v.lat != null) { s.lat = v.lat; s.lon = v.lon; s.comment = s.comment || v.info || ''; s.pos_t = 0; s.pos_fixed = true; s.pos_approx = !!v.ungefaehr; }
    });
    return st;
  }
  function render(d) {
    withFixed(d.stations || []);
    lastAprs = d;
    var allSt = d.stations || [], allPk = d.packets || [], onlyDirect = aprsMode === 'direct';
    var nDirSt = allSt.filter(function (s) { return s.direct; }).length, nDirPk = allPk.filter(function (p) { return p.direct; }).length;
    // Stationen der letzten 90 Tage, sortiert nach „zuletzt gehört" (im Direkt-Modus: zuletzt DIREKT gehört)
    var lastOf = function (s) { return onlyDirect ? (s.last_direct || 0) : s.last; };
    var sts = (onlyDirect ? allSt.filter(function (s) { return s.direct; }) : allSt.slice()).sort(function (a, b) { return lastOf(b) - lastOf(a); });
    d = { ts: d.ts, stations: sts, packets: onlyDirect ? allPk.filter(function (p) { return p.direct; }) : allPk };
    function tag(s) {
      if (s.relayed) return '<span class="tag digi" title="Digipeater, direkt gehört">Digi</span>';
      if (s.direct) return '<span class="tag dir" title="' + s.n_direct + ' von ' + s.n + ' Paketen direkt empfangen">direkt</span>';
      return '<span class="tag via" title="nur über Digipeater empfangen">über ' + esc((s.digis || [])[0] || 'Digi') + '</span>';
    }
    var st = d.stations || [], pk = d.packets || [];
    worked = {}; updateWorked();   // Felder nur bei FT8 hervorheben
    $('sttitle').textContent = 'Stationen · ' + (lastAprs.days || 90) + ' Tage, zuletzt gehört oben'; $('pktitle').textContent = 'Pakete';
    $('stcount').textContent = st.length ? '(' + st.length + ')' : '';
    $('pkcount').textContent = pk.length ? '(' + pk.length + ')' : '';
    markers.clearLayers();
    var withPos = st.filter(function (s) { return s.lat != null && s.lon != null && km(s.lat, s.lon) < 3000; });   // Baken mit Unsinns-Position (z. B. 23° N 113° O) nicht auf die Karte
    withPos.forEach(function (s) {
      markers.add(s.lat, s.lon, s.relayed ? 'mk-digi' : 'mk-aprs' + (s.direct ? '' : ' mk-via'), '<b>' + esc(s.src) + '</b> ' + tag(s) + '<br>' + esc(s.comment || '') + '<br>' + (s.pos_fixed ? (s.pos_approx ? 'Standort ungefähr (Liste)' : 'Standort aus Liste') : ago(s.pos_t)) + ' · ' + km(s.lat, s.lon).toFixed(0) + ' km' + (s.relayed ? '<br>' + s.relayed + ' Pakete weitergereicht' : ''));
    });
    $('stations').innerHTML = st.length ? st.map(function (s) {
      return '<div class="st"><b>' + esc(s.src) + '</b>' + tag(s) + '<span class="muted">' + (s.relayed ? s.relayed + '× weitergereicht' : s.n_direct && s.n_direct !== s.n ? s.n_direct + '/' + s.n + '×' : s.n + '×') + '</span><span class="muted" title="zuletzt ' + (onlyDirect ? 'direkt ' : '') + 'gehört">' + ago(lastOf(s)) + '</span>' +
        (s.lat != null ? '<span class="dist">' + km(s.lat, s.lon).toFixed(0) + ' km</span>' : '<span class="muted">ohne Position</span>') + '</div>';
    }).join('') : '<span class="muted">noch nichts gehört</span>';
    $('packets').innerHTML = pk.length ? pk.slice(0, 120).map(function (p) {
      var info = p.name && p.name !== p.src ? ' → ' + esc(p.name) : '';
      var pos = p.lat != null && p.lon != null ? ' · ' + p.lat.toFixed(4) + ', ' + p.lon.toFixed(4) + (p.alt ? ' · ' + Math.round(p.alt) + ' m' : '') : '';
      return '<div class="pk' + (p.direct ? '' : ' isvia') + '"><span class="t">' + hm(p.t) + '</span><b>' + esc(p.src) + '</b>' + info + (p.direct ? '<span class="tag dir">direkt</span>' : '<span class="tag via">über ' + esc(p.digi || '?') + '</span>') +
        '<span class="lvl">' + esc(p.lvl) + (p.err ? ' ✱' + esc(p.err) : '') + '</span><span class="cm">' + esc(p.comment || p.status || '') + esc(pos) + '</span></div>';
    }).join('') : '<span class="muted">noch keine Pakete</span>';
    $('status').innerHTML = '<span>APRS ' + freqs(DECS.aprs) + ' · direwolf</span><span class="' + (Date.now() / 1000 - d.ts < 120 ? 'ok' : 'warn') + '">Stand ' + hm(d.ts) + '</span><span>' + nDirSt + ' von ' + allSt.length + ' Stationen direkt empfangen (' + (lastAprs.days || 90) + ' Tage)</span><span>' + nDirPk + ' von ' + allPk.length + ' Paketen direkt</span>';
  }
  // Reichweite: direkt empfangene Stationen (reichweite.json) + Hüllkurve der weitesten Entfernung je Himmelsrichtung
  var rangeData = null, DIRS = ['N', 'NO', 'O', 'SO', 'S', 'SW', 'W', 'NW'], SRCN = { aprs: 'APRS', ft8: 'FT8', log: 'Logbuch' };
  function dest(km, deg) {
    var R = 6371, d = km / R, b = deg * Math.PI / 180, la1 = HOME[0] * Math.PI / 180, lo1 = HOME[1] * Math.PI / 180;
    var la2 = Math.asin(Math.sin(la1) * Math.cos(d) + Math.cos(la1) * Math.sin(d) * Math.cos(b));
    var lo2 = lo1 + Math.atan2(Math.sin(b) * Math.sin(d) * Math.cos(la1), Math.cos(d) - Math.sin(la1) * Math.sin(la2));
    return [lo2 * 180 / Math.PI, la2 * 180 / Math.PI];
  }
  function rangeGeo() {
    var f = [];
    if (dec === 'reichweite' && rangeData && rangeData.points && rangeData.points.length) {
      // Hüllkurve: je 5° die weiteste Station im Umkreis von ±25°, mindestens 20 km – ergibt eine weiche Form statt 8 Zacken
      var ring = [];
      for (var a = 0; a < 360; a += 5) {
        var m = 20;
        rangeData.points.forEach(function (p) { var dd = Math.abs(((p.deg - a) + 540) % 360 - 180); if (dd <= 25) m = Math.max(m, p.km); });
        ring.push(dest(m, a));
      }
      ring.push(ring[0]);
      f.push({ type: 'Feature', properties: { k: 'hull' }, geometry: { type: 'Polygon', coordinates: [ring] } });
      [100, 250, 500].forEach(function (r) { var c = []; for (var b = 0; b <= 360; b += 4) c.push(dest(r, b)); f.push({ type: 'Feature', properties: { k: 'ring', r: r }, geometry: { type: 'LineString', coordinates: c } }); });
    }
    return { type: 'FeatureCollection', features: f };
  }
  function ensureRange() {
    // isStyleLoaded() ist erst true, wenn alle Kacheln da sind -> einfach versuchen, sonst beim nächsten Leerlauf erneut
    try { addRange(); } catch (e) { map.once('idle', function () { try { addRange(); } catch (e2) {} }); }
  }
  function addRange() {
    if (!map.getSource('range')) {
      map.addSource('range', { type: 'geojson', data: rangeGeo() });
      map.addLayer({ id: 'range-fill', type: 'fill', source: 'range', filter: ['==', ['get', 'k'], 'hull'], paint: { 'fill-color': '#34d399', 'fill-opacity': 0.12 } });
      map.addLayer({ id: 'range-line', type: 'line', source: 'range', filter: ['==', ['get', 'k'], 'hull'], paint: { 'line-color': '#34d399', 'line-width': 1.5 } });
      map.addLayer({ id: 'range-rings', type: 'line', source: 'range', filter: ['==', ['get', 'k'], 'ring'], paint: { 'line-color': '#9ca3af', 'line-width': 1, 'line-dasharray': [2, 3], 'line-opacity': 0.6 } });
    } else map.getSource('range').setData(rangeGeo());
  }
  map.on('style.load', ensureRange);
  function renderReichweite(d) {
    rangeData = d; var p = d.points || [];
    $('sttitle').textContent = 'Am weitesten'; $('pktitle').textContent = 'Nach Richtung';
    $('stcount').textContent = p.length ? '(' + p.length + ' Stationen)' : ''; $('pkcount').textContent = '';
    markers.clearLayers(); worked = {}; updateWorked();
    p.forEach(function (s) {
      markers.add(s.lat, s.lon, 'mk-' + s.src, '<b>' + esc(s.call) + '</b> · ' + SRCN[s.src] + '<br>' + s.km + ' km · ' + DIRS[Math.round(s.deg / 45) % 8] + (s.grid ? ' · ' + esc(s.grid) : '') + '<br>' + ago(s.t));
    });
    ensureRange();
    if (!renderReichweite.fitted && p.length) {   // beim ersten Öffnen auf die ganze Reichweite zoomen
      renderReichweite.fitted = true;
      var b = new maplibregl.LngLatBounds([HOME[1], HOME[0]], [HOME[1], HOME[0]]);
      rangeGeo().features.forEach(function (f) { if (f.properties.k === 'hull') f.geometry.coordinates[0].forEach(function (c) { b.extend(c); }); });
      map.fitBounds(b, { padding: 30, maxZoom: 8, duration: 600 });
    }
    $('stations').innerHTML = p.length ? p.slice(0, 60).map(function (s) {
      return '<div class="st"><b>' + esc(s.call) + '</b><span class="muted">' + SRCN[s.src] + '</span><span class="muted">' + DIRS[Math.round(s.deg / 45) % 8] + '</span><span class="muted">' + ago(s.t) + '</span><span class="dist">' + s.km + ' km</span></div>';
    }).join('') : '<span class="muted">noch keine Daten</span>';
    var sec = d.sectors || [], mx = Math.max.apply(null, sec.concat([1])), cnt = { aprs: 0, ft8: 0, log: 0 };
    p.forEach(function (s) { cnt[s.src]++; });
    $('packets').innerHTML = '<div class="rw-dirs">' + DIRS.map(function (n, i) {
      return '<div class="rw-dir"><span>' + n + '</span><i style="width:' + (100 * (sec[i] || 0) / mx).toFixed(0) + '%"></i><b>' + (sec[i] ? sec[i] + ' km' : '–') + '</b></div>';
    }).join('') + '</div><p class="muted rw-note">Nur Direktempfang der letzten ' + (d.days || 90) + ' Tage: APRS-Stationen und Digipeater, die ' + esc(NAME) + ' selbst hört (keine weitergeleiteten Pakete), FT8-Decodes (Position = Locator-Mitte) und Logbuch-Einträge. ' +
      cnt.aprs + ' APRS · ' + cnt.ft8 + ' FT8 · ' + cnt.log + ' Logbuch.</p>';
    $('status').innerHTML = '<span>Reichweite · Direktempfang</span><span class="ok">Stand ' + hm(d.ts) + '</span><span>weiteste: ' + (p[0] ? esc(p[0].call) + ' ' + p[0].km + ' km' : '–') + '</span>';
  }
  // Reichweite als Bild (PNG) speichern bzw. am Handy direkt teilen; Link teilen
  var SRCCOL = { aprs: '#fbbf24', ft8: '#34d399', log: '#e5e7eb' };
  function exportRange() {
    if (!rangeData) return;
    map.once('idle', function () {
      var mc = map.getCanvas(), dpr = mc.width / mc.clientWidth, W = mc.width, head = Math.round(64 * dpr), foot = Math.round(40 * dpr);
      var c = document.createElement('canvas'); c.width = W; c.height = mc.height + head + foot;
      var g = c.getContext('2d'), p = rangeData.points || [], cnt = { aprs: 0, ft8: 0, log: 0 };
      p.forEach(function (s) { cnt[s.src]++; });
      g.fillStyle = '#07090a'; g.fillRect(0, 0, c.width, c.height);
      g.drawImage(mc, 0, head);
      // Stationen (die Karten-Marker sind HTML und stecken nicht im Kartenbild)
      p.forEach(function (s) {
        var pt = map.project([s.lon, s.lat]); if (pt.x < 0 || pt.y < 0 || pt.x > mc.clientWidth || pt.y > mc.clientHeight) return;
        g.beginPath(); g.arc(pt.x * dpr, head + pt.y * dpr, 4.5 * dpr, 0, 2 * Math.PI); g.fillStyle = SRCCOL[s.src]; g.fill();
        g.lineWidth = 1.2 * dpr; g.strokeStyle = 'rgba(0,0,0,.7)'; g.stroke();
      });
      var hp = map.project([HOME[1], HOME[0]]);
      g.beginPath(); g.arc(hp.x * dpr, head + hp.y * dpr, 7 * dpr, 0, 2 * Math.PI); g.fillStyle = '#34d399'; g.fill(); g.lineWidth = 2 * dpr; g.strokeStyle = '#07090a'; g.stroke();
      var far = p[0], mono = '"JetBrains Mono", Menlo, monospace';
      g.fillStyle = '#e5e7eb'; g.font = '600 ' + Math.round(20 * dpr) + 'px ' + mono; g.textBaseline = 'middle';
      g.fillText(NAME + ' · Reichweite', 16 * dpr, 22 * dpr);
      g.fillStyle = '#9ca3af'; g.font = Math.round(12 * dpr) + 'px ' + mono;
      g.fillText('Direktempfang der letzten ' + (rangeData.days || 90) + ' Tage · ' + cnt.aprs + ' APRS · ' + cnt.ft8 + ' FT8 · ' + cnt.log + ' Logbuch' + (far ? ' · weiteste: ' + far.call + ' ' + far.km + ' km' : ''), 16 * dpr, 46 * dpr);
      var y = c.height - foot / 2;
      g.fillStyle = '#34d399'; g.fillText(location.host, 16 * dpr, y);
      g.fillStyle = '#6b7280'; g.textAlign = 'right';
      g.fillText('Stand ' + new Date(rangeData.ts * 1000).toLocaleDateString('de-DE') + ' · Karte © OpenStreetMap-Mitwirkende, OpenFreeMap', W - 16 * dpr, y);
      var name = NAME.replace(/[^A-Za-z0-9]+/g, '-').replace(/^-|-$/g, '') + '-Reichweite-' + new Date().toISOString().slice(0, 10) + '.png';
      c.toBlob(function (blob) {
        if (!blob) { alert('Bild konnte nicht erzeugt werden.'); return; }
        var file = new File([blob], name, { type: 'image/png' });
        if (navigator.canShare && navigator.canShare({ files: [file] }) && matchMedia('(pointer: coarse)').matches) {
          navigator.share({ files: [file], title: NAME + ' Reichweite', text: 'So weit hört ' + NAME + ': ' + location.origin + location.pathname + '?tab=reichweite' }).catch(function () {});
          return;
        }
        var a = document.createElement('a'); a.href = URL.createObjectURL(blob); a.download = name; document.body.appendChild(a); a.click();
        setTimeout(function () { URL.revokeObjectURL(a.href); a.remove(); }, 2000);
      }, 'image/png');
    });
    map.triggerRepaint();
  }
  function shareRangeLink() {
    var url = location.origin + location.pathname + '?tab=reichweite', text = 'So weit hört ' + NAME + ': Reichweite des WebSDR';
    if (navigator.share && matchMedia('(pointer: coarse)').matches) { navigator.share({ title: NAME + ' Reichweite', text: text, url: url }).catch(function () {}); return; }
    if (navigator.clipboard) navigator.clipboard.writeText(text + '\n' + url).then(function () { var b = $('rwlink'); var t = b.textContent; b.textContent = 'Link kopiert ✓'; setTimeout(function () { b.textContent = t; }, 1800); });
    else window.prompt('Link', url);
  }
  $('rwpng').onclick = exportRange; $('rwlink').onclick = shareRangeLink;

  // SSTV-Galerie (sstv.json: Bilder neueste zuerst), Lightbox mit Blättern
  var gal = [], lbIdx = -1;
  function fmtT(t) { return new Date(t * 1000).toLocaleString('de-DE', { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' }); }
  function renderSstv(d) {
    gal = d.images || [];
    $('galcount').textContent = gal.length ? '(' + gal.length + ')' : '';
    $('galgrid').innerHTML = gal.length ? gal.map(function (im, i) {
      return '<figure data-i="' + i + '"><img loading="lazy" src="' + esc(window.crabAccount ? crabAccount.url(im.file) : im.file) + '" alt="' + esc(im.mode) + '"><figcaption><b>' + esc(im.mode) + '</b><span>' + (im.freq / 1e6).toFixed(3) + '</span><span>' + fmtT(im.t) + '</span></figcaption></figure>';
    }).join('') : '<span class="muted">noch kein Bild empfangen – die Lauscher warten auf ' + freqs(DECS.sstv) + ' MHz</span>';
    Array.prototype.forEach.call(document.querySelectorAll('#galgrid figure'), function (f) { f.onclick = function () { showLb(Number(f.dataset.i)); }; });
    var last = gal.length ? gal[0].t : 0;
    $('status').innerHTML = '<span>SSTV ' + freqs(DECS.sstv) + ' · eigener Decoder</span><span id="sstvrun"></span><span>' + (last ? 'letztes Bild ' + fmtT(last) : 'noch kein Bild') + '</span><span>' + gal.length + ' Bilder</span>';
    var run = DECS.sstv.filter(function (x) { return x.state === 'läuft'; }).length, el = $('sstvrun');
    el.className = run === DECS.sstv.length ? 'ok' : 'warn';
    el.textContent = run === DECS.sstv.length ? (run > 1 ? 'Lauscher laufen' : 'Lauscher läuft') : run + ' von ' + DECS.sstv.length + ' Lauschern laufen';
  }
  function showLb(i) {
    if (i < 0 || i >= gal.length) return;
    lbIdx = i; var im = gal[i];
    $('lbimg').src = window.crabAccount ? crabAccount.url(im.file) : im.file; $('lbinfo').textContent = im.mode + ' · ' + (im.freq / 1e6).toFixed(3) + ' MHz · ' + fmtT(im.t) + ' · ' + im.lines + '/' + im.of + ' Zeilen';
    $('lightbox').hidden = false;
  }
  $('lbclose').onclick = function () { $('lightbox').hidden = true; };
  $('lbprev').onclick = function () { showLb(lbIdx + 1); };   // neuere zuerst: „zurück“ = älter = höherer Index? nein: links = neuer
  $('lbnext').onclick = function () { showLb(lbIdx - 1); };
  $('lightbox').onclick = function (e) { if (e.target === this) this.hidden = true; };
  document.addEventListener('keydown', function (e) { if ($('lightbox').hidden) return; if (e.key === 'Escape') $('lightbox').hidden = true; if (e.key === 'ArrowLeft') showLb(lbIdx + 1); if (e.key === 'ArrowRight') showLb(lbIdx - 1); });

  // Reichweite aus aprs.json (direkt gehörte Stationen und Digis), ft8.json (Sender mit Locator) und dem Logbuch
  var CALLRE = /^(?:[A-Z]{1,2}|[A-Z][0-9]|[0-9][A-Z])[0-9][A-Z]{1,4}(?:[-\/][A-Z0-9]{1,3})?$/;
  var PREFIX_EU = /^(?:D[A-R]|D[0-9]|P[A-I]|O[N-TZ]|L[A-NX]|S[A-FMP-R]|H[AB]|O[EK-M]|G|M|2|E[I-J]|F|T[FK]|Y[LO]|L[YZ]|E[RS]|U|R|I|Z[ABS]|9A|S5|E[A-H]|C[T]|4X|Y[UT]|Z[23]|OH|OF|OG|OI|OJ|OX|OY|TF|JW|LA|SM|SA|SK|SL|OZ|5[PQ])/;
  var inEurope = HOME[0] > 34 && HOME[0] < 72 && HOME[1] > -25 && HOME[1] < 45;   // Präfix-Filter gegen Fehldecodes nur in Europa
  function bearing(lat, lon) {
    var la1 = HOME[0] * Math.PI / 180, la2 = lat * Math.PI / 180, dl = (lon - HOME[1]) * Math.PI / 180;
    var y = Math.sin(dl) * Math.cos(la2), x = Math.cos(la1) * Math.sin(la2) - Math.sin(la1) * Math.cos(la2) * Math.cos(dl);
    return (Math.atan2(y, x) * 180 / Math.PI + 360) % 360;
  }
  function loc2ll(g) {
    g = String(g || '').toUpperCase(); if (!/^[A-R]{2}\d{2}/.test(g)) return null;
    var lon = (g.charCodeAt(0) - 65) * 20 - 180 + Number(g[2]) * 2, lat = (g.charCodeAt(1) - 65) * 10 - 90 + Number(g[3]);
    if (/^[A-R]{2}\d{2}[A-X]{2}/.test(g)) { lon += (g.charCodeAt(4) - 65) * 5 / 60 + 2.5 / 60; lat += (g.charCodeAt(5) - 65) * 2.5 / 60 + 1.25 / 60; } else { lon += 1; lat += 0.5; }
    return [lat, lon];
  }
  function buildRange(aprs, ft8, log) {
    var now = Date.now() / 1000, DAYS = 90, pts = {};
    function put(src, call, lat, lon, t, n, extra) {
      if (lat == null || lon == null || Math.abs(lat) > 90 || Math.abs(lon) > 180 || (Math.abs(lat) < 0.01 && Math.abs(lon) < 0.01) || now - t > DAYS * 86400) return;
      var d = km(lat, lon); if (d > 3000) return;   // Unsinn (falsche Baken-Position)
      if (src !== 'log') {
        var base = call.split('/')[0].split('-')[0];
        if (!CALLRE.test(base) || (inEurope && !PREFIX_EU.test(base)) || (d > 800 && (n || 1) < 2)) return;
      }
      var k = src + ':' + call, p = pts[k];
      if (!p || t >= p.t) pts[k] = Object.assign({ src: src, call: call, lat: +lat.toFixed(4), lon: +lon.toFixed(4), km: Math.round(d), deg: Math.round(bearing(lat, lon)), t: Math.round(t) }, extra || {});
    }
    withFixed((aprs && aprs.stations) || []).forEach(function (s) { if (s.direct && s.lat != null) put('aprs', s.src, s.lat, s.lon, s.last_direct || s.last, (s.n_direct || 0) + (s.relayed || 0), s.pos_approx ? { approx: true } : null); });
    ((ft8 && ft8.stations) || []).forEach(function (s) { if (s.lat != null) put('ft8', s.call, s.lat, s.lon, s.last, s.n, { snr: s.snr, grid: s.grid }); });
    (log || []).forEach(function (r) { var ll = r.loc && loc2ll(r.loc); if (ll) put('log', String(r.call || '?').toUpperCase(), ll[0], ll[1], r.t, 1, { grid: String(r.loc).toUpperCase(), khz: r.freq }); });
    var p = Object.keys(pts).map(function (k) { return pts[k]; }).sort(function (a, b) { return b.km - a.km; });
    var sectors = [0, 0, 0, 0, 0, 0, 0, 0];
    p.forEach(function (x) { var i = Math.floor(((x.deg + 22.5) % 360) / 45); sectors[i] = Math.max(sectors[i], x.km); });
    return { ts: Math.round(now), days: DAYS, sectors: sectors, points: p };
  }
  function get(url, cb) {
    var x = new XMLHttpRequest(); x.open('GET', url + (url.indexOf('?') < 0 ? '?' : '&') + '_=' + Date.now()); x.timeout = 8000;
    if (window.crabAccount) crabAccount.header(x);   // angemeldet: auch nicht öffentliche Decoder
    x.onload = function () { var v = null; if (x.status === 200) try { v = JSON.parse(x.responseText); } catch (e) {} cb(v); };
    x.onerror = x.ontimeout = function () { cb(null); };
    x.send();
  }
  function load() {
    if (dec === 'reichweite') {
      var got = {}, n = 0;
      var fin = function (k, v) { got[k] = v; if (++n === 3) renderReichweite(buildRange(got.aprs, got.ft8, got.log)); };
      if (DECS.aprs.length) get('aprs.json', function (v) { fin('aprs', v); }); else fin('aprs', null);
      if (DECS.ft8.length) get('ft8.json', function (v) { fin('ft8', v); }); else fin('ft8', null);
      if (UI.features && UI.features.logbook) get('../logbuch/api/log?n=500', function (v) { fin('log', v); }); else fin('log', null);
      return;
    }
    get(dec + '.json', function (v) {
      if (!v) { $('status').textContent = 'noch keine Daten'; return; }
      try { (dec === 'ft8' ? renderFt8 : dec === 'sstv' ? renderSstv : dec === 'pocsag' ? renderPocsag : dec === 'freedv' ? renderFreedv : render)(v); } catch (e) { console.error(e); $('status').textContent = 'noch keine Daten'; }
    });
  }
  function refreshDecs(cb) {
    get('../api/decoders', function (v) {
      var D = { aprs: [], ft8: [], sstv: [], pocsag: [], freedv: [] };
      ((v && v.decoders) || []).forEach(function (x) { if (D[x.plugin]) D[x.plugin].push(x); });
      DECS = D; if (cb) cb();
    });
  }
  get('../positionen.json', function (v) {
    if (v) Object.keys(v).forEach(function (k) { if (k.charAt(0) !== '_') FIXED[k] = v[k]; });
    refreshDecs(function () {
      if (!startTabs()) return;
      wireTabs(); markMode(); setDec(dec);
      setInterval(load, 20000);
      setInterval(refreshDecs, 60000);
    });
  });
});
