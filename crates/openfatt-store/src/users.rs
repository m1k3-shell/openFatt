//! Più utenti sullo stesso archivio: utenti con password e permessi per area, postazioni
//! collegate e blocco dei documenti aperti in modifica da un'altra postazione.
//!
//! Finché non si crea il primo utente l'archivio si apre senza accesso (uso monoutente).
//! Le password sono conservate come PBKDF2-HMAC-SHA256 con sale casuale.

use chrono::NaiveDateTime;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::{now, now_sql, Result, Store, StoreError};

/// Aree del programma a cui si può dare o togliere l'accesso.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum Area {
    Registry,
    Sales,
    Counter,
    Purchases,
    EInvoice,
    Stock,
    Accounting,
    Analysis,
    Settings,
}

impl Area {
    pub const ALL: [Area; 9] = [Area::Registry, Area::Sales, Area::Counter, Area::Purchases, Area::EInvoice, Area::Stock, Area::Accounting, Area::Analysis, Area::Settings];
    pub fn label(self) -> &'static str {
        match self {
            Area::Registry => "Anagrafiche e listini",
            Area::Sales => "Vendite",
            Area::Counter => "Vendita al banco",
            Area::Purchases => "Acquisti",
            Area::EInvoice => "Fattura elettronica",
            Area::Stock => "Magazzino e produzione",
            Area::Accounting => "Contabilità, IVA e banca",
            Area::Analysis => "Statistiche, agenti e agenda",
            Area::Settings => "Impostazioni, utenti e archivio",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Access {
    None,
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub full_name: String,
    pub admin: bool,
    pub permissions: Vec<(Area, Access)>,
    pub active: bool,
    /// "pbkdf2-sha256$<giri>$<sale esadecimale>$<hash esadecimale>"
    pub password_hash: String,
}

impl Default for User {
    fn default() -> Self {
        User { id: 0, username: String::new(), full_name: String::new(), admin: false, permissions: Area::ALL.iter().map(|a| (*a, if *a == Area::Settings { Access::None } else { Access::Write })).collect(), active: true, password_hash: String::new() }
    }
}

impl User {
    pub fn access(&self, a: Area) -> Access {
        if self.admin {
            return Access::Write;
        }
        self.permissions.iter().find(|(x, _)| *x == a).map(|(_, p)| *p).unwrap_or(Access::None)
    }
    pub fn can_read(&self, a: Area) -> bool {
        self.access(a) != Access::None
    }
    pub fn can_write(&self, a: Area) -> bool {
        self.access(a) == Access::Write
    }
}

/// Postazione collegata all'archivio.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: i64,
    pub username: String,
    pub host: String,
    pub started_at: String,
    pub seen_at: NaiveDateTime,
}

/// Dopo questo tempo senza segnali una postazione si considera scollegata.
pub const SESSION_TIMEOUT_SECS: i64 = 180;
/// Giri di PBKDF2-SHA256 per le password nuove (raccomandazione OWASP 2023). Le password
/// salvate con meno giri si aggiornano da sole al primo accesso riuscito.
const ROUNDS: u32 = 600_000;
/// Oltre questo numero di giri un hash salvato è considerato non valido (un valore enorme
/// inserito nell'archivio bloccherebbe il programma a ogni accesso).
const MAX_ROUNDS: u32 = 5_000_000;
/// Tentativi sbagliati consentiti prima dell'attesa obbligatoria.
const FREE_ATTEMPTS: u32 = 3;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Hash di riferimento per gli utenti inesistenti (stesso costo di uno vero).
const DUMMY_HASH: &str = "pbkdf2-sha256$600000$00000000000000000000000000000000$0000000000000000000000000000000000000000000000000000000000000000";

pub fn hash_password(password: &str) -> Result<String> {
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|e| StoreError::Invalid(format!("generatore casuale: {}", e)))?;
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, ROUNDS, &mut out);
    Ok(format!("pbkdf2-sha256${}${}${}", ROUNDS, hex(&salt), hex(&out)))
}

pub fn verify_password(password: &str, stored: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    if parts.len() != 4 || parts[0] != "pbkdf2-sha256" {
        return false;
    }
    let (Ok(rounds), Some(salt), Some(expected)) = (parts[1].parse::<u32>(), unhex(parts[2]), unhex(parts[3])) else { return false };
    if rounds == 0 || rounds > MAX_ROUNDS || expected.is_empty() || expected.len() > 64 {
        return false;
    }
    let mut out = vec![0u8; expected.len()];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, rounds, &mut out);
    // confronto a tempo costante
    out.len() == expected.len() && out.iter().zip(expected.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

impl Store {
    pub fn users(&self) -> Result<Vec<User>> {
        let mut st = self.conn().prepare("SELECT id, username, active, data FROM users ORDER BY username COLLATE NOCASE")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, active, data) = row?;
            let mut u: User = serde_json::from_str(&data)?;
            u.id = id;
            u.username = name;
            u.active = active != 0;
            out.push(u);
        }
        Ok(out)
    }

    pub fn has_users(&self) -> Result<bool> {
        Ok(self.conn().query_row("SELECT COUNT(*) FROM users WHERE active = 1", [], |r| r.get::<_, i64>(0))? > 0)
    }

    /// Salva l'utente; la password, se indicata, sostituisce quella attuale.
    pub fn save_user(&self, u: &User, new_password: Option<&str>) -> Result<User> {
        let mut u = u.clone();
        u.username = u.username.trim().to_string();
        if u.username.is_empty() || u.username.contains(char::is_whitespace) {
            return Err(StoreError::Invalid("Nome utente non valido (senza spazi)".into()));
        }
        if let Some(p) = new_password {
            if p.chars().count() < 8 {
                return Err(StoreError::Invalid("La password deve avere almeno 8 caratteri".into()));
            }
            u.password_hash = hash_password(p)?;
        }
        if u.password_hash.is_empty() {
            return Err(StoreError::Invalid("Indicare la password".into()));
        }
        // deve restare almeno un amministratore attivo
        let others_admin = self.users()?.iter().any(|x| x.id != u.id && x.admin && x.active);
        if !(u.admin && u.active) && !others_admin {
            return Err(StoreError::Invalid("Deve esserci almeno un amministratore attivo".into()));
        }
        let data = serde_json::to_string(&u)?;
        if u.id == 0 {
            self.conn()
                .execute("INSERT INTO users(username, active, data) VALUES (?1, ?2, ?3)", params![u.username, u.active as i64, data])
                .map_err(|e| match e {
                    rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => StoreError::Invalid(format!("Esiste già l'utente {}", u.username)),
                    other => other.into(),
                })?;
            u.id = self.conn().last_insert_rowid();
        } else {
            self.conn().execute("UPDATE users SET username=?1, active=?2, data=?3 WHERE id=?4", params![u.username, u.active as i64, data, u.id])?;
        }
        self.audit("user", u.id, "save", &u.username)?;
        Ok(u)
    }

    pub fn delete_user(&self, id: i64) -> Result<()> {
        let users = self.users()?;
        let Some(u) = users.iter().find(|x| x.id == id) else { return Ok(()) };
        if u.admin && !users.iter().any(|x| x.id != id && x.admin && x.active) {
            return Err(StoreError::Invalid("Non si può eliminare l'ultimo amministratore".into()));
        }
        self.conn().execute("DELETE FROM users WHERE id = ?1", [id])?;
        self.audit("user", id, "delete", &u.username)?;
        Ok(())
    }

    /// Verifica nome e password. Gli errori non dicono quale dei due è sbagliato, e il tempo
    /// di risposta è lo stesso anche per un utente che non esiste. Dopo alcuni tentativi
    /// sbagliati bisogna attendere, con un'attesa che raddoppia a ogni errore.
    pub fn login(&self, username: &str, password: &str) -> Result<User> {
        if let Some(wait) = self.login_wait() {
            return Err(StoreError::Invalid(format!("Troppi tentativi sbagliati: riprovare tra {} secondi", wait)));
        }
        let u = self.users()?.into_iter().find(|u| u.username.eq_ignore_ascii_case(username.trim()) && u.active);
        // per un utente inesistente si calcola comunque un hash: la risposta non lo rivela
        let ok = match &u {
            Some(u) => verify_password(password, &u.password_hash),
            None => {
                let _ = verify_password(password, DUMMY_HASH);
                false
            }
        };
        match u {
            Some(u) if ok => {
                self.login_failures.set((0, None));
                *self.user.borrow_mut() = u.username.clone();
                self.audit("user", u.id, "login", "")?;
                // hash con meno giri di quelli attuali: si rifà con la password appena verificata
                if u.password_hash.split('$').nth(1).and_then(|r| r.parse::<u32>().ok()).is_some_and(|r| r < ROUNDS) {
                    if let Ok(h) = hash_password(password) {
                        let _ = self.conn().execute("UPDATE users SET data = json_set(data, '$.password_hash', ?1) WHERE id = ?2", params![h, u.id]);
                    }
                }
                Ok(u)
            }
            _ => {
                let (n, _) = self.login_failures.get();
                let n = n + 1;
                let lock = (n >= FREE_ATTEMPTS).then(|| {
                    let secs = (5u64 << (n - FREE_ATTEMPTS).min(6)).min(300);
                    std::time::Instant::now() + std::time::Duration::from_secs(secs)
                });
                self.login_failures.set((n, lock));
                self.audit("user", 0, "login_failed", username.trim())?;
                Err(StoreError::Invalid("Nome utente o password non corretti".into()))
            }
        }
    }

    /// Secondi da attendere prima di un nuovo tentativo di accesso (None: si può provare).
    pub fn login_wait(&self) -> Option<u64> {
        let (_, until) = self.login_failures.get();
        let left = until?.checked_duration_since(std::time::Instant::now())?;
        Some(left.as_secs() + 1)
    }

    /// Utente collegato (vuoto se l'archivio non ha utenti).
    pub fn current_user(&self) -> String {
        self.user.borrow().clone()
    }

    // ------------------------------------------------------------------
    // Postazioni e blocchi
    // ------------------------------------------------------------------

    pub fn start_session(&self, host: &str) -> Result<i64> {
        let user = self.current_user();
        self.conn().execute("INSERT INTO sessions(username, host, started_at, seen_at) VALUES (?1, ?2, ?3, ?3)", params![user, host, now_sql()])?;
        let id = self.conn().last_insert_rowid();
        self.session.set(id);
        self.cleanup_sessions()?;
        Ok(id)
    }

    /// Segnale di presenza della postazione (da chiamare ogni minuto circa).
    pub fn heartbeat(&self) -> Result<()> {
        let id = self.session.get();
        if id != 0 {
            self.conn().execute("UPDATE sessions SET seen_at = ?1 WHERE id = ?2", params![now_sql(), id])?;
        }
        Ok(())
    }

    pub fn end_session(&self) -> Result<()> {
        let id = self.session.get();
        if id != 0 {
            self.conn().execute("DELETE FROM edit_locks WHERE session_id = ?1", [id])?;
            self.conn().execute("DELETE FROM sessions WHERE id = ?1", [id])?;
            self.session.set(0);
        }
        Ok(())
    }

    fn cleanup_sessions(&self) -> Result<()> {
        let limit = (now() - chrono::Duration::seconds(SESSION_TIMEOUT_SECS * 10)).format("%Y-%m-%d %H:%M:%S").to_string();
        self.conn().execute("DELETE FROM edit_locks WHERE session_id IN (SELECT id FROM sessions WHERE seen_at < ?1)", [&limit])?;
        self.conn().execute("DELETE FROM sessions WHERE seen_at < ?1", [&limit])?;
        Ok(())
    }

    /// Postazioni attive (con segnale recente).
    pub fn active_sessions(&self) -> Result<Vec<Session>> {
        let limit = now() - chrono::Duration::seconds(SESSION_TIMEOUT_SECS);
        let mut st = self.conn().prepare("SELECT id, username, host, started_at, seen_at FROM sessions ORDER BY started_at")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, u, h, s, seen) = row?;
            let seen = NaiveDateTime::parse_from_str(&seen, "%Y-%m-%d %H:%M:%S").unwrap_or_default();
            if seen >= limit {
                out.push(Session { id, username: u, host: h, started_at: s, seen_at: seen });
            }
        }
        Ok(out)
    }

    /// Blocca un elemento in modifica per questa postazione. Se è già aperto da un'altra
    /// postazione attiva restituisce chi lo sta modificando.
    pub fn acquire_lock(&self, entity: &str, id: i64) -> Result<Option<String>> {
        let me = self.session.get();
        if me == 0 || id == 0 {
            return Ok(None);
        }
        let owner: Option<i64> = self.conn().query_row("SELECT session_id FROM edit_locks WHERE entity = ?1 AND entity_id = ?2", params![entity, id], |r| r.get(0)).optional()?;
        if let Some(o) = owner {
            if o != me {
                if let Some(s) = self.active_sessions()?.into_iter().find(|s| s.id == o) {
                    let who = if s.username.is_empty() { s.host.clone() } else { format!("{} ({})", s.username, s.host) };
                    return Ok(Some(who));
                }
            }
        }
        self.conn().execute("INSERT OR REPLACE INTO edit_locks(entity, entity_id, session_id, since) VALUES (?1, ?2, ?3, ?4)", params![entity, id, me, now_sql()])?;
        Ok(None)
    }

    pub fn release_lock(&self, entity: &str, id: i64) -> Result<()> {
        self.conn().execute("DELETE FROM edit_locks WHERE entity = ?1 AND entity_id = ?2 AND session_id = ?3", params![entity, id, self.session.get()])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hashing() {
        let h = hash_password("segreta123").unwrap();
        assert!(verify_password("segreta123", &h));
        assert!(!verify_password("segreta124", &h));
        assert_ne!(h, hash_password("segreta123").unwrap(), "sale diverso a ogni salvataggio");
        assert!(!verify_password("x", "formato-sbagliato"));
        // un numero di giri enorme scritto nell'archivio non deve bloccare il programma
        assert!(!verify_password("x", "pbkdf2-sha256$4000000000$00$00"));
        assert!(!verify_password("x", "pbkdf2-sha256$0$00$00"));
        assert!(h.starts_with("pbkdf2-sha256$600000$"));
    }

    #[test]
    fn tentativi_sbagliati_e_password_aggiornate() {
        let s = Store::open_in_memory().unwrap();
        let admin = s.save_user(&User { username: "admin".into(), admin: true, ..Default::default() }, Some("password1")).unwrap();
        // password salvata da una versione precedente (meno giri): al primo accesso si aggiorna
        let mut salt = [0u8; 16];
        getrandom::getrandom(&mut salt).unwrap();
        let mut out = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<Sha256>(b"password1", &salt, 120_000, &mut out);
        let old = format!("pbkdf2-sha256$120000${}${}", hex(&salt), hex(&out));
        s.conn().execute("UPDATE users SET data = json_set(data, '$.password_hash', ?1) WHERE id = ?2", params![old, admin.id]).unwrap();
        s.login("admin", "password1").unwrap();
        let now: String = s.conn().query_row("SELECT json_extract(data, '$.password_hash') FROM users WHERE id = ?1", [admin.id], |r| r.get(0)).unwrap();
        assert!(now.starts_with("pbkdf2-sha256$600000$"), "{}", now);
        assert!(verify_password("password1", &now));
        // tre errori sono concessi, poi bisogna aspettare (anche con la password giusta)
        for _ in 0..3 {
            assert!(s.login("admin", "sbagliata").is_err());
        }
        assert!(s.login_wait().is_some());
        let e = s.login("admin", "password1").unwrap_err().to_string();
        assert!(e.contains("Troppi tentativi"), "{}", e);
        // un utente inesistente dà lo stesso messaggio di una password sbagliata
        s.login_failures.set((0, None));
        let a = s.login("nessuno", "x").unwrap_err().to_string();
        let b = s.login("admin", "x").unwrap_err().to_string();
        assert_eq!(a, b);
        s.login_failures.set((0, None));
        assert!(s.login("admin", "password1").is_ok());
    }

    #[test]
    fn users_permissions_login_and_locks() {
        let s = Store::open_in_memory().unwrap();
        assert!(!s.has_users().unwrap());
        assert!(s.save_user(&User { username: "mario".into(), ..Default::default() }, Some("password1")).is_err(), "il primo utente deve essere amministratore");
        let admin = s.save_user(&User { username: "admin".into(), admin: true, ..Default::default() }, Some("password1")).unwrap();
        let mut u = User { username: "cassa".into(), ..Default::default() };
        u.permissions = vec![(Area::Counter, Access::Write), (Area::Registry, Access::Read)];
        let u = s.save_user(&u, Some("banco2026")).unwrap();
        assert!(s.save_user(&User { username: "corta".into(), admin: true, ..Default::default() }, Some("1234")).is_err());
        assert!(s.login("CASSA", "sbagliata").is_err());
        let logged = s.login("Cassa", "banco2026").unwrap();
        assert!(logged.can_write(Area::Counter) && logged.can_read(Area::Registry) && !logged.can_write(Area::Registry) && !logged.can_read(Area::Accounting));
        assert_eq!(s.current_user(), "cassa");
        assert!(s.delete_user(admin.id).is_err());
        s.delete_user(u.id).unwrap();
        // due postazioni sullo stesso file
        let dir = std::env::temp_dir().join(format!("ga-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rete.gadb");
        let _ = std::fs::remove_file(&path);
        let a = Store::open(&path).unwrap();
        let b = Store::open(&path).unwrap();
        a.start_session("PC-UFFICIO").unwrap();
        b.start_session("PC-MAGAZZINO").unwrap();
        assert_eq!(a.active_sessions().unwrap().len(), 2);
        assert_eq!(a.acquire_lock("document", 7).unwrap(), None);
        assert_eq!(b.acquire_lock("document", 7).unwrap().as_deref(), Some("PC-UFFICIO"));
        a.release_lock("document", 7).unwrap();
        assert_eq!(b.acquire_lock("document", 7).unwrap(), None);
        b.end_session().unwrap();
        assert_eq!(a.active_sessions().unwrap().len(), 1);
        drop(a);
        drop(b);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
