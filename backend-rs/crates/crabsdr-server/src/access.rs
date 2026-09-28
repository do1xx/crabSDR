//! Wer darf was (docs/SECURITY.md). Ein Token wird bei jeder Anfrage gegen die Datenbank geprüft: Konto vorhanden,
//! aktiv, Sitzungszähler gleich. Rechte kommen aus der Datenbank, nie aus dem Token.

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use crabsdr_auth::{ALL, ROLE_ADMIN, SCOPE_ADMIN};
use serde_json::json;
use std::net::SocketAddr;

use crate::AppState;

#[derive(Debug, Clone)]
pub struct Principal {
    pub id: String,
    pub username: String,
    /// admin | user | guest
    pub role: String,
    /// listen | admin
    pub scope: String,
    pub bands: Vec<String>,
    pub decoders: Vec<String>,
    pub must_change: bool,
}

impl Principal {
    pub fn guest() -> Self {
        Self { id: "guest".into(), username: String::new(), role: "guest".into(), scope: "listen".into(), bands: vec![], decoders: vec![], must_change: false }
    }
    pub fn is_admin(&self) -> bool { self.role == ROLE_ADMIN }
    pub fn is_guest(&self) -> bool { self.role == "guest" }

    /// Band hören: öffentlich = alle; Mitglieder = zugeteilte Nutzer; nur Admin = Admins
    pub fn may_band(&self, id: &str, public: bool, admin_only: bool) -> bool {
        if self.is_admin() { return true; }
        if admin_only { return false; }
        if public { return true; }
        !self.is_guest() && self.bands.iter().any(|b| b == id || b == ALL)
    }

    /// Decoder-Ergebnisse sehen: öffentliche alle; nicht öffentliche Admins und zugeteilte Nutzer
    pub fn may_decoder(&self, id: &str, public: bool) -> bool {
        public || self.is_admin() || (!self.is_guest() && self.decoders.iter().any(|d| d == id || d == ALL))
    }
}

/// Token aus `Authorization: Bearer …`
pub fn bearer(headers: &HeaderMap) -> Option<String> {
    headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ").map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Absender für die Anmeldebremse: direkte Gegenstelle; steht davor ein Proxy im eigenen Netz, dessen Angabe
/// (X-Real-IP, sonst erster Eintrag von X-Forwarded-For). Fälschen hilft nichts gegen die Sperre je Name.
pub fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>) -> String {
    let local = peer.map_or(true, |p| match p.ip() {
        std::net::IpAddr::V4(v) => v.is_loopback() || v.is_private() || (v.octets()[0] == 100 && (64..128).contains(&v.octets()[1])),
        std::net::IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00,
    });
    if local {
        let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).map(|s| s.split(',').next().unwrap_or("").trim().to_string()).filter(|s| !s.is_empty());
        if let Some(ip) = h("x-real-ip").or_else(|| h("x-forwarded-for")) { return ip.chars().take(64).collect(); }
    }
    peer.map(|p| p.ip().to_string()).unwrap_or_else(|| "?".into())
}

/// Token → Person. Kein Token = Gast; ungültiges oder widerrufenes Token = Err.
pub async fn resolve(state: &AppState, token: Option<&str>) -> Result<Principal, ()> {
    let Some(token) = token.filter(|t| !t.is_empty()) else { return Ok(Principal::guest()) };
    let Some(auth) = &state.auth else { return Err(()) };
    let claims = auth.jwt.verify_token(token).map_err(|_| ())?;
    if claims.is_guest() { return Ok(Principal::guest()); }
    let db = auth.db.clone();
    let sub = claims.sub.clone();
    let row = tokio::task::spawn_blocking(move || {
        let db = db.lock().unwrap();
        let u = db.get_user(&sub).ok().flatten()?;
        let bands = db.bands_of(&sub).unwrap_or_default();
        let decs = db.decoders_of(&sub).unwrap_or_default();
        Some((u, bands, decs))
    }).await.map_err(|_| ())?;
    let Some((u, bands, decoders)) = row else { return Err(()) };
    if !u.active || u.token_epoch != claims.ep { return Err(()); }
    // Admin-Sitzung nur für Admin-Konten (falls jemand herabgestuft wurde, ist ep ohnehin gestiegen)
    if claims.scope == SCOPE_ADMIN && u.role != ROLE_ADMIN { return Err(()); }
    Ok(Principal { id: u.id, username: u.username, role: u.role, scope: claims.scope, bands, decoders, must_change: u.must_change_pw })
}

/// Person aus dem Authorization-Kopf; ohne Kopf Gast, ungültiges Token ebenfalls Gast (Lesezugriffe)
pub async fn from_headers_or_guest(state: &AppState, headers: &HeaderMap) -> Principal {
    resolve(state, bearer(headers).as_deref()).await.unwrap_or_else(|_| Principal::guest())
}

fn deny(code: StatusCode, msg: &str) -> Response {
    (code, [(axum::http::header::CACHE_CONTROL, "no-store")], Json(json!({"error": msg}))).into_response()
}

/// Angemeldetes Konto (kein Gast), beliebige Art; `must_change` erlaubt (für den Passwortwechsel)
pub async fn require_account(state: &AppState, headers: &HeaderMap) -> Result<Principal, Response> {
    match resolve(state, bearer(headers).as_deref()).await {
        Ok(p) if !p.is_guest() => Ok(p),
        _ => Err(deny(StatusCode::UNAUTHORIZED, "nicht angemeldet")),
    }
}

/// Admin-Schnittstelle: Admin-Sitzung (scope admin) eines Admin-Kontos, Passwort bereits geändert
pub async fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<Principal, Response> {
    let p = match resolve(state, bearer(headers).as_deref()).await {
        Ok(p) if !p.is_guest() => p,
        _ => return Err(deny(StatusCode::UNAUTHORIZED, "nicht angemeldet")),
    };
    if !p.is_admin() || p.scope != SCOPE_ADMIN { return Err(deny(StatusCode::FORBIDDEN, "nur mit Admin-Anmeldung")); }
    if p.must_change {
        return Err((StatusCode::FORBIDDEN, [(axum::http::header::CACHE_CONTROL, "no-store")], Json(json!({"error": "Passwort zuerst ändern", "must_change": true}))).into_response());
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(role: &str, bands: &[&str], decs: &[&str]) -> Principal {
        Principal { id: "x".into(), username: "x".into(), role: role.into(), scope: "listen".into(),
            bands: bands.iter().map(|s| s.to_string()).collect(), decoders: decs.iter().map(|s| s.to_string()).collect(), must_change: false }
    }

    #[test]
    fn baender() {
        let g = Principal::guest();
        assert!(g.may_band("2m", true, false));
        assert!(!g.may_band("70cm", false, false));
        assert!(!g.may_band("2m", true, true), "nur Admin schlägt öffentlich");
        let u = p("user", &["70cm"], &[]);
        assert!(u.may_band("70cm", false, false) && !u.may_band("23cm", false, false) && !u.may_band("70cm", false, true));
        assert!(p("user", &["*"], &[]).may_band("23cm", false, false));
        assert!(p("admin", &[], &[]).may_band("x", false, true));
    }

    #[test]
    fn decoder() {
        assert!(Principal::guest().may_decoder("aprs-144800", true));
        assert!(!Principal::guest().may_decoder("pocsag-1", false));
        assert!(p("user", &[], &["pocsag-1"]).may_decoder("pocsag-1", false));
        assert!(!p("user", &[], &["pocsag-1"]).may_decoder("adsb", false));
        assert!(p("admin", &[], &[]).may_decoder("adsb", false));
    }

    #[test]
    fn absender() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "8.8.8.8, 10.0.0.1".parse().unwrap());
        assert_eq!(client_ip(&h, Some("127.0.0.1:5000".parse().unwrap())), "8.8.8.8", "hinter lokalem Proxy");
        assert_eq!(client_ip(&h, Some("203.0.113.9:5000".parse().unwrap())), "203.0.113.9", "direkt aus dem Netz: Kopf ignoriert");
        h.insert("x-real-ip", "1.2.3.4".parse().unwrap());
        assert_eq!(client_ip(&h, Some("100.64.0.5:5000".parse().unwrap())), "1.2.3.4", "Tailscale-Proxy");
    }
}
