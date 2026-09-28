//! Admin-Schnittstelle `/api/admin/…` (docs/SECURITY.md). Jede Funktion beginnt mit `require_admin`: Admin-Sitzung
//! eines aktiven Admin-Kontos, Passwort bereits geändert. Jede Änderung steht mit Benutzername im Protokoll.

use axum::{
    extract::{Path as AxPath, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use crabsdr_auth::{ROLE_ADMIN, ROLE_USER};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tracing::{info, warn};

use crate::access::require_admin;
use crate::confedit;
use crate::AppState;

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, [(header::CACHE_CONTROL, "no-store")], Json(json!({"error": msg.into()}))).into_response()
}
fn ok(v: Value) -> Response { ([(header::CACHE_CONTROL, "no-store")], Json(v)).into_response() }

macro_rules! admin {
    ($state:expr, $headers:expr) => {
        match require_admin(&$state, &$headers).await { Ok(p) => p, Err(r) => return r }
    };
}

/// bcrypt im eigenen Thread, ohne die Datenbank zu sperren
async fn hash_async(pw: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || crabsdr_auth::hash_password(&pw).map_err(|e| e.to_string())).await.map_err(|_| "Hashen fehlgeschlagen".to_string())?
}

/// Blockierende Datenbankarbeit im eigenen Thread
async fn with_db<T: Send + 'static>(state: &AppState, f: impl FnOnce(&crabsdr_auth::AuthDb) -> T + Send + 'static) -> Option<T> {
    let db = state.auth.as_ref()?.db.clone();
    tokio::task::spawn_blocking(move || { let g = db.lock().unwrap(); f(&g) }).await.ok()
}

// ---------------- Überblick ----------------

pub async fn overview(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let _p = admin!(state, headers);
    let cfg = state.config.read().await.clone();
    let manager = state.manager.read().await;
    let mut bands = Vec::new();
    for b in &cfg.sdrs {
        let (running, error, clients, listeners) = match manager.get(&b.id) {
            Some(p) => {
                let s = p.status.read().await;
                let c = p.clients.lock().await;
                (s.running, s.error.clone(), c.count_real(), c.listeners_json(&b.id))
            }
            None => (false, Some(if b.enabled { "nicht gestartet".into() } else { "aus".into() }), 0, vec![]),
        };
        bands.push(json!({"id": b.id, "label": b.label, "enabled": b.enabled, "running": running, "error": error, "clients": clients,
            "listeners": listeners, "center_freq": b.center_freq, "sample_rate": b.sample_rate, "gain": b.gain, "driver": b.sdr_driver,
            "access": access_name(b.guest, b.admin_only)}));
    }
    drop(manager);
    let load = std::fs::read_to_string("/proc/loadavg").ok().map(|s| s.split_whitespace().take(3).map(|x| x.parse::<f32>().unwrap_or(0.0)).collect::<Vec<_>>());
    let mem = std::fs::read_to_string("/proc/meminfo").ok().map(|s| {
        let f = |k: &str| s.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        json!({"total_mb": f("MemTotal:") / 1024, "available_mb": f("MemAvailable:") / 1024})
    });
    let temp = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp").ok().and_then(|s| s.trim().parse::<f32>().ok()).map(|t| (t / 100.0).round() / 10.0);
    let disks = disk_usage(&[std::path::Path::new("/"), &cfg.data_dir]).await;
    ok(json!({
        "version": crate::version_long(),
        "uptime_s": crate::uptime_s(),
        "station": cfg.station.name,
        "config_path": cfg.source,
        "restart_pending": state.restart_pending.load(Ordering::Relaxed),
        "can_restart": supervised(),
        "system": {"load": load, "memory": mem, "temp_c": temp, "disks": disks},
        "bands": bands,
        "decoders": state.decoders.admin_list().await,
    }))
}

/// Belegung der Dateisysteme unter den Pfaden (df -Pk, POSIX; je Dateisystem einmal). Voll laufende Platten
/// (Protokolle, Aufnahmen) sollen auf der Admin-Seite auffallen, bevor etwas stehen bleibt.
async fn disk_usage(paths: &[&std::path::Path]) -> Vec<serde_json::Value> {
    let mut cmd = tokio::process::Command::new("df");
    cmd.arg("-Pk").args(paths.iter().filter(|p| p.exists())).kill_on_drop(true);
    let out = match tokio::time::timeout(std::time::Duration::from_secs(3), cmd.output()).await {
        Ok(Ok(o)) => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return vec![],
    };
    parse_df(&out)
}

fn parse_df(out: &str) -> Vec<serde_json::Value> {
    let mut seen = std::collections::HashSet::new();
    out.lines().skip(1).filter_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 6 { return None; }
        let mount = f[5..].join(" ");
        if !seen.insert(mount.clone()) { return None; }
        let (total, used, avail) = (f[1].parse::<u64>().ok()?, f[2].parse::<u64>().ok()?, f[3].parse::<u64>().ok()?);
        let pct = if used + avail > 0 { (used * 100 + used + avail - 1) / (used + avail) } else { 0 };   // aufgerundet wie df
        Some(json!({"mount": mount, "total_mb": total / 1024, "used_mb": used / 1024, "free_mb": avail / 1024, "percent": pct}))
    }).collect()
}

pub fn access_name(guest: bool, admin_only: bool) -> &'static str {
    if admin_only { "admin" } else if guest { "public" } else { "members" }
}

// ---------------- Benutzer ----------------

pub async fn list_users(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let _p = admin!(state, headers);
    match with_db(&state, |db| db.list()).await {
        Some(Ok(u)) => ok(json!({"users": u})),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "Benutzer nicht lesbar"),
    }
}

#[derive(Deserialize)]
pub struct NewUser {
    pub username: String,
    #[serde(default)] pub password: Option<String>,
    #[serde(default = "default_role")] pub role: String,
    #[serde(default)] pub bands: Vec<String>,
    #[serde(default)] pub decoders: Vec<String>,
}
fn default_role() -> String { ROLE_USER.into() }

/// Ohne Passwort wird eines erzeugt (einmal in der Antwort); das Konto muss es beim ersten Anmelden ändern.
pub async fn create_user(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<NewUser>) -> Response {
    let p = admin!(state, headers);
    let generated = req.password.as_deref().map_or(true, |s| s.is_empty());
    let pw = if generated { crabsdr_auth::random_password(16) } else { req.password.clone().unwrap() };
    if let Err(e) = crabsdr_auth::validate_username(&req.username).and(crabsdr_auth::validate_role(&req.role)).and(crabsdr_auth::validate_password(&pw)) {
        return err(StatusCode::BAD_REQUEST, e.to_string());
    }
    let hash = match hash_async(pw.clone()).await { Ok(h) => h, Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e) };
    let (name, role, bands, decs) = (req.username.clone(), req.role.clone(), req.bands.clone(), req.decoders.clone());
    let res = with_db(&state, move |db| -> Result<String, String> {
        let id = db.create_user(&name, &hash, &role, true).map_err(|e| e.to_string())?;
        db.set_bands(&id, &bands).map_err(|e| e.to_string())?;
        db.set_decoders(&id, &decs).map_err(|e| e.to_string())?;
        Ok(id)
    }).await;
    match res {
        Some(Ok(id)) => {
            info!("Admin „{}“: Konto „{}“ ({}) angelegt", p.username, req.username, req.role);
            // ein erzeugtes Passwort steht nur in dieser Antwort; es muss beim ersten Anmelden geändert werden
            ok(json!({"id": id, "password": if generated { Some(pw) } else { None }}))
        }
        Some(Err(e)) => err(StatusCode::BAD_REQUEST, e),
        None => err(StatusCode::SERVICE_UNAVAILABLE, "Anmeldung nicht verfügbar"),
    }
}

#[derive(Deserialize)]
pub struct EditUser {
    #[serde(default)] pub username: Option<String>,
    #[serde(default)] pub role: Option<String>,
    #[serde(default)] pub active: Option<bool>,
    #[serde(default)] pub bands: Option<Vec<String>>,
    #[serde(default)] pub decoders: Option<Vec<String>>,
}

pub async fn update_user(State(state): State<Arc<AppState>>, headers: HeaderMap, AxPath(id): AxPath<String>, Json(req): Json<EditUser>) -> Response {
    let p = admin!(state, headers);
    let me = p.id.clone();
    let (id2, r2) = (id.clone(), (req.username.clone(), req.role.clone(), req.active, req.bands.clone(), req.decoders.clone()));
    let res = with_db(&state, move |db| -> Result<String, String> {
        let u = db.get_user(&id2).map_err(|e| e.to_string())?.ok_or("Konto gibt es nicht")?;
        let (name, role, active, bands, decs) = r2;
        let demote = role.as_deref().map_or(false, |r| r != ROLE_ADMIN) && u.role == ROLE_ADMIN;
        let disable = active == Some(false) && u.active;
        if id2 == me && (demote || disable) { return Err("Das eigene Konto lässt sich nicht herabstufen oder sperren".into()); }
        if u.role == ROLE_ADMIN && u.active && (demote || disable) && db.count_active_admins().map_err(|e| e.to_string())? <= 1 {
            return Err("Der letzte aktive Admin muss bleiben".into());
        }
        if let Some(n) = name.filter(|n| *n != u.username) { db.rename(&id2, &n).map_err(|e| e.to_string())?; }
        if let Some(r) = role.filter(|r| *r != u.role) { db.set_role(&id2, &r).map_err(|e| e.to_string())?; }
        if let Some(a) = active.filter(|a| *a != u.active) { db.set_active(&id2, a).map_err(|e| e.to_string())?; }
        if let Some(b) = bands { db.set_bands(&id2, &b).map_err(|e| e.to_string())?; }
        if let Some(d) = decs { db.set_decoders(&id2, &d).map_err(|e| e.to_string())?; }
        Ok(u.username)
    }).await;
    match res {
        Some(Ok(name)) => { info!("Admin „{}“: Konto „{}“ geändert", p.username, name); ok(json!({"ok": true})) }
        Some(Err(e)) => err(StatusCode::BAD_REQUEST, e),
        None => err(StatusCode::SERVICE_UNAVAILABLE, "Anmeldung nicht verfügbar"),
    }
}

#[derive(Deserialize, Default)]
pub struct ResetPw { #[serde(default)] pub password: Option<String> }

/// Passwort zurücksetzen: vorgegeben oder erzeugt; das Konto muss es beim nächsten Anmelden ändern, alle Sitzungen ab.
pub async fn reset_password(State(state): State<Arc<AppState>>, headers: HeaderMap, AxPath(id): AxPath<String>, body: Option<Json<ResetPw>>) -> Response {
    let p = admin!(state, headers);
    let given = body.and_then(|b| b.0.password).filter(|s| !s.is_empty());
    let generated = given.is_none();
    let pw = given.unwrap_or_else(|| crabsdr_auth::random_password(16));
    if let Err(e) = crabsdr_auth::validate_password(&pw) { return err(StatusCode::BAD_REQUEST, e.to_string()); }
    let hash = match hash_async(pw.clone()).await { Ok(h) => h, Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e) };
    let id2 = id.clone();
    let res = with_db(&state, move |db| -> Result<String, String> {
        let u = db.get_user(&id2).map_err(|e| e.to_string())?.ok_or("Konto gibt es nicht")?;
        db.set_password(&id2, &hash, true).map_err(|e| e.to_string())?;
        Ok(u.username)
    }).await;
    match res {
        Some(Ok(name)) => {
            info!("Admin „{}“: Passwort von „{}“ zurückgesetzt", p.username, name);
            ok(json!({"ok": true, "password": if generated { Some(pw) } else { None }}))
        }
        Some(Err(e)) => err(StatusCode::BAD_REQUEST, e),
        None => err(StatusCode::SERVICE_UNAVAILABLE, "Anmeldung nicht verfügbar"),
    }
}

pub async fn delete_user(State(state): State<Arc<AppState>>, headers: HeaderMap, AxPath(id): AxPath<String>) -> Response {
    let p = admin!(state, headers);
    if id == p.id { return err(StatusCode::BAD_REQUEST, "Das eigene Konto lässt sich nicht löschen"); }
    let id2 = id.clone();
    let res = with_db(&state, move |db| -> Result<String, String> {
        let u = db.get_user(&id2).map_err(|e| e.to_string())?.ok_or("Konto gibt es nicht")?;
        if u.role == ROLE_ADMIN && u.active && db.count_active_admins().map_err(|e| e.to_string())? <= 1 { return Err("Der letzte aktive Admin muss bleiben".into()); }
        db.delete_user(&id2).map_err(|e| e.to_string())?;
        Ok(u.username)
    }).await;
    match res {
        Some(Ok(name)) => { info!("Admin „{}“: Konto „{}“ gelöscht", p.username, name); ok(json!({"ok": true})) }
        Some(Err(e)) => err(StatusCode::BAD_REQUEST, e),
        None => err(StatusCode::SERVICE_UNAVAILABLE, "Anmeldung nicht verfügbar"),
    }
}

// ---------------- Konfiguration ----------------

async fn config_path(state: &AppState) -> Result<std::path::PathBuf, Response> {
    state.config.read().await.source.clone().ok_or_else(|| err(StatusCode::CONFLICT, "crabSDR läuft ohne Konfigurationsdatei"))
}
async fn backup_dir(state: &AppState) -> std::path::PathBuf { state.config.read().await.data_dir.join("config-backups") }

pub async fn get_config(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let _p = admin!(state, headers);
    let path = match config_path(&state).await { Ok(p) => p, Err(r) => return r };
    let text = match std::fs::read_to_string(&path) { Ok(t) => t, Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{}: {}", path.display(), e)) };
    let dir = path.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let writable = std::fs::metadata(&path).map(|m| !m.permissions().readonly()).unwrap_or(false) && dir_writable(&dir);
    let backups: Vec<Value> = confedit::list_backups(&backup_dir(&state).await).into_iter().map(|(n, s)| json!({"name": n, "bytes": s})).collect();
    // für die Formulare: gelesene Einstellungen (Reihenfolge wie in der Datei) und installierte Plugins
    let parsed = crabsdr_core::ServerConfig::from_content(&text, &path).ok().map(|l| {
        let mut v = serde_json::to_value(&l.config).unwrap_or(Value::Null);
        let ids: Vec<String> = l.config.decoders.iter().map(|d| d.id.clone().unwrap_or_else(|| format!("{}-{}", d.plugin, d.freq / 1000))).collect();
        v["decoder_ids"] = json!(ids);
        v["band_file_order"] = json!(text.contains("[[bands]]") || text.contains("[[sdrs]]"));
        v
    });
    let plugin_dir = state.config.read().await.plugin_dir.clone();
    let mut plugins: Vec<Value> = std::fs::read_dir(&plugin_dir).into_iter().flatten().flatten().filter_map(|e| {
        let m: Value = serde_json::from_str(&std::fs::read_to_string(e.path().join("decoder.json")).ok()?).ok()?;
        Some(json!({"name": e.file_name().to_string_lossy(), "label": m["label"], "description": m["description"], "requires": m["requires"]}))
    }).collect();
    plugins.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    ok(json!({"path": path, "hash": confedit::digest(&text), "text": text, "writable": writable, "locked": confedit::LOCKED, "backups": backups,
        "parsed": parsed, "plugins": plugins}))
}

fn dir_writable(dir: &std::path::Path) -> bool {
    let t = dir.join(format!(".crabsdr-schreibtest-{}", crabsdr_auth::random_password(6)));
    let ok = std::fs::write(&t, b"").is_ok();
    let _ = std::fs::remove_file(&t);
    ok
}

#[derive(Deserialize)]
pub struct CheckReq { pub text: String }

pub async fn check_config(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<CheckReq>) -> Response {
    let _p = admin!(state, headers);
    let path = match config_path(&state).await { Ok(p) => p, Err(r) => return r };
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let r = tokio::task::spawn_blocking(move || confedit::check(&path, &current, &req.text)).await.unwrap_or_default();
    ok(json!(r))
}

#[derive(Deserialize)]
pub struct SaveReq { pub hash: String, #[serde(default)] pub text: Option<String>, #[serde(default)] pub ops: Option<Vec<confedit::Op>> }

/// Speichern: ganze Datei (`text`) oder Änderungen aus Formularen (`ops`); immer mit der Prüfsumme des gelesenen Stands
pub async fn save_config(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<SaveReq>) -> Response {
    let p = admin!(state, headers);
    let path = match config_path(&state).await { Ok(p) => p, Err(r) => return r };
    let bdir = backup_dir(&state).await;
    let _lock = state.config_lock.lock().await;
    let current = match std::fs::read_to_string(&path) { Ok(t) => t, Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()) };
    if confedit::digest(&current) != req.hash {
        return err(StatusCode::CONFLICT, "Die Datei wurde inzwischen geändert – bitte neu laden");
    }
    let text = match (&req.text, &req.ops) {
        (Some(t), None) => t.clone(),
        (None, Some(ops)) => match confedit::apply(&current, ops) { Ok(t) => t, Err(e) => return err(StatusCode::BAD_REQUEST, e) },
        _ => return err(StatusCode::BAD_REQUEST, "text oder ops erwartet"),
    };
    let (path2, hash) = (path.clone(), req.hash.clone());
    let res = tokio::task::spawn_blocking(move || confedit::save(&path2, &bdir, &hash, &text)).await;
    match res {
        Ok(Ok((h, check))) => {
            if h != req.hash { state.restart_pending.store(true, Ordering::Relaxed); }
            info!("Admin „{}“: Konfiguration gespeichert ({})", p.username, path.display());
            ok(json!({"ok": true, "hash": h, "warnings": check.warnings, "restart_pending": state.restart_pending.load(Ordering::Relaxed)}))
        }
        Ok(Err(confedit::SaveError::Conflict)) => err(StatusCode::CONFLICT, "Die Datei wurde inzwischen geändert – bitte neu laden"),
        Ok(Err(confedit::SaveError::Invalid(c))) => (StatusCode::UNPROCESSABLE_ENTITY, [(header::CACHE_CONTROL, "no-store")],
            Json(json!({"error": "Prüfung fehlgeschlagen – nichts gespeichert", "errors": c.errors, "warnings": c.warnings}))).into_response(),
        Ok(Err(confedit::SaveError::ReadOnly(m))) => err(StatusCode::FORBIDDEN, m),
        Ok(Err(confedit::SaveError::Io(m))) => err(StatusCode::INTERNAL_SERVER_ERROR, m),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "Speichern fehlgeschlagen"),
    }
}

pub async fn get_backup(State(state): State<Arc<AppState>>, headers: HeaderMap, AxPath(name): AxPath<String>) -> Response {
    let _p = admin!(state, headers);
    match confedit::backup_path(&backup_dir(&state).await, &name).and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(t) => ok(json!({"name": name, "text": t})),
        None => err(StatusCode::NOT_FOUND, "Sicherung gibt es nicht"),
    }
}

/// Neu starten läuft über den Dienstverwalter (systemd, Docker): der Server beendet sich, der Verwalter startet neu.
/// Ohne Verwalter würde er einfach stehen bleiben – dann wird abgelehnt.
pub fn supervised() -> bool {
    std::env::var_os("INVOCATION_ID").is_some() || std::path::Path::new("/.dockerenv").exists() || std::env::var_os("CRABSDR_SUPERVISED").is_some()
}

pub async fn restart(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let p = admin!(state, headers);
    if !supervised() { return err(StatusCode::CONFLICT, "crabSDR läuft ohne Dienstverwalter (systemd/Docker) – bitte von Hand neu starten"); }
    let path = state.config.read().await.source.clone();
    if let Err(e) = crabsdr_core::ServerConfig::load_checked(path.as_deref()) {
        return err(StatusCode::UNPROCESSABLE_ENTITY, format!("Konfiguration lädt nicht – kein Neustart: {}", e));
    }
    warn!("Admin „{}“: Neustart", p.username);
    tokio::spawn(async { tokio::time::sleep(std::time::Duration::from_millis(400)).await; std::process::exit(0); });
    ok(json!({"ok": true}))
}

// ---------------- Bänder live ----------------

#[derive(Deserialize)]
pub struct GainReq { pub gain: f64 }

/// Verstärkung sofort setzen (ohne Speichern) – zum Ausprobieren; bleibend über die Konfiguration
pub async fn live_gain(State(state): State<Arc<AppState>>, headers: HeaderMap, AxPath(id): AxPath<String>, Json(req): Json<GainReq>) -> Response {
    let p = admin!(state, headers);
    if !(0.0..=100.0).contains(&req.gain) { return err(StatusCode::BAD_REQUEST, "Verstärkung 0–100 dB"); }
    let manager = state.manager.read().await;
    let Some(pipe) = manager.get(&id) else { return err(StatusCode::NOT_FOUND, "Band gibt es nicht") };
    if pipe.sdr_cmd_tx.send(crabsdr_sdr::DriverCommand::SetGain(req.gain)).await.is_err() { return err(StatusCode::SERVICE_UNAVAILABLE, "Quelle antwortet nicht"); }
    pipe.config.write().await.gain = req.gain;
    info!("Admin „{}“: Band {} Verstärkung live {} dB", p.username, id, req.gain);
    ok(json!({"ok": true}))
}

// ---------------- Chat und Logbuch ----------------

#[derive(Deserialize)]
pub struct ListQ { pub n: Option<i64> }

pub async fn list_chat(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<ListQ>) -> Response {
    let _p = admin!(state, headers);
    ok(json!({"chat": state.chat.recent_chat(q.n.unwrap_or(200).clamp(1, 1000)), "log": state.chat.recent_log(q.n.unwrap_or(200).clamp(1, 1000))}))
}

pub async fn delete_chat(State(state): State<Arc<AppState>>, headers: HeaderMap, AxPath(id): AxPath<i64>) -> Response {
    let p = admin!(state, headers);
    if state.chat.delete_chat(id) { info!("Admin „{}“: Chat-Zeile {} gelöscht", p.username, id); ok(json!({"ok": true})) } else { err(StatusCode::NOT_FOUND, "gibt es nicht") }
}

pub async fn delete_log(State(state): State<Arc<AppState>>, headers: HeaderMap, AxPath(id): AxPath<i64>) -> Response {
    let p = admin!(state, headers);
    if state.chat.delete_log(id) { info!("Admin „{}“: Logbuch-Eintrag {} gelöscht", p.username, id); ok(json!({"ok": true})) } else { err(StatusCode::NOT_FOUND, "gibt es nicht") }
}

// ---------------- Sticks ----------------

fn rtl_index(all: &[crabsdr_sdr::probe::DetectedDevice], target: &crabsdr_sdr::probe::DetectedDevice) -> Option<u32> {
    let mut i = 0u32;
    for d in all { if std::ptr::eq(d, target) { return Some(i); } if d.driver == "rtlsdr" { i += 1; } }
    None
}

/// Angeschlossene Empfänger, mit „schon als Band eingetragen“ und doppelten Seriennummern
pub async fn devices(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let _p = admin!(state, headers);
    let found = tokio::task::spawn_blocking(crabsdr_sdr::probe::scan_devices).await.unwrap_or_default();
    let cfg = state.config.read().await;
    let mut serials: HashMap<String, usize> = HashMap::new();
    for d in found.iter().filter(|d| d.driver == "rtlsdr" && !d.serial.is_empty()) { *serials.entry(d.serial.clone()).or_default() += 1; }
    let list: Vec<Value> = found.iter().map(|d| {
        let index = (d.driver == "rtlsdr").then(|| rtl_index(&found, d)).flatten();
        let band = cfg.sdrs.iter().find(|s| (!d.serial.is_empty() && s.sdr_device == d.serial)
            || (s.sdr_driver == "rtl_sdr" && d.driver == "rtlsdr" && s.sdr_device.parse::<u32>().ok() == index)).map(|s| s.id.clone());
        json!({"driver": d.driver, "label": d.label, "serial": d.serial, "product": d.product, "available": d.available, "index": index, "band": band,
            "duplicate_serial": d.driver == "rtlsdr" && serials.get(&d.serial).copied().unwrap_or(0) > 1})
    }).collect();
    ok(json!({"devices": list}))
}

#[derive(Deserialize)]
pub struct SerialReq { pub index: u32, pub serial: String }

/// Seriennummer eines RTL-SDR-Sticks setzen (rtl_eeprom). Danach Stick ab- und wieder anstecken.
pub async fn set_serial(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<SerialReq>) -> Response {
    let p = admin!(state, headers);
    let s = req.serial.trim();
    if s.is_empty() || s.len() > 16 || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return err(StatusCode::BAD_REQUEST, "Seriennummer: 1–16 Zeichen, Buchstaben, Ziffern, - _");
    }
    if req.index > 64 { return err(StatusCode::BAD_REQUEST, "Stick-Nummer"); }
    info!("Admin „{}“: Stick {} bekommt Seriennummer „{}“", p.username, req.index, s);
    // ohne Shell: Argumente direkt, „y“ bestätigt die Rückfrage von rtl_eeprom
    let child = tokio::process::Command::new("rtl_eeprom").args(["-d", &req.index.to_string(), "-s", s])
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true).spawn();
    let mut child = match child { Ok(c) => c, Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("rtl_eeprom nicht startbar: {}", e)) };
    if let Some(mut stdin) = child.stdin.take() { use tokio::io::AsyncWriteExt; let _ = stdin.write_all(b"y\n").await; }
    let out = match tokio::time::timeout(std::time::Duration::from_secs(30), child.wait_with_output()).await {
        Ok(Ok(o)) => o, _ => return err(StatusCode::INTERNAL_SERVER_ERROR, "rtl_eeprom antwortet nicht"),
    };
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() || text.contains("Writing new serial") {
        ok(json!({"ok": true, "message": "Seriennummer geschrieben. Stick ab- und wieder anstecken.", "output": text.trim()}))
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "rtl_eeprom meldet einen Fehler (Stick in Benutzung?)", "output": text.trim()}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn df_ausgabe() {
        let out = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda2 245000000 56000000 176000000 25% /\n/dev/sda2 245000000 56000000 176000000 25% /\n/dev/sdb1 1000 900 100 90% /mnt/mein usb\n";
        let d = parse_df(out);
        assert_eq!(d.len(), 2, "gleiches Dateisystem nur einmal");
        assert_eq!(d[0]["percent"], 25);
        assert_eq!(d[1]["mount"], "/mnt/mein usb");
        assert_eq!(d[1]["percent"], 90);
    }
}
