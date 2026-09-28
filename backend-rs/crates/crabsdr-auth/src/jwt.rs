//! Sitzungs-Tokens (JWT, HS256). Regeln: docs/SECURITY.md.
//!
//! Im Token stehen nur Kennung, Name, Rolle, Art (`scope`) und der Sitzungszähler (`ep`) des Kontos – keine Rechte.
//! Der Server liest die Rechte bei jeder Anfrage aus der Datenbank und vergleicht den Sitzungszähler.

use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{info, warn};

/// Hören-Seite (auch für Admin-Konten ohne Admin-Rechte in der Schnittstelle)
pub const SCOPE_LISTEN: &str = "listen";
/// Admin-Seite (nur Admin-Konten)
pub const SCOPE_ADMIN: &str = "admin";
pub const TTL_LISTEN: u64 = 30 * 24 * 3600;
pub const TTL_ADMIN: u64 = 8 * 3600;
pub const TTL_GUEST: u64 = 7 * 24 * 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Kontokennung (Gäste: `guest-…`)
    pub sub: String,
    pub username: String,
    /// admin | user | guest
    pub role: String,
    /// listen | admin
    #[serde(default = "default_scope")]
    pub scope: String,
    /// Sitzungszähler des Kontos beim Ausstellen
    #[serde(default)]
    pub ep: i64,
    pub exp: u64,
    pub iat: u64,
}
fn default_scope() -> String { SCOPE_LISTEN.into() }

impl Claims {
    pub fn is_guest(&self) -> bool { self.role == "guest" }
}

pub struct JwtManager {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
}

fn now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() }

impl JwtManager {
    pub fn new(secret: &[u8]) -> Self {
        Self { encoding_key: EncodingKey::from_secret(secret), decoding_key: DecodingKey::from_secret(secret) }
    }

    /// Schlüssel aus der Umgebungsvariable, sonst aus `data_dir/.jwt_secret`, sonst neu (256 Bit, Datei mit Rechten 0600)
    pub fn load_or_generate(env_var: &str, data_dir: &Path) -> Self {
        if let Ok(s) = std::env::var(env_var) {
            if s.len() >= 32 { info!("Sitzungsschlüssel aus {}", env_var); return Self::new(s.as_bytes()); }
            if !s.is_empty() { warn!("{} ist kürzer als 32 Zeichen und wird ignoriert", env_var); }
        }
        let file = data_dir.join(".jwt_secret");
        if let Ok(s) = std::fs::read_to_string(&file) {
            let s = s.trim().to_string();
            if s.len() >= 32 { restrict(&file); return Self::new(s.as_bytes()); }
        }
        let mut raw = [0u8; 32];
        getrandom::getrandom(&mut raw).expect("Zufallsgenerator des Betriebssystems");
        let secret: String = raw.iter().map(|b| format!("{:02x}", b)).collect();
        let _ = std::fs::create_dir_all(data_dir);
        match std::fs::write(&file, &secret) {
            Ok(()) => { restrict(&file); info!("Sitzungsschlüssel erzeugt: {}", file.display()); }
            Err(e) => warn!("Sitzungsschlüssel nicht speicherbar ({}): {} – Sitzungen enden beim Neustart", file.display(), e),
        }
        Self::new(secret.as_bytes())
    }

    pub fn create_token(&self, sub: &str, username: &str, role: &str, scope: &str, ep: i64, ttl_secs: u64) -> Result<String, crate::db::AuthError> {
        let t = now();
        let c = Claims { sub: sub.into(), username: username.into(), role: role.into(), scope: scope.into(), ep, exp: t + ttl_secs, iat: t };
        encode(&Header::new(Algorithm::HS256), &c, &self.encoding_key).map_err(|e| crate::db::AuthError::Jwt(e.to_string()))
    }

    /// Signatur (nur HS256) und Ablauf prüfen. Ob das Konto noch gilt, prüft der Aufrufer mit der Datenbank.
    pub fn verify_token(&self, token: &str) -> Result<Claims, crate::db::AuthError> {
        let mut v = Validation::new(Algorithm::HS256);
        v.leeway = 30;
        v.validate_exp = true;
        decode::<Claims>(token, &self.decoding_key, &v).map(|d| d.claims).map_err(|e| crate::db::AuthError::Jwt(e.to_string()))
    }
}

/// Datei nur für den Eigentümer lesbar
fn restrict(p: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_ablauf_und_signatur() {
        let j = JwtManager::new(b"0123456789abcdef0123456789abcdef");
        let t = j.create_token("id1", "admin", "admin", SCOPE_ADMIN, 3, 60).unwrap();
        let c = j.verify_token(&t).unwrap();
        assert_eq!((c.sub.as_str(), c.scope.as_str(), c.ep), ("id1", SCOPE_ADMIN, 3));
        let andere = JwtManager::new(b"fedcba9876543210fedcba9876543210");
        assert!(andere.verify_token(&t).is_err());
        // abgelaufen
        let alt = j.create_token("id1", "admin", "admin", SCOPE_ADMIN, 3, 0).unwrap();
        let mut c2 = j.verify_token(&alt).unwrap(); c2.exp = 1;
        let abgelaufen = encode(&Header::new(Algorithm::HS256), &c2, &j.encoding_key).unwrap();
        assert!(j.verify_token(&abgelaufen).is_err());
        // alg=none wird abgelehnt
        let teile: Vec<&str> = t.split('.').collect();
        let none = format!("eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.{}.", teile[1]);
        assert!(j.verify_token(&none).is_err());
    }

    #[test]
    fn schluessel_datei() {
        let d = std::env::temp_dir().join(format!("crabsdr-jwt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let a = JwtManager::load_or_generate("CRABSDR_TEST_KEIN_ENV", &d);
        let t = a.create_token("x", "x", "user", SCOPE_LISTEN, 0, 60).unwrap();
        let b = JwtManager::load_or_generate("CRABSDR_TEST_KEIN_ENV", &d);   // gleicher Schlüssel aus der Datei
        assert!(b.verify_token(&t).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(d.join(".jwt_secret")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(std::fs::read_to_string(d.join(".jwt_secret")).unwrap().len(), 64);
    }
}
