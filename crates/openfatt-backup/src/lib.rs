//! Copie di sicurezza pensate per non perdere dati.
//!
//! Regole che valgono sempre:
//! - la copia si ricava con SQLite da un'istantanea coerente, anche con l'archivio in uso, e
//!   si controlla (integrità e conteggi) **prima** di mandarla da qualche parte;
//! - ogni destinazione riceve il file sotto un nome provvisorio, lo si rilegge e se ne
//!   confronta l'impronta SHA-256; solo allora prende il nome definitivo, quindi una copia
//!   interrotta non può essere scambiata per una buona;
//! - una destinazione che non funziona non ferma le altre;
//! - le copie vecchie si eliminano solo dopo che la nuova è stata verificata, seguendo una
//!   rotazione (giornaliere, settimanali, mensili, annuali) e mai la più recente;
//! - un disco esterno si riconosce da un file di riconoscimento e non dalla lettera di unità:
//!   su una chiavetta diversa non si scrive nulla.

mod crypto;
mod target;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{Datelike, NaiveDateTime};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use crypto::{decrypt_file, encrypt_file};
use target::{open_target, Target};

/// Nome del file di riconoscimento nella cartella di destinazione.
pub const MARKER: &str = ".openfatt-backup";
const TIME_FMT: &str = "%Y-%m-%d %H:%M:%S";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum DestKind {
    /// Hard disk esterno, chiavetta, NAS o altra cartella
    #[default]
    Folder,
    /// Cartella sincronizzata da OneDrive, Google Drive, Dropbox...
    CloudFolder,
    /// Cloud con accesso WebDAV (Nextcloud e altri)
    WebDav,
}

impl DestKind {
    pub fn label(self) -> &'static str {
        match self {
            DestKind::Folder => "Hard disk / cartella",
            DestKind::CloudFolder => "Cloud (cartella sincronizzata)",
            DestKind::WebDav => "Cloud (WebDAV)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Dest {
    pub id: String,
    pub name: String,
    pub kind: DestKind,
    /// Cartella (Folder, CloudFolder)
    pub path: String,
    /// Indirizzo e utente (WebDav); la password sta nel Gestore credenziali
    pub url: String,
    pub user: String,
    /// Copie cifrate con una password scelta dall'utente
    pub encrypt: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Retention {
    pub recent: usize,
    pub daily: usize,
    pub weekly: usize,
    pub monthly: usize,
    pub yearly: usize,
}

impl Default for Retention {
    fn default() -> Self {
        Retention { recent: 10, daily: 30, weekly: 12, monthly: 24, yearly: 10 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    pub interval_hours: u32,
    pub alert_days: u32,
    pub retention: Retention,
    pub dests: Vec<Dest>,
}

impl Default for Config {
    fn default() -> Self {
        Config { enabled: true, interval_hours: 2, alert_days: 3, retention: Retention::default(), dests: Vec::new() }
    }
}

/// Scheda salvata accanto a ogni copia.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Manifest {
    pub file: String,
    pub created: String,
    /// Dimensione e impronta del file come è scritto nella destinazione
    pub size: u64,
    pub sha256: String,
    /// Impronta dell'archivio in chiaro (uguale a `sha256` se non è cifrato)
    pub plain_sha256: String,
    pub encrypted: bool,
    pub counts: BTreeMap<String, i64>,
    pub app_version: String,
}

impl Manifest {
    pub fn created_at(&self) -> Option<NaiveDateTime> {
        NaiveDateTime::parse_from_str(&self.created, TIME_FMT).ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct DestState {
    pub last_ok: Option<String>,
    pub last_try: Option<String>,
    pub last_error: String,
    pub warning: String,
    pub last_file: String,
    pub last_plain_sha: String,
    pub copies: usize,
    /// Copie ricontrollate: nome e data dell'ultimo controllo
    pub scrubbed: BTreeMap<String, String>,
}

impl DestState {
    pub fn last_ok_at(&self) -> Option<NaiveDateTime> {
        self.last_ok.as_deref().and_then(|s| NaiveDateTime::parse_from_str(s, TIME_FMT).ok())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Saved,
    /// Dati identici all'ultima copia verificata
    Unchanged,
    /// Disco non collegato o cartella non riconosciuta
    Unreachable,
    Failed,
}

#[derive(Debug, Clone)]
pub struct JobDest {
    pub dest: Dest,
    pub password: Option<String>,
    pub secret: Option<String>,
    pub state: DestState,
}

#[derive(Debug, Clone)]
pub struct JobInput {
    pub source: PathBuf,
    pub stem: String,
    pub uid: String,
    pub spool: PathBuf,
    pub retention: Retention,
    pub dests: Vec<JobDest>,
    pub now: NaiveDateTime,
    pub app_version: String,
}

#[derive(Debug, Clone)]
pub struct DestOutcome {
    pub id: String,
    pub status: Status,
    pub message: String,
    pub state: DestState,
    /// Nuova posizione se il disco è comparso con un'altra lettera
    pub moved_to: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct JobResult {
    pub outcomes: Vec<DestOutcome>,
    /// Errore che ha impedito la copia (archivio illeggibile o danneggiato)
    pub fatal: Option<String>,
}

/// Testo di avanzamento letto dall'interfaccia.
pub type Progress = Arc<Mutex<String>>;

fn say(p: &Progress, t: impl Into<String>) {
    if let Ok(mut g) = p.lock() {
        *g = t.into();
    }
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("{}: {}", path.display(), e))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

/// Identificativo casuale per destinazioni e archivi.
pub fn new_id() -> String {
    let mut b = [0u8; 8];
    getrandom::getrandom(&mut b).expect("generatore casuale del sistema");
    hex(&b)
}

/// Sottocartella dell'archivio nella destinazione.
pub fn archive_folder(stem: &str, uid: &str) -> String {
    let clean: String = stem.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
    format!("{}-{}", clean.trim(), &uid[..uid.len().min(8)])
}

/// Trova la cartella di destinazione: si controlla il file di riconoscimento e, se serve,
/// si cerca lo stesso percorso sulle altre lettere di unità.
pub fn resolve_folder(dest: &Dest) -> Result<PathBuf, String> {
    let matches = |p: &Path| std::fs::read_to_string(p.join(MARKER)).map(|s| s.trim() == dest.id).unwrap_or(false);
    let p = PathBuf::from(&dest.path);
    if matches(&p) {
        return Ok(p);
    }
    let chars: Vec<char> = dest.path.chars().collect();
    if chars.len() >= 2 && chars[1] == ':' {
        let rest: String = chars[1..].iter().collect();
        for l in b'C'..=b'Z' {
            let c = PathBuf::from(format!("{}{}", l as char, rest));
            if matches(&c) {
                return Ok(c);
            }
        }
    }
    if p.exists() {
        Err(format!("{}: la cartella c'è ma non è quella scelta per le copie (disco diverso?)", dest.path))
    } else {
        Err(format!("{}: disco o cartella non disponibile", dest.path))
    }
}

/// La destinazione si può raggiungere adesso? (controllo rapido, senza rete)
pub fn reachable(dest: &Dest) -> bool {
    match dest.kind {
        DestKind::WebDav => true,
        _ => resolve_folder(dest).is_ok(),
    }
}

/// Prepara una destinazione nuova e prova davvero a scrivere, rileggere e cancellare.
pub fn prepare_and_test(dest: &Dest, secret: Option<&str>, spool: &Path) -> Result<(), String> {
    if dest.kind != DestKind::WebDav {
        std::fs::create_dir_all(&dest.path).map_err(|e| format!("{}: {}", dest.path, e))?;
        let marker = Path::new(&dest.path).join(MARKER);
        let mut f = File::create(&marker).map_err(|e| format!("Impossibile scrivere in {}: {}", dest.path, e))?;
        f.write_all(dest.id.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
    }
    let target = open_target(dest, secret, "_prova")?;
    target.ensure_dir()?;
    std::fs::create_dir_all(spool).map_err(|e| e.to_string())?;
    let local = spool.join(format!("prova-{}.bin", new_id()));
    let mut data = vec![0u8; 256 * 1024];
    getrandom::getrandom(&mut data).map_err(|e| e.to_string())?;
    std::fs::write(&local, &data).map_err(|e| e.to_string())?;
    let sha = sha256_file(&local)?;
    let res = target.put(&local, "prova.bin", &sha).and_then(|_| {
        let back = spool.join(format!("prova-{}.back", new_id()));
        target.get_to("prova.bin", &back)?;
        let got = sha256_file(&back);
        let _ = std::fs::remove_file(&back);
        if got? != sha {
            return Err("il file riletto è diverso da quello scritto".to_string());
        }
        target.delete("prova.bin")
    });
    let _ = std::fs::remove_file(&local);
    let _ = target.remove_self();
    res
}

/// Quali copie tenere: le più recenti e la più nuova di ogni giorno, settimana, mese, anno.
/// `dates` può essere in qualsiasi ordine; restituisce gli indici da conservare.
pub fn select_keep(dates: &[NaiveDateTime], r: &Retention) -> BTreeSet<usize> {
    let mut order: Vec<usize> = (0..dates.len()).collect();
    order.sort_by(|a, b| dates[*b].cmp(&dates[*a]));
    let mut keep: BTreeSet<usize> = order.iter().copied().take(r.recent.max(1)).collect();
    // anche la più vecchia: protegge da un problema che nessuno nota per molto tempo
    keep.extend(order.last().copied());
    fn take<K: Eq + std::hash::Hash>(order: &[usize], max: usize, key: impl Fn(usize) -> K, keep: &mut BTreeSet<usize>) {
        let mut seen = std::collections::HashSet::new();
        for i in order {
            if seen.len() >= max && !seen.contains(&key(*i)) {
                break;
            }
            if seen.insert(key(*i)) {
                keep.insert(*i);
            }
        }
    }
    take(&order, r.daily, |i| dates[i].date(), &mut keep);
    take(&order, r.weekly, |i| (dates[i].iso_week().year(), dates[i].iso_week().week()), &mut keep);
    take(&order, r.monthly, |i| (dates[i].year(), dates[i].month()), &mut keep);
    take(&order, r.yearly, |i| dates[i].year(), &mut keep);
    keep
}

/// Esegue una copia: istantanea verificata e consegna a tutte le destinazioni.
pub fn run_job(input: JobInput, progress: &Progress) -> JobResult {
    let mut res = JobResult::default();
    if let Err(e) = std::fs::create_dir_all(&input.spool) {
        res.fatal = Some(format!("{}: {}", input.spool.display(), e));
        return res;
    }
    let tag = new_id();
    let snap = input.spool.join(format!("istantanea-{}.gadb", tag));
    say(progress, "Preparazione della copia...");
    let counts = match openfatt_store::snapshot_archive(&input.source, &snap) {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&snap);
            res.fatal = Some(format!("La copia dell'archivio non ha superato il controllo: {}", e));
            return res;
        }
    };
    let plain_sha = match sha256_file(&snap) {
        Ok(h) => h,
        Err(e) => {
            let _ = std::fs::remove_file(&snap);
            res.fatal = Some(e);
            return res;
        }
    };
    let stamp = input.now.format("%Y-%m-%d %H-%M-%S").to_string();
    for jd in &input.dests {
        say(progress, format!("Copia su {}...", jd.dest.name));
        res.outcomes.push(deliver(&input, jd, &snap, &plain_sha, &counts, &stamp, &tag, progress));
    }
    let _ = std::fs::remove_file(&snap);
    say(progress, "Fatto");
    res
}

#[allow(clippy::too_many_arguments)]
fn deliver(input: &JobInput, jd: &JobDest, snap: &Path, plain_sha: &str, counts: &BTreeMap<String, i64>, stamp: &str, tag: &str, progress: &Progress) -> DestOutcome {
    let mut state = jd.state.clone();
    state.last_try = Some(input.now.format(TIME_FMT).to_string());
    let now = input.now;
    let mut dest = jd.dest.clone();
    let mut moved_to = None;
    if dest.kind != DestKind::WebDav {
        match resolve_folder(&dest) {
            Ok(p) => {
                let s = p.to_string_lossy().to_string();
                if s != dest.path {
                    moved_to = Some(s.clone());
                    dest.path = s;
                }
            }
            Err(e) => {
                state.last_error = e.clone();
                return DestOutcome { id: dest.id, status: Status::Unreachable, message: e, state, moved_to };
            }
        }
    }
    let outcome = (|| -> Result<(Status, String), String> {
        let folder = archive_folder(&input.stem, &input.uid);
        let target = open_target(&dest, jd.secret.as_deref(), &folder)?;
        target.ensure_dir()?;
        target.cleanup_partial();
        let manifests = read_manifests(target.as_ref());
        // nessuna novità rispetto all'ultima copia verificata
        if state.last_plain_sha == plain_sha && manifests.iter().any(|m| m.file == state.last_file) {
            state.copies = manifests.len();
            return Ok((Status::Unchanged, "Nessuna modifica dall'ultima copia".into()));
        }
        let enc_path = input.spool.join(format!("cifrata-{}-{}.enc", tag, &dest.id));
        let (local, name, encrypted): (PathBuf, String, bool) = if dest.encrypt {
            let pw = jd.password.as_deref().filter(|p| !p.is_empty()).ok_or("password di cifratura mancante")?;
            encrypt_file(snap, &enc_path, pw)?;
            (enc_path.clone(), format!("{} {}.gadb.enc", input.stem, stamp), true)
        } else {
            (snap.to_path_buf(), format!("{} {}.gadb", input.stem, stamp), false)
        };
        let finish = (|| -> Result<(Status, String), String> {
            let sha = sha256_file(&local)?;
            let size = std::fs::metadata(&local).map_err(|e| e.to_string())?.len();
            say(progress, format!("Copia su {}: scrittura e verifica...", dest.name));
            if let Err(first) = target.put(&local, &name, &sha) {
                // disco pieno: si applica la rotazione normale e si riprova una volta
                prune(target.as_ref(), &manifests, &input.retention);
                target.put(&local, &name, &sha).map_err(|e| if e == first { e } else { format!("{} ({})", e, first) })?;
            }
            let m = Manifest { file: name.clone(), created: now.format(TIME_FMT).to_string(), size, sha256: sha, plain_sha256: plain_sha.to_string(), encrypted, counts: counts.clone(), app_version: input.app_version.clone() };
            let json = serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?;
            target.put_small(&format!("{}.json", name), &json)?;
            state.warning.clear();
            // un calo forte dei dati è sospetto: si avvisa e non si elimina nessuna copia
            let previous = manifests.iter().max_by_key(|m| m.created_at());
            let before = previous.and_then(|p| p.counts.get("documents")).copied().unwrap_or(0);
            let after = counts.get("documents").copied().unwrap_or(0);
            let mut all = manifests.clone();
            all.push(m);
            if before > 20 && after * 2 < before {
                state.warning = format!("I documenti sono scesi da {} a {}: le copie precedenti sono state tenute tutte", before, after);
            } else {
                prune(target.as_ref(), &all, &input.retention);
                all = read_manifests(target.as_ref());
            }
            scrub(target.as_ref(), &dest, &mut state, &all, &name, now, &input.spool);
            state.copies = all.len();
            state.last_file = name.clone();
            state.last_plain_sha = plain_sha.to_string();
            Ok((Status::Saved, format!("Copia verificata: {}", name)))
        })();
        let _ = std::fs::remove_file(&enc_path);
        finish
    })();
    match outcome {
        Ok((status, message)) => {
            state.last_ok = Some(now.format(TIME_FMT).to_string());
            state.last_error.clear();
            DestOutcome { id: dest.id, status, message, state, moved_to }
        }
        Err(e) => {
            state.last_error = e.clone();
            DestOutcome { id: dest.id, status: Status::Failed, message: e, state, moved_to }
        }
    }
}

fn read_manifests(t: &dyn Target) -> Vec<Manifest> {
    let mut out = Vec::new();
    for n in t.list().unwrap_or_default() {
        let Some(base) = n.strip_suffix(".json") else { continue };
        if let Some(m) = t.get_small(&n).ok().and_then(|b| serde_json::from_slice::<Manifest>(&b).ok()) {
            if m.file == base {
                out.push(m);
            }
        }
    }
    out.sort_by(|a, b| b.created.cmp(&a.created));
    out
}

fn prune(t: &dyn Target, list: &[Manifest], r: &Retention) {
    let dated: Vec<(&Manifest, NaiveDateTime)> = list.iter().filter_map(|m| m.created_at().map(|d| (m, d))).collect();
    let dates: Vec<NaiveDateTime> = dated.iter().map(|x| x.1).collect();
    let keep = select_keep(&dates, r);
    for (i, (m, _)) in dated.iter().enumerate() {
        if !keep.contains(&i) && t.delete(&m.file).is_ok() {
            let _ = t.delete(&format!("{}.json", m.file));
        }
    }
}

/// Ricontrolla una copia già fatta, per scoprire in tempo un disco che si sta guastando.
fn scrub(t: &dyn Target, dest: &Dest, st: &mut DestState, list: &[Manifest], newest: &str, now: NaiveDateTime, spool: &Path) {
    st.scrubbed.retain(|k, _| list.iter().any(|m| &m.file == k));
    // nel cloud rileggere vuol dire riscaricare: basta la verifica fatta alla scrittura
    if dest.kind != DestKind::Folder {
        return;
    }
    let due = list.iter().filter(|m| m.file != newest).filter(|m| st.scrubbed.get(&m.file).and_then(|d| NaiveDateTime::parse_from_str(d, TIME_FMT).ok()).map(|d| (now - d).num_days() >= 7).unwrap_or(true)).min_by_key(|m| st.scrubbed.get(&m.file).cloned().unwrap_or_default());
    let Some(m) = due else { return };
    let back = spool.join(format!("controllo-{}.bin", new_id()));
    let ok = t.get_to(&m.file, &back).and_then(|_| sha256_file(&back)).map(|h| h == m.sha256).unwrap_or(false);
    let _ = std::fs::remove_file(&back);
    if ok {
        st.scrubbed.insert(m.file.clone(), now.format(TIME_FMT).to_string());
    } else {
        // fuori dalla rotazione, così non conta come copia buona
        let _ = t.delete(&format!("{}.json", m.file));
        st.warning = format!("La copia {} risulta danneggiata: il disco potrebbe avere problemi", m.file);
    }
}

/// Copie presenti in una destinazione, dalla più recente.
pub fn list_backups(dest: &Dest, secret: Option<&str>, stem: &str, uid: &str) -> Result<Vec<Manifest>, String> {
    let mut d = dest.clone();
    if d.kind != DestKind::WebDav {
        d.path = resolve_folder(&d)?.to_string_lossy().to_string();
    }
    let t = open_target(&d, secret, &archive_folder(stem, uid))?;
    t.list()?;
    Ok(read_manifests(t.as_ref()))
}

/// Ripristina una copia in un file nuovo: impronta, decifratura e integrità sono verificate,
/// e l'archivio attuale non viene toccato.
#[allow(clippy::too_many_arguments)]
pub fn restore(dest: &Dest, secret: Option<&str>, password: Option<&str>, stem: &str, uid: &str, m: &Manifest, out: &Path, spool: &Path) -> Result<(), String> {
    if out.exists() {
        return Err(format!("{} esiste già", out.display()));
    }
    let mut d = dest.clone();
    if d.kind != DestKind::WebDav {
        d.path = resolve_folder(&d)?.to_string_lossy().to_string();
    }
    let t = open_target(&d, secret, &archive_folder(stem, uid))?;
    std::fs::create_dir_all(spool).map_err(|e| e.to_string())?;
    let got = spool.join(format!("ripristino-{}.bin", new_id()));
    let res = t.get_to(&m.file, &got).and_then(|_| {
        if sha256_file(&got)? != m.sha256 {
            return Err("La copia è danneggiata: l'impronta non corrisponde".to_string());
        }
        finish_restore(&got, m.encrypted, password, Some(&m.plain_sha256), out)
    });
    let _ = std::fs::remove_file(&got);
    res
}

/// Ripristina da un file scelto a mano (per esempio su un computer nuovo).
pub fn restore_file(file: &Path, password: Option<&str>, out: &Path) -> Result<(), String> {
    if out.exists() {
        return Err(format!("{} esiste già", out.display()));
    }
    let encrypted = file.extension().map(|e| e == "enc").unwrap_or(false);
    let sidecar = PathBuf::from(format!("{}.json", file.display()));
    let m: Option<Manifest> = std::fs::read(&sidecar).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(m) = &m {
        if sha256_file(file)? != m.sha256 {
            return Err("La copia è danneggiata: l'impronta non corrisponde".into());
        }
    }
    finish_restore(file, encrypted, password, m.as_ref().map(|m| m.plain_sha256.as_str()), out)
}

fn finish_restore(file: &Path, encrypted: bool, password: Option<&str>, plain_sha: Option<&str>, out: &Path) -> Result<(), String> {
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let part = PathBuf::from(format!("{}.part", out.display()));
    let done = (|| -> Result<(), String> {
        if encrypted {
            decrypt_file(file, &part, password.unwrap_or(""))?;
        } else {
            std::fs::copy(file, &part).map_err(|e| e.to_string())?;
        }
        if let Some(h) = plain_sha {
            if sha256_file(&part)? != h {
                return Err("La copia è danneggiata: l'impronta non corrisponde".into());
            }
        }
        openfatt_store::check_archive(&part).map_err(|e| format!("Controllo dell'archivio non riuscito: {}", e))?;
        for s in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", part.display(), s));
        }
        std::fs::OpenOptions::new().write(true).open(&part).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
        std::fs::rename(&part, out).map_err(|e| e.to_string())
    })();
    if done.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    done
}

#[cfg(test)]
mod tests;
