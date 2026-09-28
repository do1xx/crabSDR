# crabSDR – Sicherheit von Anmeldung und Admin-Seite

Die Admin-Seite (`/admin/`) ist aus dem Internet erreichbar. Wer sich dort als Admin anmeldet, kann die Station
vollständig steuern. Deshalb gelten diese Regeln; jede Änderung an Anmeldung und Admin-Schnittstelle muss sie einhalten.

## Konten und Rollen

| Rolle | darf |
|---|---|
| Gast (ohne Anmeldung) | öffentliche Bänder hören, öffentliche Decoder sehen, Chat und Logbuch |
| Nutzer | zusätzlich die ihm zugeteilten Mitglieder-Bänder und nicht öffentlichen Decoder („alle“ oder einzelne) |
| Admin | alles, dazu die Admin-Seite |

Zugang je Band (`[[bands]]`): **öffentlich** (`guest = true`), **Mitglieder** (`guest = false`), **nur Admin**
(`admin_only = true`). Decoder mit `public = false` sehen nur Admins und Nutzer, denen der Decoder zugeteilt ist.
Der Server prüft das bei jeder Verbindung und jeder Abfrage; die Oberfläche blendet nur aus.

## Passwörter

- bcrypt (Kosten 12), geprüft in einem eigenen Thread (der Server hängt nicht während der Prüfung).
- Mindestens 10 Zeichen, höchstens 72 Byte (bcrypt-Grenze, längere werden abgelehnt statt still gekürzt).
- Das Startpasswort des ersten Admins kommt aus dem Zufallsgenerator des Betriebssystems (16 Zeichen) und muss
  beim ersten Anmelden geändert werden. Setzt ein Admin das Passwort eines anderen zurück, muss auch dieser es ändern.
- Vergessen: `crabsdr-server --reset-admin [config.toml]` auf dem Rechner (braucht Zugang zur Maschine) setzt ein
  neues Zufallspasswort für den Benutzer `admin` und meldet alle seine Sitzungen ab.

## Anmelden

- Unbekannter Name und falsches Passwort dauern gleich lange und liefern dieselbe Meldung.
- Bremse: je Benutzername höchstens 10 Fehlversuche in 15 Minuten, je Absender-Adresse 20; insgesamt höchstens
  60 Fehlversuche je Minute. Danach „zu viele Versuche“ (HTTP 429) bis das Fenster abläuft. Die Sperre je Name
  wirkt auch dann, wenn ein Angreifer seine Adresse fälscht oder wechselt.
- Gesperrte (deaktivierte) Konten können sich nicht anmelden.

## Sitzungen (Token)

- JWT, HS256, Schlüssel 256 Bit aus dem Zufallsgenerator, Datei `data_dir/.jwt_secret` mit Rechten 0600
  (oder Umgebungsvariable `CRABSDR_JWT_SECRET`).
- Zwei Arten: **Hören** (30 Tage, Hören-Seite) und **Admin** (8 Stunden, nur Admin-Seite, nur für Admins).
  Ein Hören-Token öffnet die Admin-Schnittstelle nie, auch nicht bei einem Admin-Konto.
- Rechte stehen nicht im Token, sondern werden bei jeder Anfrage aus der Datenbank gelesen: Änderungen wirken sofort.
- Jedes Konto hat einen Sitzungszähler im Token. Passwortwechsel, Sperren, Rollenwechsel und Löschen erhöhen ihn –
  alle alten Sitzungen sind sofort ungültig.
- Die Admin-Seite hält ihr Token nur im `sessionStorage` des Tabs; die Hören-Seite ihr eigenes im `localStorage`.
- Admin-Anfragen nehmen das Token nur aus dem Kopf `Authorization: Bearer …`, nie aus der Adresse (Adressen landen
  in Protokollen). Daher auch kein CSRF: Browser senden diesen Kopf nicht von selbst.
- Hören-Tokens stehen dagegen in Adressen (WebSocket, `bandinfo.js`, Bilder nicht öffentlicher Decoder), weil Browser
  dort keinen Kopf setzen können. Sie öffnen nur Bänder und Decoder, nie die Admin-Schnittstelle; Protokolle eines
  vorgeschalteten Webservers sollten Abfrageparameter nicht speichern.

## Admin-Seite

- Eigene Dateien aus der Oberfläche (`web/admin/`); der Stationsordner kann sie nicht ersetzen.
- Strenge Content-Security-Policy (nur eigene Skripte, kein Inline-Code, keine fremden Quellen), keine Einbettung in
  fremde Seiten (`frame-ancestors 'none'`), `no-store`, `no-referrer`. Alle Texte werden als Text eingesetzt, nie
  als HTML.
- Ein Admin kann das eigene Konto nicht löschen, sperren oder herabstufen; der letzte aktive Admin bleibt immer.
- Jede Änderung steht mit Benutzername im Protokoll (Journal).

## Konfiguration über die Admin-Seite

- Formulare ändern nur eine feste Liste von Einstellungen (Station, Oberfläche, Bänder, Decoder); die Textansicht
  zeigt die ganze Datei.
- **Pfade, Port und Schlüsselquellen** (`port`, `frontend_dir`, `plugin_dir`, `data_dir`, `db_path`, `site_dir`,
  `jwt_secret_env`, `[site]`, `mqtt.password_file`) sind über die Admin-Seite nicht änderbar – nur in der Datei am Rechner.
  So kann auch ein gestohlenes Admin-Passwort keine fremden Programme als Decoder unterschieben. Ebenso gesperrt:
  Decoder-Optionen mit Dateipfaden (`logdir`, `json`, `log`, `out`, `igate_passfile`, alles auf `_file`/`_dir`/`_path`) –
  sonst ließen sich Dateien der Station überschreiben oder geheime Dateien über einen Decoder nach außen schicken.
- Gespeichert wird nur, was die volle Prüfung (`--check`) besteht. Vorher Sicherung nach
  `data_dir/config-backups/` (die letzten 20), dann atomar ersetzt; Kommentare und Reihenfolge bleiben erhalten.
- Hat sich die Datei inzwischen geändert (anderer Admin, Einspielen per Skript), wird nicht überschrieben (409).
- Neustart über die Admin-Seite nur, wenn die gespeicherte Datei lädt: der Server beendet sich, systemd bzw. Docker
  starten ihn neu. Ohne Dienstverwalter wird der Neustart abgelehnt (der Server bliebe sonst einfach stehen).
- Größe der Datei höchstens 512 KiB.

## Prüfen

`tools/e2e/security_test.py` prüft jede Regel dieser Seite gegen einen laufenden Server, `tools/e2e/admin.spec.mjs` die
Admin-Seite im Browser (auch auf Verstöße gegen die CSP). Beide laufen in `tools/e2e/run.sh` mit.
