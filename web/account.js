/* account.js — Anmelden auf der Hören-Seite und den Unterseiten (docs/SECURITY.md).
   Hören-Token (30 Tage) im localStorage „crab_token“; die Admin-Seite hat ein eigenes, getrenntes Token.
   Nach dem Laden wird das Token einmal geprüft (api/auth/me); ist es abgelaufen oder widerrufen, gilt man als Gast.
   Andere Skripte:  crabAccount.ready(fn)  ·  crabAccount.token()  ·  crabAccount.user()  ·  crabAccount.header(xhr)
                    crabAccount.url(u)  (hängt ?token= an, für <img> und WebSocket) */
(function () {
  'use strict';
  var base = (function () { var s = document.currentScript && document.currentScript.src; return s ? s.replace(/account\.js(\?.*)?$/, '') : '/'; })();
  var KEY = 'crab_token', UKEY = 'crab_user';
  var st = { token: null, user: null, checked: false, wait: [] };
  function en() { try { return localStorage.getItem('crab_lang') === 'en'; } catch (e) { return false; } }
  function T(de, e) { return en() ? e : de; }
  try { st.token = localStorage.getItem(KEY); st.user = JSON.parse(localStorage.getItem(UKEY) || 'null'); } catch (e) {}

  function save(tok, user) {
    st.token = tok; st.user = user;
    try { if (tok) { localStorage.setItem(KEY, tok); localStorage.setItem(UKEY, JSON.stringify(user)); } else { localStorage.removeItem(KEY); localStorage.removeItem(UKEY); } } catch (e) {}
  }
  function req(method, path, body, cb) {
    var x = new XMLHttpRequest(); x.open(method, base + path);
    x.setRequestHeader('Content-Type', 'application/json');
    if (st.token) x.setRequestHeader('Authorization', 'Bearer ' + st.token);
    x.onload = function () { var r = null; try { r = JSON.parse(x.responseText); } catch (e) {} cb(x.status, r); };
    x.onerror = function () { cb(0, null); };
    x.send(body ? JSON.stringify(body) : null);
  }
  function done() { st.checked = true; st.wait.splice(0).forEach(function (f) { try { f(); } catch (e) { console.error(e); } }); }

  window.crabAccount = {
    ready: function (f) { if (st.checked) f(); else st.wait.push(f); },
    token: function () { return st.token; },
    user: function () { return st.user; },
    header: function (x) { if (st.token) x.setRequestHeader('Authorization', 'Bearer ' + st.token); },
    url: function (u) { return st.token ? u + (u.indexOf('?') < 0 ? '?' : '&') + 'token=' + encodeURIComponent(st.token) : u; }
  };

  // gespeichertes Token prüfen (Rechte können sich geändert haben, Sitzung abgemeldet sein)
  if (st.token) {
    req('GET', 'api/auth/me', null, function (code, r) {
      if (code === 200 && r && r.user && r.user.role !== 'guest') { st.user = r.user; try { localStorage.setItem(UKEY, JSON.stringify(r.user)); } catch (e) {} }
      else if (code === 401 || (r && r.user && r.user.role === 'guest')) save(null, null);
      done(); paint();
    });
  } else done();

  /* ---------- Knopf und Dialog ---------- */
  var btn = null, pop = null;
  function el(tag, cls, text) { var e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; }
  function paint() {
    if (!btn) return;
    btn.textContent = st.user ? st.user.username + ' ▾' : T('Anmelden', 'Sign in');
    btn.title = st.user ? T('Angemeldet – Menü', 'Signed in – menu') : T('Anmelden für Mitglieder-Bänder', 'Sign in for member bands');
  }
  function closePop() { if (pop) { pop.remove(); pop = null; } }
  function modal(title) {
    closePop();
    pop = el('div', 'acct-back'); var box = el('div', 'acct-box'); pop.appendChild(box);
    var head = el('div', 'acct-head'); head.appendChild(el('b', null, title));
    var x = el('button', 'btn btn-ghost btn-icon', '✕'); x.type = 'button'; x.onclick = closePop; head.appendChild(x);
    box.appendChild(head);
    pop.addEventListener('mousedown', function (e) { if (e.target === pop) closePop(); });
    document.addEventListener('keydown', function esc(e) { if (e.key === 'Escape') { closePop(); document.removeEventListener('keydown', esc); } });
    document.body.appendChild(pop);
    return box;
  }
  function field(form, name, label, type, auto) {
    var l = el('label', 'acct-field'); l.appendChild(el('span', null, label));
    var i = el('input'); i.name = name; i.type = type; i.autocomplete = auto; i.required = true; i.maxLength = 72;
    l.appendChild(i); form.appendChild(l); return i;
  }
  function loginDialog() {
    var box = modal(T('Anmelden', 'Sign in'));
    var f = el('form', 'acct-form'); box.appendChild(f);
    var u = field(f, 'username', T('Benutzername', 'User name'), 'text', 'username');
    var p = field(f, 'password', T('Passwort', 'Password'), 'password', 'current-password');
    var msg = el('div', 'acct-msg'); f.appendChild(msg);
    var go = el('button', 'btn btn-accent', T('Anmelden', 'Sign in')); go.type = 'submit'; f.appendChild(go);
    f.appendChild(el('p', 'acct-note', T('Ohne Anmeldung sind alle öffentlichen Bänder hörbar.', 'Public bands need no account.')));
    f.onsubmit = function (e) {
      e.preventDefault(); msg.textContent = ''; go.disabled = true;
      req('POST', 'api/auth/login', { username: u.value.trim(), password: p.value, scope: 'listen' }, function (code, r) {
        go.disabled = false;
        if (code === 200 && r && r.token) {
          save(r.token, r.user);
          if (r.user.must_change) return passwordDialog(p.value, true);
          location.reload();
        } else msg.textContent = (r && r.error) || T('Anmeldung fehlgeschlagen', 'Sign-in failed');
      });
    };
    setTimeout(function () { u.focus(); }, 30);
  }
  function passwordDialog(oldPw, forced) {
    var box = modal(forced ? T('Neues Passwort festlegen', 'Choose a new password') : T('Passwort ändern', 'Change password'));
    var f = el('form', 'acct-form'); box.appendChild(f);
    if (forced) f.appendChild(el('p', 'acct-note', T('Dein Passwort wurde vergeben oder zurückgesetzt – bitte ein eigenes wählen (mindestens 10 Zeichen).', 'Your password was set by an admin – please choose your own (at least 10 characters).')));
    var o = oldPw ? null : field(f, 'old', T('Bisheriges Passwort', 'Current password'), 'password', 'current-password');
    var n = field(f, 'new', T('Neues Passwort (mind. 10 Zeichen)', 'New password (min. 10 characters)'), 'password', 'new-password');
    var n2 = field(f, 'new2', T('Neues Passwort wiederholen', 'Repeat new password'), 'password', 'new-password');
    n.minLength = n2.minLength = 10;
    var msg = el('div', 'acct-msg'); f.appendChild(msg);
    var go = el('button', 'btn btn-accent', T('Speichern', 'Save')); go.type = 'submit'; f.appendChild(go);
    f.onsubmit = function (e) {
      e.preventDefault(); msg.textContent = '';
      if (n.value !== n2.value) { msg.textContent = T('Die beiden neuen Passwörter sind verschieden', 'Passwords differ'); return; }
      go.disabled = true;
      req('POST', 'api/auth/password', { old: oldPw || o.value, new: n.value }, function (code, r) {
        go.disabled = false;
        if (code === 200 && r && r.token) { save(r.token, r.user); location.reload(); }
        else msg.textContent = (r && r.error) || T('Nicht gespeichert', 'Not saved');
      });
    };
    setTimeout(function () { (o || n).focus(); }, 30);
  }
  function menu() {
    var box = modal(st.user.username);
    var m = el('div', 'acct-menu'); box.appendChild(m);
    if (st.user.role === 'admin') { var a = el('a', 'btn', T('Admin-Seite', 'Admin page')); a.href = base + 'admin/'; m.appendChild(a); }
    var pw = el('button', 'btn', T('Passwort ändern', 'Change password')); pw.type = 'button'; pw.onclick = function () { passwordDialog(null, false); }; m.appendChild(pw);
    var out = el('button', 'btn', T('Abmelden', 'Sign out')); out.type = 'button'; out.onclick = function () { save(null, null); location.reload(); }; m.appendChild(out);
  }

  /* Knopf nur, wenn es etwas zum Anmelden gibt (Mitglieder- oder Admin-Bänder, [ui] login) oder man schon angemeldet ist */
  function mount(show) {
    var right = document.querySelector('.topright'); if (!right || btn) return;
    if (!show && !st.token) return;
    btn = el('button', 'btn btn-ghost acct-btn'); btn.type = 'button'; btn.id = 'acctbtn';
    btn.onclick = function () { if (st.user) menu(); else loginDialog(); };
    var theme = document.getElementById('themebtn');
    right.insertBefore(btn, theme || null);
    paint();
  }
  var x = new XMLHttpRequest(); x.open('GET', base + 'ui.json?' + Date.now());
  x.onload = function () { var ui = {}; try { ui = JSON.parse(x.responseText) || {}; } catch (e) {} mount(ui.features && ui.features.login); };
  x.send();
})();
