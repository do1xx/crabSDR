//! Daten der Seite „Digital" (`web/digi/`): die Decoder-Plugins schreiben ihre Übersichten in ihr Datenverzeichnis
//! (`data_dir/decoders/<id>/`), der Server reicht sie unter festen Namen weiter – die Seite muss keine Kennungen kennen:
//!   /digi/aprs.json, /digi/relais.json   vom ersten öffentlichen APRS-Decoder (aprs.json, relais.json)
//!   /digi/ft8.json                       vom ersten öffentlichen FT8-Decoder
//!   /digi/sstv.json                      alle öffentlichen SSTV-Decoder zusammen, neueste Bilder zuerst
//!   /digi/freedv.json                    FreeDV: freedv.json des Codec-2-Decoders + Rufzeichen (text-Treffer) von rade und freedv
//! Fehlt der Decoder oder seine Datei, kommt 404 (freedv.json: leere Listen).

use axum::{extract::State, http::{header, HeaderMap, StatusCode}, response::{IntoResponse, Response}};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::AppState;

async fn ids(state: &AppState, headers: &HeaderMap, plugin: &str) -> Vec<String> {
    let p = crate::access::from_headers_or_guest(state, headers).await;
    state.decoders.visible_ids(plugin, &p).await
}

async fn read(state: &AppState, id: &str, file: &str) -> Option<Vec<u8>> {
    tokio::fs::read(state.decoders.data_dir().join("decoders").join(id).join(file)).await.ok()
}

fn json_resp(body: Vec<u8>) -> Response {
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-cache")], body).into_response()
}

async fn first(state: &AppState, headers: &HeaderMap, plugin: &str, file: &str) -> Response {
    for id in ids(state, headers, plugin).await {
        if let Some(b) = read(state, &id, file).await { return json_resp(b); }
    }
    (StatusCode::NOT_FOUND, "kein Decoder").into_response()
}

pub async fn aprs(State(s): State<Arc<AppState>>, h: HeaderMap) -> Response { first(&s, &h, "aprs", "aprs.json").await }
pub async fn relais(State(s): State<Arc<AppState>>, h: HeaderMap) -> Response { first(&s, &h, "aprs", "relais.json").await }
pub async fn ft8(State(s): State<Arc<AppState>>, h: HeaderMap) -> Response { first(&s, &h, "ft8", "ft8.json").await }
pub async fn pocsag(State(s): State<Arc<AppState>>, h: HeaderMap) -> Response { first(&s, &h, "pocsag", "pocsag.json").await }
/// Seite „FreeDV“: Zustand der Codec-2-Decoder (`freedv.json` des ersten sichtbaren `freedv`-Decoders: `active`, `modes`),
/// die sichtbaren Decoder beider DV-Plugins (`rade`, `freedv`, auch die flüchtigen der Betriebsart DV) und die
/// Rufzeichen (`text`-Treffer aus dem Ereignisspeicher: RADE aus dem EOO-Rahmen, Codec 2 aus dem Textkanal), neueste zuerst.
/// Nie 404: ohne Decoder leere Listen – die Rufzeichen bleiben im Ringpuffer, auch wenn der DV-Decoder schon beendet ist.
pub async fn freedv(State(s): State<Arc<AppState>>, h: HeaderMap) -> Response {
    let p = crate::access::from_headers_or_guest(&s, &h).await;
    let mut status = Value::Null;
    for id in s.decoders.visible_ids("freedv", &p).await {
        if let Some(v) = read(&s, &id, "freedv.json").await.and_then(|b| serde_json::from_slice::<Value>(&b).ok()) { status = v; break; }
    }
    let decs: Vec<Value> = s.decoders.visible_status(crate::decoders::DV_PLUGINS, &p).await.iter().map(|d| json!({
        "id": d.id, "plugin": d.plugin, "label": d.label, "band": d.band, "freq": d.freq, "state": d.state, "dynamic": d.dynamic, "events": d.events, "last_event": d.last_event })).collect();
    let calls: Vec<Value> = s.decoders.events_of(crate::decoders::DV_PLUGINS, "text", &p, 500).await.iter().map(|e| json!({
        "t": e.t.round() as i64, "text": e.data.get("text").and_then(|v| v.as_str()).unwrap_or(""), "mode": e.data.get("mode").and_then(|v| v.as_str()).unwrap_or(""),
        "snr": e.data.get("snr").and_then(|v| v.as_f64()), "freq": e.freq, "band": e.band, "plugin": e.plugin, "id": e.id })).collect();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    json_resp(json!({ "ts": now, "status": status, "decoders": decs, "calls": calls }).to_string().into_bytes())
}

/// Neuestes SSTV-Bild für Monitore, Spiegel und Skripte: /digi/sstv/latest.png (immer das jüngste Bild, nie gecacht)
/// und /digi/sstv/latest.json (sein Eintrag). `?decoder=<id>` nur dieser Decoder, `?max_age=<s>` 404, wenn älter.
#[derive(serde::Deserialize)]
pub struct LatestQuery { pub decoder: Option<String>, pub max_age: Option<u64> }

async fn latest_entry(s: &AppState, h: &HeaderMap, q: &LatestQuery) -> Option<Value> {
    let mut best: Option<Value> = None;
    for id in ids(s, h, "sstv").await {
        if q.decoder.as_deref().is_some_and(|d| d != id) { continue; }
        let Some(b) = read(s, &id, "sstv.json").await else { continue };
        let Ok(v) = serde_json::from_slice::<Value>(&b) else { continue };
        for mut im in v["images"].as_array().cloned().unwrap_or_default() {
            im["decoder"] = json!(id);
            if best.as_ref().is_none_or(|b| im["t"].as_i64().unwrap_or(0) > b["t"].as_i64().unwrap_or(0)) { best = Some(im); }
        }
    }
    let best = best?;
    if let Some(max) = q.max_age {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        if now - best["t"].as_i64().unwrap_or(0) > max as i64 { return None; }
    }
    Some(best)
}

pub async fn sstv_latest_json(State(s): State<Arc<AppState>>, h: HeaderMap, axum::extract::Query(q): axum::extract::Query<LatestQuery>) -> Response {
    match latest_entry(&s, &h, &q).await {
        Some(mut im) => {
            if let (Some(id), Some(f)) = (im["decoder"].as_str().map(str::to_string), im["file"].as_str().map(str::to_string)) {
                im["url"] = json!(format!("/api/decoders/files/{}/{}", id, f.trim_start_matches('/')));
            }
            ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")], im.to_string().into_bytes()).into_response()
        }
        None => (StatusCode::NOT_FOUND, "kein Bild").into_response(),
    }
}

pub async fn sstv_latest_png(State(s): State<Arc<AppState>>, h: HeaderMap, axum::extract::Query(q): axum::extract::Query<LatestQuery>) -> Response {
    let Some(im) = latest_entry(&s, &h, &q).await else { return (StatusCode::NOT_FOUND, "kein Bild").into_response() };
    let (Some(id), Some(f)) = (im["decoder"].as_str(), im["file"].as_str()) else { return (StatusCode::NOT_FOUND, "kein Bild").into_response() };
    let f = f.trim_start_matches('/');
    if f.contains("..") || !f.ends_with(".png") { return (StatusCode::NOT_FOUND, "kein Bild").into_response(); }
    let Some(b) = read(&s, id, f).await else { return (StatusCode::NOT_FOUND, "Bilddatei fehlt").into_response() };
    let mode = im["mode"].as_str().unwrap_or("").to_string();
    let t = im["t"].as_i64().unwrap_or(0).to_string();
    ([(header::CONTENT_TYPE, "image/png".to_string()), (header::CACHE_CONTROL, "no-store".to_string()),
      (header::HeaderName::from_static("x-sstv-mode"), mode), (header::HeaderName::from_static("x-sstv-time"), t)], b).into_response()
}

pub async fn sstv(State(s): State<Arc<AppState>>, h: HeaderMap) -> Response {
    let list = ids(&s, &h, "sstv").await;
    if list.is_empty() { return (StatusCode::NOT_FOUND, "kein Decoder").into_response(); }
    let (mut images, mut ts) = (Vec::<Value>::new(), 0i64);
    for id in &list {
        let Some(b) = read(&s, id, "sstv.json").await else { continue };
        let Ok(v) = serde_json::from_slice::<Value>(&b) else { continue };
        ts = ts.max(v["ts"].as_i64().unwrap_or(0));
        for mut im in v["images"].as_array().cloned().unwrap_or_default() {
            if let Some(f) = im["file"].as_str() { let f = format!("../api/decoders/files/{}/{}", id, f.trim_start_matches('/')); im["file"] = json!(f); }
            im["decoder"] = json!(id);
            images.push(im);
        }
    }
    images.sort_by_key(|im| std::cmp::Reverse(im["t"].as_i64().unwrap_or(0)));
    images.truncate(1000);
    json_resp(json!({"ts": ts, "images": images}).to_string().into_bytes())
}
