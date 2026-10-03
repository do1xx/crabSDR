//! Chat und Logbuch der Hörer, dauerhaft in SQLite (`data_dir/pinnwand.db`) – gleiche Tabellen und Schnittstelle wie der
//! frühere Dienst `pinnwand.py`, dessen Datenbank sich deshalb einfach übernehmen lässt:
//!   GET  /logbuch/api/log?n=100[&sort=km]   Logbuch, neueste zuerst (sort=km: weiteste zuerst)
//!   POST /logbuch/api/log  {name, call, freq, loc, comment}   freq in kHz (MHz wird erkannt), loc = Locator (optional)
//!   GET  /logbuch/api/chat?since=<id>[&wait=1]   Chat ab id, wait=1 = Langabfrage bis 25 s → {lines:[{id,t,name,msg}], id}
//!   POST /logbuch/api/chat {name, msg}
//!   GET  /logbuch/api/online   Hörer gerade online;  GET /logbuch/api/ping
//! (alles auch unter /pinnwand/api/…). Keine IP-Adressen in der Datenbank; die Bremse (1 Chat/2 s, 1 Eintrag/5 s je
//! Absender) hält sie nur flüchtig im Speicher. Abschaltbar mit `builtin_chat = false`.

use axum::{routing::get, Router, extract::{Query, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Json}};
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::AppState;

const MAX_NAME: usize = 20;
const MAX_CALL: usize = 20;
const MAX_MSG: usize = 200;
const MAX_COMMENT: usize = 80;

pub struct Chat {
    db: Mutex<Option<Connection>>,
    notify: Notify,
    brake: Mutex<HashMap<(String, &'static str), Instant>>,
    home: Option<(f64, f64)>,
    /// Chat-Zeilen älter als so viele Sekunden löschen (0 = behalten)
    keep_s: i64,
}

impl Chat {
    pub fn open(data_dir: &Path, home: Option<(f64, f64)>, keep_hours: u32) -> Self {
        let path = data_dir.join("pinnwand.db");
        let db = match Connection::open(&path) {
            Ok(c) => {
                let ok = c.execute_batch(
                    "create table if not exists log (id integer primary key, t integer, name text, call text, freq real, comment text);
                     create table if not exists chat (id integer primary key, t integer, name text, msg text);",
                ).is_ok();
                // Verbund-Chat: Herkunft fremder Zeilen (Stationskürzel), NULL = eigene Station
                let ccols: Vec<String> = c.prepare("pragma table_info(chat)").and_then(|mut s| s.query_map([], |r| r.get::<_, String>(1)).map(|m| m.flatten().collect())).unwrap_or_default();
                if !ccols.iter().any(|x| x == "station") { let _ = c.execute("alter table chat add column station text", []); }
                // Spalten wie pinnwand.py nachrüsten
                let cols: Vec<String> = c.prepare("pragma table_info(log)").and_then(|mut s| s.query_map([], |r| r.get::<_, String>(1)).map(|m| m.flatten().collect())).unwrap_or_default();
                for (col, typ) in [("loc", "text"), ("km", "integer"), ("deg", "integer")] {
                    if !cols.iter().any(|x| x == col) { let _ = c.execute(&format!("alter table log add column {} {}", col, typ), []); }
                }
                if ok { info!("Chat/Logbuch: {}", path.display()); Some(c) } else { None }
            }
            Err(e) => { warn!("Chat/Logbuch: {} nicht zu öffnen: {}", path.display(), e); None }
        };
        Self { db: Mutex::new(db), notify: Notify::new(), brake: Mutex::new(HashMap::new()), home, keep_s: keep_hours as i64 * 3600 }
    }

    /// Alte Chat-Zeilen löschen (chat_keep_hours); beim Lesen und Schreiben aufgerufen, billig
    pub fn purge(&self) {
        if self.keep_s <= 0 { return; }
        let g = self.db.lock().unwrap();
        if let Some(c) = g.as_ref() { let _ = c.execute("delete from chat where t < ?", [now_s() - self.keep_s]); }
    }

    /// Ohne Datenbank (builtin_chat = false): Routen sind dann gar nicht angemeldet
    pub fn disabled() -> Self { Self { db: Mutex::new(None), notify: Notify::new(), brake: Mutex::new(HashMap::new()), home: None, keep_s: 0 } }

    /// Für die Admin-Seite: letzte Chat-Zeilen (neueste zuerst)
    pub fn recent_chat(&self, n: i64) -> Vec<serde_json::Value> {
        let g = self.db.lock().unwrap();
        let Some(c) = g.as_ref() else { return vec![] };
        c.prepare("select id,t,name,msg from chat order by id desc limit ?").and_then(|mut st| st.query_map([n], |r| Ok(json!({
            "id": r.get::<_, i64>(0)?, "t": r.get::<_, i64>(1)?, "name": r.get::<_, String>(2)?, "msg": r.get::<_, String>(3)?}))).map(|m| m.flatten().collect())).unwrap_or_default()
    }
    /// Für die Admin-Seite: letzte Logbuch-Einträge (neueste zuerst)
    pub fn recent_log(&self, n: i64) -> Vec<serde_json::Value> {
        let g = self.db.lock().unwrap();
        let Some(c) = g.as_ref() else { return vec![] };
        c.prepare("select id,t,name,call,freq,loc,comment from log order by id desc limit ?").and_then(|mut st| st.query_map([n], |r| Ok(json!({
            "id": r.get::<_, i64>(0)?, "t": r.get::<_, i64>(1)?, "name": r.get::<_, Option<String>>(2)?, "call": r.get::<_, Option<String>>(3)?,
            "freq": r.get::<_, Option<f64>>(4)?, "loc": r.get::<_, Option<String>>(5)?, "comment": r.get::<_, Option<String>>(6)?}))).map(|m| m.flatten().collect())).unwrap_or_default()
    }
    pub fn delete_chat(&self, id: i64) -> bool {
        let g = self.db.lock().unwrap();
        g.as_ref().is_some_and(|c| c.execute("delete from chat where id = ?", [id]).is_ok_and(|n| n > 0))
    }
    pub fn delete_log(&self, id: i64) -> bool {
        let g = self.db.lock().unwrap();
        g.as_ref().is_some_and(|c| c.execute("delete from log where id = ?", [id]).is_ok_and(|n| n > 0))
    }

    fn throttle(&self, who: &str, kind: &'static str, secs: u64) -> bool {
        let mut b = self.brake.lock().unwrap();
        if b.len() > 5000 { b.clear(); }
        let k = (who.to_string(), kind);
        if let Some(t) = b.get(&k) { if t.elapsed() < Duration::from_secs(secs) { return false; } }
        b.insert(k, Instant::now());
        true
    }
}

fn now_s() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }
fn clean(s: Option<&str>, max: usize) -> String {
    s.unwrap_or("").chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().chars().take(max).collect()
}
fn sender(h: &HeaderMap) -> String {
    h.get("x-real-ip").or_else(|| h.get("x-forwarded-for")).and_then(|v| v.to_str().ok()).map(|s| s.split(',').next().unwrap_or("").trim().to_string()).unwrap_or_else(|| "lokal".into())
}
/// Entfernung (km) und Richtung (°) vom Stationsstandort
pub fn dist_bearing(home: (f64, f64), lat: f64, lon: f64) -> (f64, f64) {
    let (la1, lo1, la2, lo2) = (home.0.to_radians(), home.1.to_radians(), lat.to_radians(), lon.to_radians());
    let a = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    let d = 2.0 * 6371.0 * a.sqrt().asin();
    let y = (lo2 - lo1).sin() * la2.cos();
    let x = la1.cos() * la2.sin() - la1.sin() * la2.cos() * (lo2 - lo1).cos();
    (d, (y.atan2(x).to_degrees() + 360.0) % 360.0)
}

#[derive(Deserialize)]
pub struct SinceQuery { since: Option<i64>, wait: Option<u8> }

fn chat_rows(chat: &Chat, since: i64) -> (i64, Vec<serde_json::Value>) {
    let g = chat.db.lock().unwrap();
    let Some(c) = g.as_ref() else { return (since, vec![]) };
    let map = |r: &rusqlite::Row| Ok(json!({"id": r.get::<_, i64>(0)?, "t": r.get::<_, i64>(1)?, "name": r.get::<_, String>(2)?, "msg": r.get::<_, String>(3)?, "station": r.get::<_, Option<String>>(4)?}));
    let rows: Vec<serde_json::Value> = if since == 0 {
        c.prepare("select id,t,name,msg,station from (select * from chat order by id desc limit 100) order by id").and_then(|mut s| s.query_map([], map).map(|m| m.flatten().collect())).unwrap_or_default()
    } else {
        c.prepare("select id,t,name,msg,station from chat where id>? order by id limit 200").and_then(|mut s| s.query_map([since], map).map(|m| m.flatten().collect())).unwrap_or_default()
    };
    let last = rows.last().and_then(|r| r["id"].as_i64()).unwrap_or(since);
    (last, rows)
}

pub async fn get_chat(State(state): State<Arc<AppState>>, Query(q): Query<SinceQuery>) -> impl IntoResponse {
    let chat = state.chat.clone();
    chat.purge();
    let since = q.since.unwrap_or(0);
    let (mut id, mut lines) = chat_rows(&chat, since);
    if lines.is_empty() && q.wait == Some(1) {
        let _ = tokio::time::timeout(Duration::from_secs(25), chat.notify.notified()).await;
        let r = chat_rows(&chat, since); id = r.0; lines = r.1;
    }
    Json(json!({"lines": lines, "id": id}))
}

#[derive(Deserialize)]
pub struct Post { name: Option<String>, msg: Option<String>, call: Option<String>, freq: Option<serde_json::Value>, loc: Option<String>, comment: Option<String> }

pub async fn post_chat(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(p): Json<Post>) -> impl IntoResponse {
    let chat = state.chat.clone();
    chat.purge();
    let msg = clean(p.msg.as_deref(), MAX_MSG);
    if msg.is_empty() { return (StatusCode::BAD_REQUEST, Json(json!({"error": "leer"}))).into_response(); }
    if !chat.throttle(&sender(&headers), "chat", 2) { return (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error": "langsamer"}))).into_response(); }
    let mut name = clean(p.name.as_deref(), MAX_NAME); if name.is_empty() { name = "Hörer".into(); }
    let id = {
        let g = chat.db.lock().unwrap();
        match g.as_ref() { Some(c) => c.execute("insert into chat (t,name,msg) values (?,?,?)", params![now_s(), name, msg]).map(|_| c.last_insert_rowid()).ok(), None => None }
    };
    chat.notify.notify_waiters();
    // Verbund-Chat: Zeile an crabsdr.de weiterreichen (ohne Hörer-IP), Fehler nur leise
    if id.is_some() {
        let cfg = state.config.read().await;
        if cfg.chat_verbund && cfg.directory.enabled && !cfg.station.url.is_empty() {
            let url = format!("{}/api/verbund/chat", cfg.directory.server.trim_end_matches('/'));
            let body = json!({"id": crate::directory::station_id(&cfg.data_dir), "url": cfg.station.url, "name": name, "msg": msg});
            drop(cfg);
            tokio::task::spawn_blocking(move || { if let Err(e) = crate::directory::post(&url, &body) { tracing::debug!("Verbund-Chat: {}", e); } });
        }
    }
    match id { Some(i) => Json(json!({"ok": true, "id": i})).into_response(), None => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "Datenbank fehlt"}))).into_response() }
}

/// Verbund-Chat: fremde Zeilen von crabsdr.de holen (Langabfrage) und in den eigenen Chat einsortieren. Die letzte
/// gesehene Nummer steht in `data_dir/verbund.id`, damit nach einem Neustart nichts doppelt kommt.
pub async fn run_verbund(state: Arc<AppState>) {
    let (server, own, idfile) = { let c = state.config.read().await;
        (c.directory.server.trim_end_matches('/').to_string(), crate::directory::station_id(&c.data_dir), c.data_dir.join("verbund.id")) };
    let mut since: i64 = std::fs::read_to_string(&idfile).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let mut warned: Option<Instant> = None;
    info!("Verbund-Chat: an ({})", server);
    loop {
        let url = format!("{}/api/verbund/chat?since={}&wait=1", server, since);
        let res = tokio::task::spawn_blocking(move || {
            let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(40)).redirects(0).user_agent(&format!("crabSDR/{}", env!("CARGO_PKG_VERSION"))).build();
            agent.get(&url).call().map_err(|e| e.to_string()).and_then(|r| r.into_json::<serde_json::Value>().map_err(|e| e.to_string()))
        }).await.unwrap_or_else(|e| Err(e.to_string()));
        match res {
            Ok(v) => {
                let lines = v["lines"].as_array().cloned().unwrap_or_default();
                let mut added = false;
                for l in &lines {
                    let id = l["id"].as_i64().unwrap_or(0);
                    if id > since { since = id; }
                    if l["station"].as_str() == Some(own.as_str()) { continue; }   // eigene Zeilen stehen schon lokal
                    let (t, name, msg, label) = (l["t"].as_i64().unwrap_or_else(now_s), l["name"].as_str().unwrap_or("Hörer"), l["msg"].as_str().unwrap_or(""), l["label"].as_str().unwrap_or("?"));
                    if msg.is_empty() { continue; }
                    let g = state.chat.db.lock().unwrap();
                    if let Some(c) = g.as_ref() { let _ = c.execute("insert into chat (t,name,msg,station) values (?,?,?,?)", params![t, name, msg, label]); added = true; }
                }
                if added { state.chat.notify.notify_waiters(); }
                if !lines.is_empty() { let _ = std::fs::write(&idfile, since.to_string()); }
                warned = None;
            }
            Err(e) => {
                if warned.is_none_or(|t| t.elapsed() >= Duration::from_secs(3600)) { warn!("Verbund-Chat: {} nicht erreichbar: {} (weiter alle 30 s)", server, e); warned = Some(Instant::now()); }
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        }
    }
}

#[derive(Deserialize)]
pub struct LogQuery { n: Option<i64>, sort: Option<String> }

pub async fn get_log(State(state): State<Arc<AppState>>, Query(q): Query<LogQuery>) -> impl IntoResponse {
    let n = q.n.unwrap_or(100).clamp(1, 500);
    let order = if q.sort.as_deref() == Some("km") { "km desc, id desc" } else { "id desc" };
    let g = state.chat.db.lock().unwrap();
    let rows: Vec<serde_json::Value> = g.as_ref().and_then(|c| c.prepare(&format!("select id,t,name,call,freq,loc,km,deg,comment from log order by {} limit ?", order)).ok()
        .and_then(|mut s| s.query_map([n], |r| Ok(json!({"id": r.get::<_, i64>(0)?, "t": r.get::<_, i64>(1)?, "name": r.get::<_, Option<String>>(2)?,
            "call": r.get::<_, Option<String>>(3)?, "freq": r.get::<_, Option<f64>>(4)?, "loc": r.get::<_, Option<String>>(5)?, "km": r.get::<_, Option<i64>>(6)?,
            "deg": r.get::<_, Option<i64>>(7)?, "comment": r.get::<_, Option<String>>(8)?}))).map(|m| m.flatten().collect()).ok())).unwrap_or_default();
    Json(serde_json::Value::Array(rows))
}

pub async fn post_log(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(p): Json<Post>) -> impl IntoResponse {
    let chat = state.chat.clone();
    let call = clean(p.call.as_deref(), MAX_CALL).to_uppercase();
    let mut freq = match &p.freq { Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0), Some(serde_json::Value::String(s)) => s.replace(',', ".").trim().parse().unwrap_or(0.0), _ => 0.0 };
    if freq > 0.0 && freq < 1000.0 { freq *= 1000.0; }       // MHz eingegeben
    if call.is_empty() || !(1000.0..=30_000_000.0).contains(&freq) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Rufzeichen und Frequenz (kHz) fehlen"}))).into_response();
    }
    let loc = clean(p.loc.as_deref(), 8).to_uppercase().replace(' ', "");
    let (mut km, mut deg) = (None, None);
    if !loc.is_empty() {
        let Some((la, lo)) = crabsdr_core::locator_to_latlon(&loc) else { return (StatusCode::BAD_REQUEST, Json(json!({"error": "Locator ungültig (z. B. JO53RB)"}))).into_response() };
        if let Some(h) = chat.home { let (d, b) = dist_bearing(h, la, lo); km = Some(d.round() as i64); deg = Some(b.round() as i64); }
    }
    if !chat.throttle(&sender(&headers), "log", 5) { return (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error": "langsamer"}))).into_response(); }
    let mut name = clean(p.name.as_deref(), MAX_NAME); if name.is_empty() { name = "Hörer".into(); }
    let ok = {
        let g = chat.db.lock().unwrap();
        g.as_ref().map(|c| c.execute("insert into log (t,name,call,freq,loc,km,deg,comment) values (?,?,?,?,?,?,?,?)",
            params![now_s(), name, call, (freq * 1000.0).round() / 1000.0, if loc.is_empty() { None } else { Some(loc.clone()) }, km, deg, clean(p.comment.as_deref(), MAX_COMMENT)]).is_ok()).unwrap_or(false)
    };
    if ok { Json(json!({"ok": true, "km": km, "deg": deg})).into_response() } else { (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "Datenbank fehlt"}))).into_response() }
}

pub async fn ping() -> impl IntoResponse { Json(json!({"ok": true})) }

/// Hörer gerade online (Namen, Band, kHz) – aus den Hörerlisten der Bänder
pub async fn online(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let manager = state.manager.read().await;
    let mut users = Vec::new();
    for (id, pipeline) in manager.all() {
        let clients = pipeline.clients.lock().await;
        for l in clients.listeners_json(id) {
            users.push(json!({"name": l["name"].as_str().filter(|s| !s.is_empty()).unwrap_or("Hörer"), "band": l["band"],
                "khz": l["freq"].as_u64().map(|f| (f as f64 / 100.0).round() / 10.0)}));
        }
    }
    Json(json!({"count": users.len(), "users": users, "t": now_s()}))
}

pub fn routes() -> Router<Arc<AppState>> {
    let mut r = Router::new();
    for base in ["/logbuch/api", "/pinnwand/api"] {
        r = r.route(&format!("{}/chat", base), get(get_chat).post(post_chat))
            .route(&format!("{}/log", base), get(get_log).post(post_log));
    }
    r
}

/// Hörer online + Ping: immer da (Hörerzahl auf den Unterseiten), auch ohne Chat/Logbuch
pub fn routes_always() -> Router<Arc<AppState>> {
    let mut r = Router::new();
    for base in ["/logbuch/api", "/pinnwand/api"] {
        r = r.route(&format!("{}/online", base), get(online)).route(&format!("{}/ping", base), get(ping));
    }
    r
}
