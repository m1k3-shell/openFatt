//! Preferenze locali dell'applicazione (archivi recenti), cartelle dei dati, copie di
//! sicurezza automatiche e password PEC nel Gestore credenziali di Windows.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub recent: Vec<PathBuf>,
    pub last: Option<PathBuf>,
}

/// Dove stanno i dati (vedi anche README, sezione "Dove sono i dati"):
///
/// - **archivi**: un file `.gadb` per azienda, per impostazione in `Documenti\openFatt`
///   (si può aprire o creare un archivio in qualunque cartella, anche di rete);
/// - **copie di sicurezza automatiche**: `Documenti\openFatt\Copie di sicurezza`, una al
///   giorno alla chiusura del programma, conservate le ultime 15 per archivio;
/// - **preferenze del programma** (archivi recenti): `%APPDATA%\openFatt\config.json`;
/// - **password** di PEC, email e negozio online: Gestore credenziali di Windows, mai nel file.
///
/// La variabile d'ambiente OPENFATT_DATA sostituisce la cartella dei documenti.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let d = base.join("openFatt");
    // prima versione: preferenze in %APPDATA%\openFatt
    let old = base.join("GestionaleAperto");
    if !d.exists() && old.join("config.json").exists() {
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::copy(old.join("config.json"), d.join("config.json"));
    }
    d
}

pub fn documents_dir() -> PathBuf {
    let d = match std::env::var_os("OPENFATT_DATA") {
        Some(p) => PathBuf::from(p),
        None => {
            let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            home.join("Documents").join("openFatt")
        }
    };
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn backup_dir() -> PathBuf {
    let d = documents_dir().join("Copie di sicurezza");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Copia di sicurezza automatica: al massimo una al giorno per archivio, si tengono le
/// ultime `keep`. Restituisce il file creato.
pub fn auto_backup(store: &openfatt_store::Store, keep: usize) -> Option<PathBuf> {
    let stem = store.path().file_stem()?.to_string_lossy().to_string();
    if stem == ":memory:" {
        return None;
    }
    let dir = backup_dir();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let target = dir.join(format!("{} {}.gadb", stem, today));
    if !target.exists() {
        store.backup_to(&target).ok()?;
    }
    let prefix = format!("{} ", stem);
    let mut copies: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            n.starts_with(&prefix) && n.ends_with(".gadb") && n.len() == prefix.len() + 15
        })
        .collect();
    copies.sort();
    while copies.len() > keep {
        let old = copies.remove(0);
        let _ = std::fs::remove_file(old);
    }
    Some(target)
}

impl AppConfig {
    pub fn load() -> Self {
        std::fs::read_to_string(config_dir().join("config.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let d = config_dir();
        let _ = std::fs::create_dir_all(&d);
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(d.join("config.json"), s);
        }
    }

    pub fn touch(&mut self, p: &PathBuf) {
        self.recent.retain(|r| r != p);
        self.recent.insert(0, p.clone());
        self.recent.truncate(8);
        self.last = Some(p.clone());
        self.save();
    }
}

const KEYRING_SERVICE: &str = "gestionale-aperto-pec";

pub fn load_password(address: &str) -> Option<String> {
    if address.is_empty() {
        return None;
    }
    keyring::Entry::new(KEYRING_SERVICE, address).ok()?.get_password().ok()
}

pub fn store_password(address: &str, password: &str) -> Result<(), String> {
    let e = keyring::Entry::new(KEYRING_SERVICE, address).map_err(|e| e.to_string())?;
    e.set_password(password).map_err(|e| e.to_string())
}

pub fn delete_password(address: &str) {
    if let Ok(e) = keyring::Entry::new(KEYRING_SERVICE, address) {
        let _ = e.delete_credential();
    }
}
