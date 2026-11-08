//! Copie di sicurezza automatiche: preferenze, pianificazione e attività in sottofondo.
//! Il lavoro vero e le verifiche sono in `openfatt-backup`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use openfatt_backup::{Config, Dest, DestKind, DestState, JobDest, JobInput, JobResult, Progress, Status};
use openfatt_store::Store;

use crate::config;

const KEYRING: &str = "openfatt-backup";
/// Tra un tentativo automatico e l'altro, se qualcosa non è andato o il disco manca.
const RETRY_SECS: u64 = 300;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Health {
    Good,
    Warn,
    Bad,
    Running,
}

pub struct Running {
    pub progress: Progress,
    rx: Receiver<JobResult>,
}

pub struct Manager {
    pub cfg: Config,
    states: BTreeMap<String, DestState>,
    pub job: Option<Running>,
    uid: String,
    source: Option<(PathBuf, String)>,
    last_attempt: Option<Instant>,
    pub last_finished: Option<Instant>,
    pub fatal: Option<String>,
    pub banner_hidden: bool,
}

fn cfg_path() -> PathBuf {
    config::config_dir().join("backup.json")
}
fn state_path() -> PathBuf {
    config::config_dir().join("backup-stato.json")
}
pub fn spool_dir() -> PathBuf {
    config::config_dir().join("spool")
}

/// Scrittura che non può lasciare un file a metà.
fn write_atomic(path: &std::path::Path, data: &[u8]) {
    let part = path.with_extension("tmp");
    if std::fs::write(&part, data).is_ok() {
        let _ = std::fs::rename(&part, path);
    }
}

pub fn get_secret(dest: &str, what: &str) -> Option<String> {
    keyring::Entry::new(KEYRING, &format!("{}-{}", dest, what)).ok()?.get_password().ok()
}

pub fn set_secret(dest: &str, what: &str, value: &str) -> Result<(), String> {
    keyring::Entry::new(KEYRING, &format!("{}-{}", dest, what)).and_then(|e| e.set_password(value)).map_err(|e| format!("Gestore credenziali di Windows: {}", e))
}

fn delete_secret(dest: &str, what: &str) {
    if let Ok(e) = keyring::Entry::new(KEYRING, &format!("{}-{}", dest, what)) {
        let _ = e.delete_credential();
    }
}

impl Manager {
    pub fn load() -> Self {
        let cfg = std::fs::read(cfg_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let states = std::fs::read(state_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        // file temporanei rimasti da una chiusura improvvisa
        let _ = std::fs::remove_dir_all(spool_dir());
        Manager { cfg, states, job: None, uid: String::new(), source: None, last_attempt: None, last_finished: None, fatal: None, banner_hidden: false }
    }

    pub fn save_cfg(&self) {
        if let Ok(b) = serde_json::to_vec_pretty(&self.cfg) {
            write_atomic(&cfg_path(), &b);
        }
    }

    fn save_state(&self) {
        if let Ok(b) = serde_json::to_vec_pretty(&self.states) {
            write_atomic(&state_path(), &b);
        }
    }

    /// Da chiamare a ogni archivio aperto.
    pub fn on_archive_open(&mut self, store: &Store) {
        let mut uid: String = store.get_setting("archive_uid").unwrap_or_default();
        if uid.is_empty() {
            uid = openfatt_backup::new_id();
            let _ = store.set_setting("archive_uid", &uid);
        }
        self.uid = uid;
        let stem = store.path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        self.source = Some((store.path().to_path_buf(), stem));
        self.last_attempt = None;
        self.fatal = None;
        self.banner_hidden = false;
    }

    pub fn identity(&self) -> (String, String) {
        (self.source.as_ref().map(|s| s.1.clone()).unwrap_or_default(), self.uid.clone())
    }

    fn key(&self, dest: &str) -> String {
        format!("{}|{}", self.uid, dest)
    }

    pub fn state(&self, dest: &str) -> DestState {
        self.states.get(&self.key(dest)).cloned().unwrap_or_default()
    }

    pub fn active(&self) -> Vec<&Dest> {
        self.cfg.dests.iter().filter(|d| d.enabled).collect()
    }

    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    pub fn progress_text(&self) -> String {
        self.job.as_ref().and_then(|j| j.progress.lock().ok().map(|g| g.clone())).unwrap_or_default()
    }

    pub fn start(&mut self) -> bool {
        if self.job.is_some() {
            return false;
        }
        let Some((source, stem)) = self.source.clone() else { return false };
        if stem == ":memory:" || self.active().is_empty() {
            return false;
        }
        self.last_attempt = Some(Instant::now());
        let dests: Vec<JobDest> = self
            .active()
            .into_iter()
            .map(|d| JobDest { dest: d.clone(), password: if d.encrypt { get_secret(&d.id, "cifra") } else { None }, secret: if d.kind == DestKind::WebDav { get_secret(&d.id, "accesso") } else { None }, state: self.state(&d.id) })
            .collect();
        let input = JobInput { source, stem, uid: self.uid.clone(), spool: spool_dir(), retention: self.cfg.retention, dests, now: chrono::Local::now().naive_local(), app_version: env!("CARGO_PKG_VERSION").to_string() };
        let progress: Progress = Arc::new(Mutex::new("Avvio...".to_string()));
        let (tx, rx) = channel();
        let p = progress.clone();
        std::thread::spawn(move || {
            let _ = tx.send(openfatt_backup::run_job(input, &p));
        });
        self.job = Some(Running { progress, rx });
        true
    }

    fn overdue_days(&self, d: &Dest) -> Option<i64> {
        let now = chrono::Local::now().naive_local();
        self.state(&d.id).last_ok_at().map(|t| (now - t).num_days())
    }

    /// Destinazioni ferme da troppo tempo: (nome, motivo).
    pub fn overdue(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for d in self.active() {
            let st = self.state(&d.id);
            let late = match self.overdue_days(d) {
                Some(n) => n >= self.cfg.alert_days as i64,
                None => st.last_try.is_some(),
            };
            if late && !st.last_error.is_empty() {
                out.push((d.name.clone(), st.last_error.clone()));
            } else if late {
                out.push((d.name.clone(), "nessuna copia recente".to_string()));
            }
        }
        out
    }

    /// Controlla la fine del lavoro in corso e, se è ora, ne avvia uno nuovo.
    pub fn tick(&mut self, msgs: &mut crate::ui::Msgs) {
        self.poll(msgs);
        if self.job.is_some() || !self.cfg.enabled || self.source.is_none() {
            return;
        }
        if self.last_attempt.map(|t| t.elapsed().as_secs() < RETRY_SECS).unwrap_or(false) {
            return;
        }
        let now = chrono::Local::now().naive_local();
        let hours = self.cfg.interval_hours.max(1) as i64;
        let stale = self.active().iter().any(|d| {
            let fresh = self.state(&d.id).last_ok_at().map(|t| (now - t).num_minutes() < hours * 60).unwrap_or(false);
            !fresh && openfatt_backup::reachable(d)
        });
        if stale {
            self.start();
        } else {
            self.last_attempt = Some(Instant::now());
        }
    }

    pub fn poll(&mut self, msgs: &mut crate::ui::Msgs) -> bool {
        let Some(j) = &self.job else { return false };
        let Ok(res) = j.rx.try_recv() else { return false };
        self.job = None;
        self.last_finished = Some(Instant::now());
        self.fatal = res.fatal.clone();
        if let Some(f) = &res.fatal {
            msgs.err(format!("Copia di sicurezza non riuscita. {}", f));
        }
        for o in res.outcomes {
            let name = self.cfg.dests.iter().find(|d| d.id == o.id).map(|d| d.name.clone()).unwrap_or_default();
            if let Some(p) = &o.moved_to {
                if let Some(d) = self.cfg.dests.iter_mut().find(|d| d.id == o.id) {
                    d.path = p.clone();
                }
                self.save_cfg();
            }
            if o.status == Status::Failed {
                msgs.warn(format!("Copia su {} non riuscita: {}", name, o.message));
            }
            if !o.state.warning.is_empty() {
                msgs.warn(format!("{}: {}", name, o.state.warning));
            }
            let k = self.key(&o.id);
            self.states.insert(k, o.state);
        }
        self.save_state();
        true
    }

    pub fn remove(&mut self, id: &str) {
        self.cfg.dests.retain(|d| d.id != id);
        delete_secret(id, "cifra");
        delete_secret(id, "accesso");
        self.save_cfg();
    }

    /// Stato da mostrare in fondo alla finestra.
    pub fn summary(&self) -> (Health, String) {
        if self.running() {
            return (Health::Running, self.progress_text());
        }
        if let Some(f) = &self.fatal {
            return (Health::Bad, f.clone());
        }
        if !self.cfg.enabled {
            return (Health::Bad, "Copie automatiche disattivate".into());
        }
        if self.active().is_empty() {
            return (Health::Warn, "Copie esterne da configurare".into());
        }
        if let Some((n, _)) = self.overdue().first() {
            return (Health::Bad, format!("Copia su {} non aggiornata", n));
        }
        if let Some(w) = self.active().iter().map(|d| self.state(&d.id).warning).find(|w| !w.is_empty()) {
            return (Health::Warn, w);
        }
        let last = self.active().iter().filter_map(|d| self.state(&d.id).last_ok_at()).max();
        match last {
            Some(t) if t.date() == chrono::Local::now().date_naive() => (Health::Good, format!("Copia verificata oggi alle {}", t.format("%H:%M"))),
            Some(t) => (Health::Good, format!("Copia verificata il {}", t.format("%d/%m/%Y"))),
            None => (Health::Running, "Prima copia in preparazione".into()),
        }
    }

    /// Avviso ben visibile: nessuna destinazione o copie ferme da troppo.
    pub fn banner(&self) -> Option<String> {
        if self.banner_hidden || self.running() {
            return None;
        }
        if let Some(f) = &self.fatal {
            return Some(format!("Copia di sicurezza non riuscita: {}", f));
        }
        if !self.cfg.enabled {
            return Some("Le copie di sicurezza automatiche sono disattivate.".into());
        }
        if self.active().is_empty() {
            return Some("I dati sono solo su questo computer: scegli un hard disk esterno o un cloud per le copie di sicurezza.".into());
        }
        let o = self.overdue();
        o.first().map(|(n, why)| format!("La copia su {} non è aggiornata: {}", n, why))
    }
}

/// Cartelle dei servizi cloud installati su questo computer.
pub fn cloud_folders() -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let mut add = |name: &str, p: PathBuf| {
        if p.is_dir() && !out.iter().any(|(_, q)| *q == p) {
            out.push((name.to_string(), p));
        }
    };
    for (v, name) in [("OneDriveCommercial", "OneDrive aziendale"), ("OneDriveConsumer", "OneDrive"), ("OneDrive", "OneDrive")] {
        if let Some(p) = std::env::var_os(v) {
            add(name, PathBuf::from(p));
        }
    }
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        add("Dropbox", home.join("Dropbox"));
        add("Google Drive", home.join("Google Drive"));
        add("Google Drive", home.join("Il mio Drive"));
        add("iCloud Drive", home.join("iCloudDrive"));
    }
    for l in b'D'..=b'Z' {
        for sub in ["Il mio Drive", "My Drive"] {
            add("Google Drive", PathBuf::from(format!("{}:\\{}", l as char, sub)));
        }
    }
    out
}
