//! Eintrag im öffentlichen crabSDR-Verzeichnis (crabsdr.de), wie das Receiverbook bei OpenWebRX.
//!
//! Nur wenn der Sysop es einschaltet (`[directory] enabled = true`, Admin-Seite → Station). Dann:
//!   GET /api/directory  öffentlich: was im Verzeichnis steht (sonst 404). Das Verzeichnis ruft genau diese Adresse
//!                       unter `station.url` ab und vergleicht die Kennung – so kann niemand fremde Adressen eintragen.
//!   Meldung             alle 5 min POST `<server>/api/report` mit denselben Angaben.
//! Gemeldet wird nur, was ohnehin öffentlich ist: Name, Untertitel, Locator/Standort, öffentliche Bänder
//! (keine Mitglieder- oder Admin-Bänder), öffentliche Decoder, Hörerzahl, Version. Die Kennung ist eine Zufallszahl
//! aus `data_dir/directory.id` und verrät nichts über die Station.

use axum::{extract::State, http::{header, StatusCode}, response::{IntoResponse, Response}};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::AppState;

const INTERVAL: Duration = Duration::from_secs(300);
const FIRST: Duration = Duration::from_secs(30);

/// Kennung der Station (einmal erzeugt, bleibt über Neustarts)
pub fn station_id(data_dir: &std::path::Path) -> String {
    let p = data_dir.join("directory.id");
    if let Ok(s) = std::fs::read_to_string(&p) {
        let s = s.trim().to_string();
        if s.len() >= 16 && s.chars().all(|c| c.is_ascii_alphanumeric()) { return s; }
    }
    let id = crabsdr_auth::random_password(24);
    let _ = std::fs::write(&p, &id);
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)); }
    id
}

/// Angaben für das Verzeichnis; None, wenn die Station nicht gelistet werden soll
pub async fn entry(state: &AppState) -> Option<Value> {
    let c = state.config.read().await.clone();
    if !c.directory.enabled { return None; }
    let st = &c.station;
    let home = st.home();
    let bands: Vec<Value> = c.sdrs.iter().filter(|b| b.enabled && b.guest && !b.admin_only).map(|b| {
        let half = b.sample_rate as u64 / 2;
        json!({"id": b.id, "label": b.label, "center": b.center_freq, "from": b.center_freq.saturating_sub(half), "to": b.center_freq + half,
               "mode": b.default_mode, "note": b.note})
    }).collect();
    let decoders: Vec<Value> = c.decoders.iter().filter(|d| d.enabled && d.public).map(|d| {
        json!({"plugin": d.plugin, "label": d.label.clone().unwrap_or_else(|| d.plugin.to_uppercase()), "freq": d.freq})
    }).collect();
    let mut listeners = 0usize;
    {
        let manager = state.manager.read().await;
        for p in manager.all().values() { listeners += p.clients.lock().await.count_real(); }
    }
    Some(json!({
        "crabsdr": 1,
        "id": station_id(&c.data_dir),
        "url": st.url.trim().trim_end_matches('/'),
        "name": st.name, "subtitle": st.subtitle, "locator": st.locator,
        "lat": home.map(|h| (h.0 * 1e4).round() / 1e4), "lon": home.map(|h| (h.1 * 1e4).round() / 1e4),
        "bands": bands, "decoders": decoders, "listeners": listeners,
        "version": crate::version_long(), "uptime_s": crate::uptime_s(),
    }))
}

/// GET /api/directory (öffentlich)
pub async fn api_entry(State(state): State<Arc<AppState>>) -> Response {
    match entry(&state).await {
        Some(v) => ([(header::CACHE_CONTROL, "no-store"), (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")], axum::Json(v)).into_response(),
        None => (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, "no-store")], axum::Json(json!({"error": "nicht im Verzeichnis gelistet"}))).into_response(),
    }
}

/// Meldung verschicken (blockierend, läuft im eigenen Thread). Ok(Antwort des Verzeichnisses) oder Fehlertext.
pub fn post(url: &str, body: &Value) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).redirects(0)
        .user_agent(&format!("crabSDR/{}", env!("CARGO_PKG_VERSION"))).build();
    match agent.post(url).send_json(body.clone()) {
        Ok(r) => r.into_json::<Value>().map_err(|e| format!("Antwort unlesbar: {}", e)),
        Err(ureq::Error::Status(code, r)) => {
            let msg = r.into_json::<Value>().ok().and_then(|v| v["error"].as_str().map(String::from)).unwrap_or_default();
            Err(format!("HTTP {} {}", code, msg))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Hintergrundaufgabe: alle 5 min melden. Fehler einmal melden, danach höchstens stündlich (kein Protokollmüll,
/// wenn das Verzeichnis mal nicht erreichbar ist).
pub async fn run(state: Arc<AppState>) {
    tokio::time::sleep(FIRST).await;
    let (mut ok_before, mut last_warn): (Option<bool>, Option<Instant>) = (None, None);
    loop {
        let server = state.config.read().await.directory.server.trim_end_matches('/').to_string();
        if let Some(body) = entry(&state).await {
            let url = format!("{}/api/report", server);
            let res = tokio::task::spawn_blocking(move || post(&url, &body)).await.unwrap_or_else(|e| Err(e.to_string()));
            match res {
                Ok(v) => {
                    if ok_before != Some(true) { info!("Verzeichnis {}: gelistet{}", server, v["note"].as_str().map(|n| format!(" ({})", n)).unwrap_or_default()); }
                    ok_before = Some(true);
                }
                Err(e) => {
                    if ok_before != Some(false) || last_warn.is_none_or(|t| t.elapsed() >= Duration::from_secs(3600)) {
                        warn!("Verzeichnis {}: Meldung nicht angenommen: {} (neuer Versuch alle 5 min, Meldung höchstens stündlich)", server, e);
                        last_warn = Some(Instant::now());
                    }
                    ok_before = Some(false);
                }
            }
        }
        tokio::time::sleep(INTERVAL).await;
    }
}
