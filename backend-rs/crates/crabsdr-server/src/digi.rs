//! Daten der Seite „Digital" (`web/digi/`): die Decoder-Plugins schreiben ihre Übersichten in ihr Datenverzeichnis
//! (`data_dir/decoders/<id>/`), der Server reicht sie unter festen Namen weiter – die Seite muss keine Kennungen kennen:
//!   /digi/aprs.json, /digi/relais.json   vom ersten öffentlichen APRS-Decoder (aprs.json, relais.json)
//!   /digi/ft8.json                       vom ersten öffentlichen FT8-Decoder
//!   /digi/sstv.json                      alle öffentlichen SSTV-Decoder zusammen, neueste Bilder zuerst
//! Fehlt der Decoder oder seine Datei, kommt 404.

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
pub async fn freedv(State(s): State<Arc<AppState>>, h: HeaderMap) -> Response { first(&s, &h, "freedv", "freedv.json").await }

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
