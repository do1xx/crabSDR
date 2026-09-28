//! Bremse für Anmeldeversuche (docs/SECURITY.md): Fehlversuche je Benutzername, je Absender und insgesamt, jeweils in
//! einem gleitenden Zeitfenster. Die Sperre je Name wirkt auch dann, wenn ein Angreifer Adressen wechselt oder fälscht.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct Rule { max: usize, window: Duration }

pub struct LoginLimiter {
    per_user: Rule,
    per_ip: Rule,
    global: Rule,
    hits: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl Default for LoginLimiter {
    fn default() -> Self {
        Self::new((10, 15 * 60), (20, 15 * 60), (60, 60))
    }
}

impl LoginLimiter {
    /// (Anzahl, Sekunden) je Name, je Adresse, insgesamt
    pub fn new(user: (usize, u64), ip: (usize, u64), global: (usize, u64)) -> Self {
        let r = |(m, s): (usize, u64)| Rule { max: m, window: Duration::from_secs(s) };
        Self { per_user: r(user), per_ip: r(ip), global: r(global), hits: Mutex::new(HashMap::new()) }
    }

    fn keys(&self, user: &str, ip: &str) -> [(String, Rule); 3] {
        [(format!("u:{}", user.to_lowercase()), self.per_user), (format!("i:{}", ip), self.per_ip), ("*".to_string(), self.global)]
    }

    /// Darf ein Versuch stattfinden? Sonst Sekunden bis zum nächsten erlaubten Versuch.
    pub fn check(&self, user: &str, ip: &str) -> Result<(), u64> {
        let now = Instant::now();
        let mut g = self.hits.lock().unwrap();
        let mut wait = 0u64;
        for (k, rule) in self.keys(user, ip) {
            if let Some(q) = g.get_mut(&k) {
                while q.front().map_or(false, |t| now.duration_since(*t) > rule.window) { q.pop_front(); }
                if q.len() >= rule.max {
                    let oldest = *q.front().unwrap();
                    wait = wait.max(rule.window.saturating_sub(now.duration_since(oldest)).as_secs() + 1);
                }
            }
        }
        if wait > 0 { Err(wait) } else { Ok(()) }
    }

    pub fn failure(&self, user: &str, ip: &str) {
        let now = Instant::now();
        let mut g = self.hits.lock().unwrap();
        if g.len() > 20_000 {
            // Speicher begrenzen: abgelaufene Einträge weg (längstes Fenster = 15 min)
            g.retain(|_, q| q.back().map_or(false, |t| now.duration_since(*t) < Duration::from_secs(15 * 60)));
        }
        for (k, _) in self.keys(user, ip) { g.entry(k).or_default().push_back(now); }
    }

    /// Erfolgreiche Anmeldung: Fehlversuche dieses Namens vergessen (Adresse und Gesamtzahl bleiben)
    pub fn success(&self, user: &str) {
        self.hits.lock().unwrap().remove(&format!("u:{}", user.to_lowercase()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sperre_je_name_adresse_gesamt() {
        let l = LoginLimiter::new((3, 60), (5, 60), (8, 60));
        for _ in 0..3 { assert!(l.check("admin", "1.2.3.4").is_ok()); l.failure("admin", "1.2.3.4"); }
        assert!(l.check("Admin", "9.9.9.9").is_err(), "Name gesperrt, auch von anderer Adresse und in anderer Schreibweise");
        assert!(l.check("dl1abc", "1.2.3.4").is_ok());
        l.failure("dl1abc", "1.2.3.4"); l.failure("dl1abc", "1.2.3.4");
        assert!(l.check("dk0xyz", "1.2.3.4").is_err(), "Adresse gesperrt nach 5 Fehlversuchen");
        for i in 0..3 { l.failure(&format!("u{}", i), &format!("5.5.5.{}", i)); }
        assert!(l.check("neu", "7.7.7.7").is_err(), "Gesamtgrenze erreicht");
        let w = l.check("neu", "7.7.7.7").unwrap_err();
        assert!(w >= 1 && w <= 61);
    }

    #[test]
    fn erfolg_setzt_namen_zurueck() {
        let l = LoginLimiter::new((2, 60), (10, 60), (100, 60));
        l.failure("admin", "1.1.1.1");
        l.success("admin");
        l.failure("admin", "1.1.1.1");
        assert!(l.check("admin", "1.1.1.1").is_ok());
    }
}
