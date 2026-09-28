#!/usr/bin/env python3
"""Sicherheitstest für Anmeldung, Rechte und Admin-Schnittstelle (docs/SECURITY.md) gegen einen laufenden Testserver.

  security_test.py http://127.0.0.1:8090 <Startpasswort von admin>

Erwartet eine frische Datenbank (Konto „admin“ mit Startpasswort), Bänder wie testdata/config-ui.toml mit
70cm-oben = Mitglieder (guest = false) und 10ghz = nur Admin, Decoder aprs-144800 (öffentlich) und sstv-145800
(nicht öffentlich), im Datenordner decoders/aprs-144800/direwolf.conf. Der Test ändert Konten und Konfiguration.
Am Ende sperrt er „admin“ absichtlich per Anmeldebremse (15 min).
"""
import http.client, json, sys, time, urllib.parse

BASE = sys.argv[1].rstrip("/")
START_PW = sys.argv[2]
U = urllib.parse.urlparse(BASE)
fails = 0


def req(method, path, body=None, token=None, headers=None, raw=False):
    c = http.client.HTTPConnection(U.hostname, U.port, timeout=20)
    h = {"Content-Type": "application/json"} if body is not None else {}
    if token: h["Authorization"] = "Bearer " + token
    h.update(headers or {})
    c.request(method, path, body=json.dumps(body) if body is not None else None, headers=h)
    r = c.getresponse()
    data = r.read()
    hdrs = {k.lower(): v for k, v in r.getheaders()}
    if raw: return r.status, data, hdrs
    try: j = json.loads(data or b"null")
    except Exception: j = None
    return r.status, j, hdrs


def check(name, ok, info=""):
    global fails
    print(("OK   " if ok else "FEHL ") + name + (f"  → {info}" if info != "" else ""))
    if not ok: fails += 1


def login(user, pw, scope="listen"):
    return req("POST", "/api/auth/login", {"username": user, "password": pw, "scope": scope})


def ws_status(band, token=None):
    q = f"?token={urllib.parse.quote(token)}" if token else ""
    c = http.client.HTTPConnection(U.hostname, U.port, timeout=10)
    c.request("GET", f"/ws/{band}{q}", headers={"Connection": "Upgrade", "Upgrade": "websocket", "Sec-WebSocket-Version": "13", "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ=="})
    s = c.getresponse().status
    c.close()
    return s


def bands_of(token=None):
    q = f"?token={urllib.parse.quote(token)}" if token else ""
    s, data, _ = req("GET", "/bandinfo.js" + q, raw=True)
    js = data.decode()
    arr = json.loads(js.split("var bandinfo = ", 1)[1].split(";\n", 1)[0])
    return sorted(b["name"] for b in arr)


# ---------- Köpfe, ohne Anmeldung ----------
s, _, h = req("GET", "/admin/", raw=True)
check("Admin-Seite erreichbar", s == 200, s)
csp = h.get("content-security-policy", "")
check("CSP streng (nur self, kein inline)", "script-src 'self'" in csp and "unsafe-inline" not in csp and "frame-ancestors 'none'" in csp, csp)
check("X-Frame-Options DENY, no-store", h.get("x-frame-options") == "DENY" and "no-store" in h.get("cache-control", ""))
check("nosniff überall", req("GET", "/", raw=True)[2].get("x-content-type-options") == "nosniff")
check("Admin-Schnittstelle ohne Token: 401", req("GET", "/api/admin/overview")[0] == 401)

# ---------- Anmelden ----------
t0 = time.time(); s1, j1, _ = login("admin", "falsch-falsch-1"); d1 = time.time() - t0
t0 = time.time(); s2, j2, _ = login("gibtesnicht", "falsch-falsch-1"); d2 = time.time() - t0
check("falsches Passwort: 401", s1 == 401, s1)
check("unbekannter Name: gleiche Meldung", s2 == 401 and j1 == j2, (j1, j2))
check("unbekannter Name: ähnliche Dauer (bcrypt läuft trotzdem)", d2 > 0.5 * d1, f"{d1:.3f}s / {d2:.3f}s")
s, j, _ = login("admin", START_PW, "admin")
check("Startpasswort: Anmeldung, muss ändern", s == 200 and j["user"]["must_change"], j)
tok0 = j["token"]
s, j, _ = req("GET", "/api/admin/overview", token=tok0)
check("vor Passwortwechsel keine Admin-Schnittstelle", s == 403 and j.get("must_change"), (s, j))
check("zu kurzes neues Passwort abgelehnt", req("POST", "/api/auth/password", {"old": START_PW, "new": "kurz"}, token=tok0)[0] == 400)
check("falsches altes Passwort abgelehnt", req("POST", "/api/auth/password", {"old": "nicht-das-alte", "new": "ein-langes-neues-passwort"}, token=tok0)[0] == 401)
ADMIN_PW = "Sicheres-Admin-Passwort-2026"
s, j, _ = req("POST", "/api/auth/password", {"old": START_PW, "new": ADMIN_PW}, token=tok0)
check("Passwort geändert, neues Token", s == 200 and j.get("token"), s)
admin = j["token"]
check("altes Token nach Passwortwechsel ungültig", req("GET", "/api/auth/me", token=tok0)[0] == 401)
s, ov, _ = req("GET", "/api/admin/overview", token=admin)
check("Admin-Überblick", s == 200 and "bands" in ov, s)
check("Admin-Token nicht per ?token=", req("GET", "/api/admin/overview?token=" + urllib.parse.quote(admin))[0] == 401)
s, j, _ = login("admin", ADMIN_PW, "listen")
admin_listen = j["token"]
check("Hören-Token eines Admins öffnet die Admin-Schnittstelle nicht", req("GET", "/api/admin/overview", token=admin_listen)[0] == 403)
check("manipuliertes Token abgelehnt", req("GET", "/api/admin/overview", token=admin[:-3] + ("AAA" if admin[-3:] != "AAA" else "BBB"))[0] == 401)

# ---------- Benutzer und Rechte ----------
s, j, _ = req("POST", "/api/admin/users", {"username": "dl1abc", "role": "user", "bands": ["70cm-oben"], "decoders": ["sstv-145800"]}, token=admin)
check("Benutzer angelegt, Passwort erzeugt", s == 200 and j.get("password") and len(j["password"]) == 16, s)
uid, upw = j["id"], j["password"]
check("ungültiger Name abgelehnt", req("POST", "/api/admin/users", {"username": "<script>", "role": "user"}, token=admin)[0] == 400)
check("unbekannte Rolle abgelehnt", req("POST", "/api/admin/users", {"username": "dk0xyz", "role": "root"}, token=admin)[0] == 400)
check("doppelter Name (Groß/klein) abgelehnt", req("POST", "/api/admin/users", {"username": "DL1ABC", "role": "user"}, token=admin)[0] == 400)
s, j, _ = login("dl1abc", upw)
check("Nutzer muss Passwort ändern", s == 200 and j["user"]["must_change"])
s, j, _ = req("POST", "/api/auth/password", {"old": upw, "new": "Nutzer-Passwort-lang"}, token=j["token"])
user = j["token"]
check("Nutzer hat eigenes Passwort", s == 200)
check("Nutzer-Token öffnet Admin-Schnittstelle nicht", req("GET", "/api/admin/overview", token=user)[0] in (401, 403))
check("Nutzer bekommt kein Admin-Token", login("dl1abc", "Nutzer-Passwort-lang", "admin")[0] == 403)

g, u, a = bands_of(), bands_of(user), bands_of(admin_listen)
check("Gast sieht nur öffentliche Bänder", "70cm-oben" not in g and "10ghz" not in g and "2m" in g, g)
check("Nutzer sieht sein Mitglieder-Band, nicht das Admin-Band", "70cm-oben" in u and "10ghz" not in u, u)
check("Admin sieht alle Bänder", "70cm-oben" in a and "10ghz" in a, a)
check("WebSocket: Gast am Mitglieder-Band 403", ws_status("70cm-oben") == 403)
check("WebSocket: Gast am öffentlichen Band 101", ws_status("2m") == 101)
check("WebSocket: Nutzer am Mitglieder-Band 101", ws_status("70cm-oben", user) == 101)
check("WebSocket: Nutzer am Admin-Band 403", ws_status("10ghz", user) == 403)
check("WebSocket: ungültiges Token 401", ws_status("2m", "kaputt") == 401)

dg = [d["id"] for d in req("GET", "/api/decoders")[1]["decoders"]]
du = [d["id"] for d in req("GET", "/api/decoders", token=user)[1]["decoders"]]
check("Gast sieht nur öffentliche Decoder", "aprs-144800" in dg and "sstv-145800" not in dg, dg)
check("Nutzer sieht seinen nicht öffentlichen Decoder", "sstv-145800" in du, du)
check("direwolf.conf nicht abrufbar", req("GET", "/api/decoders/files/aprs-144800/direwolf.conf", raw=True)[0] == 404)
check("Pfad mit .. nicht abrufbar", req("GET", "/api/decoders/files/aprs-144800/..%2F..%2Fconfig.toml", raw=True)[0] == 404)
check("aprs.json abrufbar", req("GET", "/api/decoders/files/aprs-144800/aprs.json", raw=True)[0] == 200)

me_id = [x for x in req("GET", "/api/admin/users", token=admin)[1]["users"] if x["username"] == "admin"][0]["id"]
check("eigenes Konto: herabstufen abgelehnt", req("PUT", f"/api/admin/users/{me_id}", {"role": "user"}, token=admin)[0] == 400)
check("eigenes Konto: löschen abgelehnt", req("DELETE", f"/api/admin/users/{me_id}", token=admin)[0] == 400)
s, _, _ = req("PUT", f"/api/admin/users/{uid}", {"active": False}, token=admin)
check("Nutzer gesperrt: Token sofort ungültig", s == 200 and req("GET", "/api/auth/me", token=user)[0] == 401)
check("gesperrter Nutzer kann sich nicht anmelden", login("dl1abc", "Nutzer-Passwort-lang")[0] == 401)
s, j, _ = req("POST", f"/api/admin/users/{uid}/password", {}, token=admin)
check("Passwort zurücksetzen liefert neues Passwort", s == 200 and len(j.get("password", "")) == 16)

# ---------- Konfiguration ----------
s, c, _ = req("GET", "/api/admin/config", token=admin)
check("Konfiguration lesbar", s == 200 and c.get("hash") and c.get("text"), s)
h0 = c["hash"]
s, j, _ = req("PUT", "/api/admin/config", {"hash": h0, "ops": [{"op": "set", "path": ["station", "name"], "value": "Sicherheitstest"}]}, token=admin)
check("Formular-Änderung gespeichert, Neustart ausstehend", s == 200 and j.get("restart_pending"), (s, j))
h1 = j.get("hash")
check("alte Prüfsumme: 409", req("PUT", "/api/admin/config", {"hash": h0, "ops": [{"op": "set", "path": ["station", "name"], "value": "x"}]}, token=admin)[0] == 409)
check("gesperrter Pfad per Formular abgelehnt", req("PUT", "/api/admin/config", {"hash": h1, "ops": [{"op": "set", "path": ["plugin_dir"], "value": "/tmp"}]}, token=admin)[0] == 400)
check("Befehl im Decoder abgelehnt", req("PUT", "/api/admin/config", {"hash": h1, "ops": [{"op": "append", "array": "decoders", "table": {"plugin": "aprs", "freq": 144800000, "command": "sh"}}]}, token=admin)[0] == 400)
txt = req("GET", "/api/admin/config", token=admin)[1]["text"]
check("Port per Text geändert: 422", req("PUT", "/api/admin/config", {"hash": h1, "text": txt.replace("port = 8090", "port = 80")}, token=admin)[0] == 422)
check("plugin_dir per Text geändert: 422", req("PUT", "/api/admin/config", {"hash": h1, "text": 'plugin_dir = "/tmp"\n' + txt.replace('plugin_dir = ', '# alt: ')}, token=admin)[0] == 422)
s, j, _ = req("POST", "/api/admin/config/check", {"text": txt + '\nirgendwas = 1\n'}, token=admin)
check("unbekannter Schlüssel: Hinweis", s == 200 and any("irgendwas" in w for w in j.get("warnings", [])), j)
check("Syntaxfehler: 422", req("PUT", "/api/admin/config", {"hash": h1, "text": txt + "\n[[bands]\n"}, token=admin)[0] == 422)
check("Kommentare bleiben erhalten", "# Testkonfiguration" in req("GET", "/api/admin/config", token=admin)[1]["text"])
bk = req("GET", "/api/admin/config", token=admin)[1]["backups"]
check("Sicherung angelegt", len(bk) >= 1, bk)
check("Sicherung mit Pfad nicht lesbar", req("GET", "/api/admin/config/backups/..%2Fsec.toml", token=admin)[0] == 404)
check("zu große Datei abgelehnt", req("PUT", "/api/admin/config", {"hash": h1, "text": txt + "#" * 600000}, token=admin)[0] in (413, 422))
check("Chat-Liste für Admin", req("GET", "/api/admin/chat", token=admin)[0] == 200)
check("Seriennummer mit Shell-Zeichen abgelehnt", req("POST", "/api/admin/devices/serial", {"index": 0, "serial": "a;reboot"}, token=admin)[0] == 400)

# ---------- Anmeldebremse (zuletzt: sperrt admin für 15 min) ----------
codes = [login("admin", f"falsch-{i}-xxxxx")[0] for i in range(10)]
s, j, h = login("admin", ADMIN_PW, "admin")
check("nach 10 Fehlversuchen: auch richtiges Passwort 429", codes.count(401) == 10 and s == 429 and "retry-after" in h, (codes[-3:], s))
check("anderer Name von derselben Adresse noch möglich", login("dl1abc", "falsch-passwort-x")[0] == 401)

print(f"\n{'ALLES IN ORDNUNG' if not fails else str(fails) + ' FEHLER'}")
sys.exit(1 if fails else 0)
