//! Benutzerdatenbank (SQLite). Regeln: docs/SECURITY.md.
//!
//! Passwörter: bcrypt. Hashen und Prüfen sind reine Funktionen ([`hash_password`], [`verify_password`]) und laufen
//! beim Aufrufer in einem eigenen Thread – die Datenbank wird dafür nicht gesperrt.
//! Jedes Konto hat einen Sitzungszähler (`token_epoch`): Passwortwechsel, Sperren, Rollenwechsel und Löschen erhöhen
//! ihn, damit werden alle ausgegebenen Tokens ungültig.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::sync::OnceLock;
use tracing::info;

pub const ROLE_ADMIN: &str = "admin";
pub const ROLE_USER: &str = "user";
/// Rechte-Listen: dieser Eintrag heißt „alle Bänder“ bzw. „alle Decoder“
pub const ALL: &str = "*";
const BCRYPT_COST: u32 = 12;

/// Konto mit Hash (nur intern)
#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub role: String,
    pub active: bool,
    pub must_change_pw: bool,
    pub token_epoch: i64,
}

/// Konto für Antworten der Schnittstelle (ohne Hash)
#[derive(Debug, Clone, Serialize)]
pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub role: String,
    pub active: bool,
    pub must_change_pw: bool,
    pub created_at: String,
    pub last_login: Option<String>,
    pub bands: Vec<String>,
    pub decoders: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Datenbank: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("bcrypt: {0}")]
    Bcrypt(String),
    #[error("Token: {0}")]
    Jwt(String),
    #[error("nicht gefunden")]
    NotFound,
}

// ---------- Regeln für Namen, Rollen, Passwörter ----------

/// 3–32 Zeichen: Buchstaben, Ziffern, Punkt, Bindestrich, Unterstrich (Rufzeichen passen, z. B. DO1XX, dl1abc-2)
pub fn validate_username(name: &str) -> Result<(), AuthError> {
    let ok = (3..=32).contains(&name.chars().count()) && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if ok { Ok(()) } else { Err(AuthError::Invalid("Benutzername: 3–32 Zeichen, nur Buchstaben, Ziffern, . - _".into())) }
}

pub fn validate_role(role: &str) -> Result<(), AuthError> {
    if role == ROLE_ADMIN || role == ROLE_USER { Ok(()) } else { Err(AuthError::Invalid("Rolle: admin oder user".into())) }
}

/// Mindestens 10 Zeichen, höchstens 72 Byte (bcrypt würde längere still kürzen)
pub fn validate_password(pw: &str) -> Result<(), AuthError> {
    if pw.chars().count() < 10 { return Err(AuthError::Invalid("Passwort: mindestens 10 Zeichen".into())); }
    if pw.len() > 72 { return Err(AuthError::Invalid("Passwort: höchstens 72 Byte".into())); }
    Ok(())
}

/// Einträge einer Rechte-Liste (Band- oder Decoder-Kennungen, oder `*`)
fn validate_ids(ids: &[String]) -> Result<(), AuthError> {
    for id in ids {
        let ok = !id.is_empty() && id.len() <= 64 && (id == ALL || id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')));
        if !ok { return Err(AuthError::Invalid(format!("ungültige Kennung „{}“", id))); }
    }
    Ok(())
}

// ---------- Passwort-Funktionen (blockierend, im eigenen Thread aufrufen) ----------

pub fn hash_password(pw: &str) -> Result<String, AuthError> {
    validate_password(pw)?;
    bcrypt::hash(pw, BCRYPT_COST).map_err(|e| AuthError::Bcrypt(e.to_string()))
}

static DUMMY: OnceLock<String> = OnceLock::new();
fn dummy_hash() -> &'static str { DUMMY.get_or_init(|| bcrypt::hash(random_password(16), BCRYPT_COST).unwrap_or_default()) }

/// Platzhalter-Hash beim Start erzeugen, damit auch die erste Anmeldung nicht verrät, ob es den Namen gibt
pub fn warm_up() { let _ = dummy_hash(); }

/// Prüft ein Passwort. Ohne Hash (Konto unbekannt) wird gegen einen Platzhalter geprüft, damit die Antwort gleich lange
/// dauert und nicht verrät, ob es den Namen gibt.
pub fn verify_password(pw: &str, hash: Option<&str>) -> bool {
    let dummy = dummy_hash();
    let target = hash.unwrap_or(dummy);
    let ok = pw.len() <= 72 && bcrypt::verify(pw, target).unwrap_or(false);
    ok && hash.is_some()
}

/// Zufallspasswort aus dem Zufallsgenerator des Betriebssystems, ohne verwechselbare Zeichen (0/O, 1/l/I)
pub fn random_password(len: usize) -> String {
    const CHARS: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut out = String::with_capacity(len);
    let mut buf = [0u8; 1];
    while out.len() < len {
        getrandom::getrandom(&mut buf).expect("Zufallsgenerator des Betriebssystems");
        // Ablehnen statt Modulo: keine Häufung einzelner Zeichen
        let lim = 256 - (256 % CHARS.len());
        if (buf[0] as usize) < lim { out.push(CHARS[buf[0] as usize % CHARS.len()] as char); }
    }
    out
}

// ---------- Datenbank ----------

pub struct AuthDb {
    conn: Connection,
}

const USER_COLS: &str = "id, username, password_hash, role, active, must_change_pw, token_epoch";

fn row_user(row: &rusqlite::Row) -> rusqlite::Result<User> {
    Ok(User {
        id: row.get(0)?, username: row.get(1)?, password_hash: row.get(2)?, role: row.get(3)?,
        active: row.get::<_, i64>(4)? != 0, must_change_pw: row.get::<_, i64>(5)? != 0, token_epoch: row.get(6)?,
    })
}

impl AuthDb {
    pub fn open(path: &Path) -> Result<Self, AuthError> {
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                username TEXT UNIQUE NOT NULL,
                password_hash TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'user',
                active INTEGER DEFAULT 1,
                created_at TEXT DEFAULT (datetime('now')),
                last_login TEXT);
             CREATE TABLE IF NOT EXISTS user_permissions (
                user_id TEXT REFERENCES users(id) ON DELETE CASCADE,
                sdr_id TEXT NOT NULL,
                PRIMARY KEY (user_id, sdr_id));
             CREATE TABLE IF NOT EXISTS user_decoder_permissions (
                user_id TEXT REFERENCES users(id) ON DELETE CASCADE,
                decoder_name TEXT NOT NULL,
                PRIMARY KEY (user_id, decoder_name));",
        )?;
        // Spalten der Version 0.2 nachrüsten
        let cols: Vec<String> = conn.prepare("PRAGMA table_info(users)")?.query_map([], |r| r.get::<_, String>(1))?.flatten().collect();
        if !cols.iter().any(|c| c == "token_epoch") {
            conn.execute("ALTER TABLE users ADD COLUMN token_epoch INTEGER NOT NULL DEFAULT 0", [])?;
        }
        if !cols.iter().any(|c| c == "must_change_pw") {
            conn.execute("ALTER TABLE users ADD COLUMN must_change_pw INTEGER NOT NULL DEFAULT 0", [])?;
            // Startpasswort, das nie benutzt wurde, gilt als unsicher (stand im Protokoll)
            conn.execute("UPDATE users SET must_change_pw = 1 WHERE last_login IS NULL", [])?;
        }
        Ok(Self { conn })
    }

    /// Erster Start: Admin `admin` mit Zufallspasswort anlegen (muss beim ersten Anmelden geändert werden).
    /// Gibt das Passwort zurück, damit es einmal im Protokoll erscheint.
    pub fn bootstrap_admin(&self) -> Result<Option<String>, AuthError> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        if n > 0 { return Ok(None); }
        let pw = random_password(16);
        self.create_user("admin", &hash_password(&pw)?, ROLE_ADMIN, true)?;
        info!("Admin-Konto „admin“ angelegt");
        Ok(Some(pw))
    }

    pub fn get_user(&self, id: &str) -> Result<Option<User>, AuthError> {
        Ok(self.conn.query_row(&format!("SELECT {} FROM users WHERE id = ?1", USER_COLS), params![id], row_user).optional()?)
    }

    pub fn get_user_by_name(&self, username: &str) -> Result<Option<User>, AuthError> {
        Ok(self.conn.query_row(&format!("SELECT {} FROM users WHERE username = ?1 COLLATE NOCASE", USER_COLS), params![username], row_user).optional()?)
    }

    pub fn touch_login(&self, id: &str) -> Result<(), AuthError> {
        self.conn.execute("UPDATE users SET last_login = datetime('now') WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn list_ids(&self, sql: &str, id: &str) -> Result<Vec<String>, AuthError> {
        let mut st = self.conn.prepare(sql)?;
        let v = st.query_map(params![id], |r| r.get::<_, String>(0))?.flatten().collect();
        Ok(v)
    }
    pub fn bands_of(&self, id: &str) -> Result<Vec<String>, AuthError> {
        self.list_ids("SELECT sdr_id FROM user_permissions WHERE user_id = ?1 ORDER BY sdr_id", id)
    }
    pub fn decoders_of(&self, id: &str) -> Result<Vec<String>, AuthError> {
        self.list_ids("SELECT decoder_name FROM user_decoder_permissions WHERE user_id = ?1 ORDER BY decoder_name", id)
    }

    pub fn info(&self, id: &str) -> Result<Option<UserInfo>, AuthError> {
        let row = self.conn.query_row(
            "SELECT id, username, role, active, must_change_pw, created_at, last_login FROM users WHERE id = ?1", params![id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)? != 0,
                    r.get::<_, i64>(4)? != 0, r.get::<_, String>(5)?, r.get::<_, Option<String>>(6)?))).optional()?;
        let Some((id, username, role, active, must_change_pw, created_at, last_login)) = row else { return Ok(None) };
        let (bands, decoders) = (self.bands_of(&id)?, self.decoders_of(&id)?);
        Ok(Some(UserInfo { id, username, role, active, must_change_pw, created_at, last_login, bands, decoders }))
    }

    pub fn list(&self) -> Result<Vec<UserInfo>, AuthError> {
        let ids: Vec<String> = self.conn.prepare("SELECT id FROM users ORDER BY username COLLATE NOCASE")?
            .query_map([], |r| r.get::<_, String>(0))?.flatten().collect();
        let mut out = Vec::new();
        for id in ids { if let Some(u) = self.info(&id)? { out.push(u); } }
        Ok(out)
    }

    pub fn count_active_admins(&self) -> Result<i64, AuthError> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM users WHERE role = 'admin' AND active = 1", [], |r| r.get(0))?)
    }

    /// Neues Konto; `hash` von [`hash_password`]
    pub fn create_user(&self, username: &str, hash: &str, role: &str, must_change: bool) -> Result<String, AuthError> {
        validate_username(username)?; validate_role(role)?;
        if self.get_user_by_name(username)?.is_some() { return Err(AuthError::Invalid("Benutzername ist vergeben".into())); }
        let id = uuid::Uuid::new_v4().to_string();
        self.conn.execute("INSERT INTO users (id, username, password_hash, role, must_change_pw) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, username, hash, role, must_change as i64])?;
        Ok(id)
    }

    fn bump(&self, id: &str) -> Result<(), AuthError> {
        let n = self.conn.execute("UPDATE users SET token_epoch = token_epoch + 1 WHERE id = ?1", params![id])?;
        if n == 0 { Err(AuthError::NotFound) } else { Ok(()) }
    }

    /// Neues Passwort (Hash); meldet alle Sitzungen ab. `must_change`: beim nächsten Anmelden erneut ändern
    pub fn set_password(&self, id: &str, hash: &str, must_change: bool) -> Result<(), AuthError> {
        self.conn.execute("UPDATE users SET password_hash = ?1, must_change_pw = ?2 WHERE id = ?3", params![hash, must_change as i64, id])?;
        self.bump(id)
    }

    pub fn set_role(&self, id: &str, role: &str) -> Result<(), AuthError> {
        validate_role(role)?;
        self.conn.execute("UPDATE users SET role = ?1 WHERE id = ?2", params![role, id])?;
        self.bump(id)
    }

    pub fn set_active(&self, id: &str, active: bool) -> Result<(), AuthError> {
        self.conn.execute("UPDATE users SET active = ?1 WHERE id = ?2", params![active as i64, id])?;
        self.bump(id)
    }

    pub fn rename(&self, id: &str, username: &str) -> Result<(), AuthError> {
        validate_username(username)?;
        if let Some(o) = self.get_user_by_name(username)? { if o.id != id { return Err(AuthError::Invalid("Benutzername ist vergeben".into())); } }
        self.conn.execute("UPDATE users SET username = ?1 WHERE id = ?2", params![username, id])?;
        self.bump(id)
    }

    fn set_list(&self, table: &str, col: &str, id: &str, ids: &[String]) -> Result<(), AuthError> {
        validate_ids(ids)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(&format!("DELETE FROM {} WHERE user_id = ?1", table), params![id])?;
        for x in ids { tx.execute(&format!("INSERT OR IGNORE INTO {} (user_id, {}) VALUES (?1, ?2)", table, col), params![id, x])?; }
        tx.commit()?;
        Ok(())
    }
    pub fn set_bands(&self, id: &str, bands: &[String]) -> Result<(), AuthError> { self.set_list("user_permissions", "sdr_id", id, bands) }
    pub fn set_decoders(&self, id: &str, decs: &[String]) -> Result<(), AuthError> { self.set_list("user_decoder_permissions", "decoder_name", id, decs) }

    pub fn delete_user(&self, id: &str) -> Result<(), AuthError> {
        self.conn.execute("DELETE FROM user_decoder_permissions WHERE user_id = ?1", params![id])?;
        self.conn.execute("DELETE FROM user_permissions WHERE user_id = ?1", params![id])?;
        let n = self.conn.execute("DELETE FROM users WHERE id = ?1", params![id])?;
        if n == 0 { Err(AuthError::NotFound) } else { Ok(()) }
    }

    /// Notfall am Rechner (`crabsdr-server --reset-admin`): `admin` bekommt ein neues Zufallspasswort (wird angelegt,
    /// falls es fehlt), ist wieder aktiv und Admin, alle Sitzungen sind abgemeldet.
    pub fn reset_admin(&self) -> Result<String, AuthError> {
        let pw = random_password(16);
        let hash = hash_password(&pw)?;
        match self.get_user_by_name("admin")? {
            Some(u) => {
                self.conn.execute("UPDATE users SET password_hash = ?1, must_change_pw = 1, role = 'admin', active = 1 WHERE id = ?2", params![hash, u.id])?;
                self.bump(&u.id)?;
            }
            None => { self.create_user("admin", &hash, ROLE_ADMIN, true)?; }
        }
        Ok(pw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> AuthDb {
        let p = std::env::temp_dir().join(format!("crabsdr-auth-{}-{}.db", std::process::id(), random_password(8)));
        AuthDb::open(&p).unwrap()
    }

    #[test]
    fn regeln() {
        assert!(validate_username("DO1XX").is_ok() && validate_username("dl1abc-2").is_ok());
        assert!(validate_username("ab").is_err() && validate_username("a b c").is_err() && validate_username("<script>").is_err());
        assert!(validate_password("kurz").is_err() && validate_password(&"x".repeat(73)).is_err() && validate_password("zehnzeichen").is_ok());
        assert!(validate_role("admin").is_ok() && validate_role("root").is_err());
        let p = random_password(16);
        assert_eq!(p.len(), 16);
        assert_ne!(p, random_password(16));
    }

    #[test]
    fn konto_ablauf() {
        let d = db();
        let pw = d.bootstrap_admin().unwrap().unwrap();
        assert!(d.bootstrap_admin().unwrap().is_none());
        let a = d.get_user_by_name("ADMIN").unwrap().unwrap();          // Name ohne Groß/klein
        assert!(a.must_change_pw && a.role == ROLE_ADMIN);
        assert!(verify_password(&pw, Some(&a.password_hash)));
        assert!(!verify_password("falsch-falsch", Some(&a.password_hash)));
        assert!(!verify_password(&pw, None));                           // unbekanntes Konto
        let e0 = a.token_epoch;
        d.set_password(&a.id, &hash_password("neues-passwort-1").unwrap(), false).unwrap();
        let a2 = d.get_user(&a.id).unwrap().unwrap();
        assert!(a2.token_epoch > e0 && !a2.must_change_pw);
        let u = d.create_user("dl1abc", &hash_password("passwort-dl1abc").unwrap(), ROLE_USER, false).unwrap();
        assert!(d.create_user("DL1ABC", "x", ROLE_USER, false).is_err());  // vergeben
        d.set_bands(&u, &["2m".into(), "70cm".into()]).unwrap();
        d.set_decoders(&u, &[ALL.into()]).unwrap();
        assert!(d.set_bands(&u, &["x y".into()]).is_err());
        let i = d.info(&u).unwrap().unwrap();
        assert_eq!(i.bands, vec!["2m".to_string(), "70cm".to_string()]);
        assert_eq!(d.count_active_admins().unwrap(), 1);
        d.delete_user(&u).unwrap();
        assert!(d.info(&u).unwrap().is_none());
        let pw2 = d.reset_admin().unwrap();
        let a3 = d.get_user(&a.id).unwrap().unwrap();
        assert!(a3.must_change_pw && a3.token_epoch > a2.token_epoch && verify_password(&pw2, Some(&a3.password_hash)));
    }
}
