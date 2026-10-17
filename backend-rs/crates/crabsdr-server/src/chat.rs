//! Chat der Hörer, dauerhaft in SQLite (`data_dir/pinnwand.db`). Die Pfade heißen aus alter Zeit noch `/logbuch/api/…`
//! (Apps nutzen sie); das frühere Logbuch ist seit 1.6.0 entfernt, seine Tabelle wird beim Start gelöscht:
//!   GET  /logbuch/api/chat?since=<id>[&wait=1]   Chat ab id, wait=1 = Langabfrage bis 25 s → {lines:[{id,t,name,msg}], id}
//!   POST /logbuch/api/chat {name, msg}
//!   GET  /logbuch/api/online   Hörer gerade online;  GET /logbuch/api/ping
//! (alles auch unter /pinnwand/api/…). Keine IP-Adressen in der Datenbank; die Bremse (1 Chat/2 s je Absender) hält
//! sie nur flüchtig im Speicher. Abschaltbar mit `builtin_chat = false`.

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
const MAX_MSG: usize = 200;

pub struct Chat {
    db: Mutex<Option<Connection>>,
    notify: Notify,
    brake: Mutex<HashMap<(String, &'static str), Instant>>,
    /// Chat-Zeilen älter als so viele Sekunden löschen (0 = behalten)
    keep_s: i64,
    keep_lines: i64,
}

impl Chat {
    pub fn open(data_dir: &Path, keep_lines: u32, keep_hours: u32) -> Self {
        let path = data_dir.join("pinnwand.db");
        let db = match Connection::open(&path) {
            Ok(c) => {
                let ok = c.execute_batch(
                    "drop table if exists log;
                     create table if not exists chat (id integer primary key, t integer, name text, msg text);",
                ).is_ok();
                // Verbund-Chat: Herkunft fremder Zeilen (Stationskürzel), NULL = eigene Station
                let ccols: Vec<String> = c.prepare("pragma table_info(chat)").and_then(|mut s| s.query_map([], |r| r.get::<_, String>(1)).map(|m| m.flatten().collect())).unwrap_or_default();
                if !ccols.iter().any(|x| x == "station") { let _ = c.execute("alter table chat add column station text", []); }
                if ok { info!("Chat: {}", path.display()); Some(c) } else { None }
            }
            Err(e) => { warn!("Chat: {} nicht zu öffnen: {}", path.display(), e); None }
        };
        Self { db: Mutex::new(db), notify: Notify::new(), brake: Mutex::new(HashMap::new()), keep_s: keep_hours as i64 * 3600, keep_lines: keep_lines as i64 }
    }

    /// Alte Chat-Zeilen löschen: FIFO (chat_keep_lines, nur die neuesten bleiben) und optional nach Zeit (chat_keep_hours);
    /// beim Lesen und Schreiben aufgerufen, billig
    pub fn purge(&self) {
        if self.keep_s <= 0 && self.keep_lines <= 0 { return; }
        let g = self.db.lock().unwrap();
        let Some(c) = g.as_ref() else { return };
        if self.keep_lines > 0 { let _ = c.execute("delete from chat where id not in (select id from chat order by id desc limit ?)", [self.keep_lines]); }
        if self.keep_s > 0 { let _ = c.execute("delete from chat where t < ?", [now_s() - self.keep_s]); }
    }

    /// Ohne Datenbank (builtin_chat = false): Routen sind dann gar nicht angemeldet
    pub fn disabled() -> Self { Self { db: Mutex::new(None), notify: Notify::new(), brake: Mutex::new(HashMap::new()), keep_s: 0, keep_lines: 0 } }

    /// Für die Admin-Seite: letzte Chat-Zeilen (neueste zuerst)
    pub fn recent_chat(&self, n: i64) -> Vec<serde_json::Value> {
        let g = self.db.lock().unwrap();
        let Some(c) = g.as_ref() else { return vec![] };
        c.prepare("select id,t,name,msg from chat order by id desc limit ?").and_then(|mut st| st.query_map([n], |r| Ok(json!({
            "id": r.get::<_, i64>(0)?, "t": r.get::<_, i64>(1)?, "name": r.get::<_, String>(2)?, "msg": r.get::<_, String>(3)?}))).map(|m| m.flatten().collect())).unwrap_or_default()
    }
    pub fn delete_chat(&self, id: i64) -> bool {
        let g = self.db.lock().unwrap();
        g.as_ref().is_some_and(|c| c.execute("delete from chat where id = ?", [id]).is_ok_and(|n| n > 0))
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
pub struct Post { name: Option<String>, msg: Option<String> }

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
        r = r.route(&format!("{}/chat", base), get(get_chat).post(post_chat));
    }
    r
}

/// Hörer online + Ping: immer da (Hörerzahl auf den Unterseiten), auch ohne Chat
pub fn routes_always() -> Router<Arc<AppState>> {
    let mut r = Router::new();
    for base in ["/logbuch/api", "/pinnwand/api"] {
        r = r.route(&format!("{}/online", base), get(online)).route(&format!("{}/ping", base), get(ping));
    }
    r
}
