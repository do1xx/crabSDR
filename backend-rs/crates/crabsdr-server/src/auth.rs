//! Anmelden, Passwort ändern, eigenes Konto (docs/SECURITY.md).
//!   POST /api/auth/login     {username, password, scope: "listen"|"admin"}  → {token, user}
//!   POST /api/auth/password  {old, new}   (Bearer)                          → neues Token, alle anderen Sitzungen ab
//!   GET  /api/auth/me        (Bearer, optional)                            → Konto oder Gast
//!   POST /api/auth/guest     → Gast-Token (Hören-Seite ohne Anmeldung)

use axum::{
    extract::{ConnectInfo, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use crabsdr_auth::{AuthDb, JwtManager, ROLE_ADMIN, SCOPE_ADMIN, SCOPE_LISTEN, TTL_ADMIN, TTL_GUEST, TTL_LISTEN};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{info, warn};

use crate::access::{self, Principal};
use crate::ratelimit::LoginLimiter;
use crate::AppState;

pub struct AuthState {
    pub db: Arc<std::sync::Mutex<AuthDb>>,
    pub jwt: JwtManager,
    pub limiter: LoginLimiter,
}

fn err(code: StatusCode, msg: &str) -> Response {
    (code, [(header::CACHE_CONTROL, "no-store")], Json(json!({"error": msg}))).into_response()
}
fn ok(v: serde_json::Value) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(v)).into_response()
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub scope: Option<String>,
}

pub async fn login(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(req): Json<LoginRequest>) -> Response {
    let Some(auth) = &state.auth else { return err(StatusCode::SERVICE_UNAVAILABLE, "Anmeldung nicht verfügbar") };
    let ip = access::client_ip(&headers, Some(peer));
    let name: String = req.username.trim().chars().take(64).collect();
    let scope = if req.scope.as_deref() == Some(SCOPE_ADMIN) { SCOPE_ADMIN } else { SCOPE_LISTEN };
    if let Err(wait) = auth.limiter.check(&name, &ip) {
        warn!("Anmeldung gebremst: „{}“ von {} ({} s)", name, ip, wait);
        return (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, wait.to_string())],
            Json(json!({"error": format!("Zu viele Versuche. Bitte in {} Minuten erneut.", wait.div_ceil(60)), "retry_after": wait}))).into_response();
    }
    let (db, pw, n2) = (auth.db.clone(), req.password.clone(), name.clone());
    // Konto holen und Passwort prüfen im eigenen Thread (bcrypt dauert); unbekannter Name dauert gleich lange
    let user = tokio::task::spawn_blocking(move || {
        let u = db.lock().unwrap().get_user_by_name(&n2).ok().flatten();
        let good = crabsdr_auth::verify_password(&pw, u.as_ref().map(|u| u.password_hash.as_str()));
        u.filter(|u| good && u.active)
    }).await.ok().flatten();
    let Some(user) = user else {
        auth.limiter.failure(&name, &ip);
        warn!("Anmeldung fehlgeschlagen: „{}“ von {}", name, ip);
        return err(StatusCode::UNAUTHORIZED, "Name oder Passwort falsch");
    };
    auth.limiter.success(&name);
    if scope == SCOPE_ADMIN && user.role != ROLE_ADMIN {
        return err(StatusCode::FORBIDDEN, "Dieses Konto hat keine Admin-Rechte");
    }
    let ttl = if scope == SCOPE_ADMIN { TTL_ADMIN } else { TTL_LISTEN };
    let token = match auth.jwt.create_token(&user.id, &user.username, &user.role, scope, user.token_epoch, ttl) {
        Ok(t) => t,
        Err(e) => { warn!("Token: {}", e); return err(StatusCode::INTERNAL_SERVER_ERROR, "Anmeldung fehlgeschlagen"); }
    };
    { let db = auth.db.clone(); let id = user.id.clone(); let _ = tokio::task::spawn_blocking(move || db.lock().unwrap().touch_login(&id)).await; }
    info!("Anmeldung: „{}“ ({}, {}) von {}", user.username, user.role, scope, ip);
    ok(json!({"token": token, "expires_in": ttl,
        "user": {"username": user.username, "role": user.role, "scope": scope, "must_change": user.must_change_pw}}))
}

#[derive(Deserialize)]
pub struct PasswordRequest { pub old: String, pub new: String }

pub async fn change_password(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(req): Json<PasswordRequest>) -> Response {
    let Some(auth) = &state.auth else { return err(StatusCode::SERVICE_UNAVAILABLE, "Anmeldung nicht verfügbar") };
    let p = match access::require_account(&state, &headers).await { Ok(p) => p, Err(r) => return r };
    let ip = access::client_ip(&headers, Some(peer));
    if let Err(e) = crabsdr_auth::validate_password(&req.new) { return err(StatusCode::BAD_REQUEST, &e.to_string()); }
    if req.new == req.old { return err(StatusCode::BAD_REQUEST, "Das neue Passwort muss anders sein"); }
    if auth.limiter.check(&p.username, &ip).is_err() { return err(StatusCode::TOO_MANY_REQUESTS, "Zu viele Versuche, bitte später"); }
    let (db, id, old, new) = (auth.db.clone(), p.id.clone(), req.old.clone(), req.new.clone());
    let res = tokio::task::spawn_blocking(move || -> Result<Option<crabsdr_auth::User>, String> {
        let u = db.lock().unwrap().get_user(&id).ok().flatten();
        if !crabsdr_auth::verify_password(&old, u.as_ref().map(|u| u.password_hash.as_str())) { return Ok(None); }
        let hash = crabsdr_auth::hash_password(&new).map_err(|e| e.to_string())?;
        let db = db.lock().unwrap();
        db.set_password(&id, &hash, false).map_err(|e| e.to_string())?;
        Ok(db.get_user(&id).ok().flatten())
    }).await;
    let user = match res {
        Ok(Ok(Some(u))) => u,
        Ok(Ok(None)) => { auth.limiter.failure(&p.username, &ip); return err(StatusCode::UNAUTHORIZED, "Bisheriges Passwort falsch"); }
        _ => return err(StatusCode::INTERNAL_SERVER_ERROR, "Passwort konnte nicht gespeichert werden"),
    };
    let ttl = if p.scope == SCOPE_ADMIN { TTL_ADMIN } else { TTL_LISTEN };
    let token = auth.jwt.create_token(&user.id, &user.username, &user.role, &p.scope, user.token_epoch, ttl).unwrap_or_default();
    info!("Passwort geändert: „{}“ (alle anderen Sitzungen abgemeldet)", user.username);
    ok(json!({"token": token, "expires_in": ttl, "user": {"username": user.username, "role": user.role, "scope": p.scope, "must_change": false}}))
}

fn me_json(p: &Principal) -> serde_json::Value {
    json!({"username": p.username, "role": p.role, "scope": p.scope, "must_change": p.must_change, "bands": p.bands, "decoders": p.decoders})
}

pub async fn me(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    match access::resolve(&state, access::bearer(&headers).as_deref()).await {
        Ok(p) => ok(json!({"user": me_json(&p)})),
        Err(()) => err(StatusCode::UNAUTHORIZED, "Sitzung abgelaufen"),
    }
}

/// Gast-Token (die Hören-Seite öffnet damit die Bänder ohne Anmeldung)
pub async fn guest_login(State(state): State<Arc<AppState>>) -> Response {
    let Some(auth) = &state.auth else { return err(StatusCode::SERVICE_UNAVAILABLE, "Anmeldung nicht verfügbar") };
    let id = format!("guest-{}", crabsdr_auth::random_password(12));
    match auth.jwt.create_token(&id, "Gast", "guest", SCOPE_LISTEN, 0, TTL_GUEST) {
        Ok(t) => ok(json!({"token": t, "user": {"username": "Gast", "role": "guest"}})),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "kein Gast-Token"),
    }
}

pub async fn guest_available(State(state): State<Arc<AppState>>) -> Response {
    let c = state.config.read().await;
    ok(json!({"available": c.sdrs.iter().any(|s| s.enabled && s.guest && !s.admin_only)}))
}
