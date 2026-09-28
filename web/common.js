/* Gemeinsamer Kopf der Unterseiten (Digital, Logbuch, Info): liest ../ui.json und setzt Stationsname, Untertitel und
   Seitentitel (Elemente mit data-station="name|sub|title"), blendet Reiter nach den Schaltern aus [ui] aus (data-feature),
   setzt Name, Hörer online, Chat-Knopf, Hell/Dunkel, Hinweisbanner und Impressum/Datenschutz.
   Seiten-Skripte warten mit crabReady(function (ui) { … }) auf ui.json (ui.station.lat/lon = Standort). */
(function () {
  'use strict';
  function $(id) { return document.getElementById(id); }
  function esc(t) { return String(t == null ? '' : t).replace(/[&<>"]/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]; }); }
  function readCookie(n) { var m = document.cookie.match('(?:^|; )' + n + '=([^;]*)'); return m ? decodeURIComponent(m[1]) : null; }
  function setCookie(n, v) { document.cookie = n + '=' + encodeURIComponent(v) + '; path=/; max-age=' + (3652 * 86400) + '; SameSite=Lax'; }
  var waiting = [], UI = null;
  window.crabReady = function (cb) { if (UI) cb(UI); else waiting.push(cb); };

  // Hell / Dunkel (gleicher Speicher wie die Hören-Seite)
  var tb = $('themebtn');
  if (tb && !tb.onclick) {
    var mark = function () { var cur = document.documentElement.getAttribute('data-theme'); tb.textContent = !cur ? '◐' : cur === 'dark' ? '☾' : '☀'; };
    tb.onclick = function () {
      var h = document.documentElement, cur = h.getAttribute('data-theme');
      var next = !cur ? 'dark' : cur === 'dark' ? 'light' : null;
      if (next) { h.setAttribute('data-theme', next); h.classList.remove('sysdark'); try { localStorage.setItem('crab_theme', next); } catch (e) {} }
      else { h.removeAttribute('data-theme'); h.classList.toggle('sysdark', window.matchMedia('(prefers-color-scheme: dark)').matches); try { localStorage.removeItem('crab_theme'); } catch (e) {} }
      mark(); document.dispatchEvent(new Event('crab:theme'));
    };
    mark();
  }

  var right = document.querySelector('.topright'), nav = right && right.querySelector('nav.tabs');
  var root = nav ? nav.querySelector('a').getAttribute('href') : '../';   // „../" auf den Unterseiten
  if (right && nav && !$('myname')) {
    var inp = document.createElement('input');
    inp.id = 'myname'; inp.className = 'myname'; inp.type = 'text'; inp.maxLength = 20; inp.autocomplete = 'off'; inp.placeholder = 'Dein Name / Call';
    right.insertBefore(inp, nav);
    var c = readCookie('username'); if (c && c !== 'Hörer') inp.value = c;
    inp.onchange = function () { setCookie('username', inp.value.trim().slice(0, 20) || 'Hörer'); };
  }
  var own = !!$('numusers');   // Logbuch-Seite pflegt die Zahl selbst
  if (right && nav && !own) {
    var u = document.createElement('span'); u.className = 'users'; u.title = 'Hörer gerade online';
    u.innerHTML = '<span id="numusers">–</span> Hörer';
    right.insertBefore(u, nav);
  }
  if (right && nav && !$('chatbtn')) {
    var a = document.createElement('a'); a.id = 'chatbtn'; a.className = 'btn btn-ghost chatbtn'; a.href = root + '?chat=1'; a.textContent = 'Chat'; a.title = 'Chat mit den anderen Hörern';
    right.insertBefore(a, nav);
  }

  function apply(ui) {
    var F = ui.features || {}, st = ui.station || {};
    Array.prototype.forEach.call(document.querySelectorAll('[data-feature]'), function (el) { if (F[el.dataset.feature] === false) el.style.display = 'none'; });
    if (F.chat === false && $('chatbtn')) $('chatbtn').style.display = 'none';
    Array.prototype.forEach.call(document.querySelectorAll('[data-station]'), function (el) {
      var k = el.dataset.station, v = k === 'name' ? st.name : k === 'sub' ? st.subtitle : k === 'locator' ? st.locator : null;
      if (v) el.textContent = v;
    });
    var page = document.body.dataset.page;
    if (page && st.name) document.title = st.name + ' · ' + page;
    if (ui.banner && !$('x1banner')) {
      var bn = document.createElement('div'); bn.id = 'x1banner'; bn.className = 'x1banner'; bn.textContent = ui.banner;
      var top = document.querySelector('.top'); if (top && top.parentNode) top.parentNode.insertBefore(bn, top.nextSibling);
    }
    var L = ui.links || {}, foot = document.querySelector('.foot');
    if (foot && (L.impressum || L.datenschutz) && !$('x1links')) {
      var ln = document.createElement('p'); ln.id = 'x1links'; ln.className = 'x1links';
      ln.innerHTML = (L.impressum ? '<a href="' + esc(L.impressum) + '">Impressum</a>' : '') + (L.datenschutz ? '<a href="' + esc(L.datenschutz) + '">Datenschutz</a>' : '');
      foot.appendChild(ln);
    }
  }
  var x = new XMLHttpRequest(); x.open('GET', root + 'ui.json?' + Date.now());
  x.onload = function () { var ui = {}; try { ui = JSON.parse(x.responseText) || {}; } catch (e) {} done(ui); };
  x.onerror = function () { done({}); };
  x.send();
  function done(ui) { UI = ui; window.crabUI = ui; apply(ui); waiting.splice(0).forEach(function (cb) { try { cb(ui); } catch (e) { console.error(e); } }); }

  if (!own && $('numusers')) {
    var poll = function () {
      if (UI && UI.features && UI.features.chat === false && UI.features.logbook === false) return;
      var y = new XMLHttpRequest(); y.open('GET', root + 'logbuch/api/online?_=' + Date.now());
      y.onload = function () { try { $('numusers').textContent = JSON.parse(y.responseText).count; } catch (e) {} };
      y.send();
    };
    poll(); setInterval(poll, 15000);
  }
})();
