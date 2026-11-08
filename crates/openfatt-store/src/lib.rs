//! Archivio SQLite del gestionale: un file per azienda.
//!
//! Principi:
//! - ogni scrittura che tocca più tabelle avviene in una transazione;
//! - importi e quantità sono salvati come testo decimale esatto, mai in virgola mobile;
//! - le righe modificabili hanno un numero di versione: un salvataggio basato su una
//!   versione superata viene rifiutato (nessuna sovrascrittura silenziosa);
//! - le operazioni rilevanti sono registrate nel registro `audit`.

mod agenda;
mod annual;
mod agents;
mod archive;
mod import_easyfatt;
mod imports;
mod stats;
mod users;
mod yearend;
mod documents;
mod einvoices;
mod extras;
mod inventory;
mod bank;
mod ledger;
mod mail;
mod migrations;
mod parties;
mod pricing;
mod products;
mod retail;
mod tracking;
mod vat;
mod warehouses;

use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::{params, Connection, OptionalExtension};
use rust_decimal::Decimal;
use serde::{de::DeserializeOwned, Serialize};

pub use documents::{DocSummary, DocFilter, DdtSource, OrderProgress};
pub use extras::{add_months, copy_line, intent_amount, period_text, Attachment, IntentUsage, LastPrice, ProductImport, Recurrence};
pub use annual::{region_of, AnnualVat, Quadro, Rigo, REGIONS};
pub use agenda::{AgendaItem, AgendaKind, Card, CardKind, CardMove, LoyaltySettings};
pub use archive::{ConservationPackage, ConservationRecord};
pub use import_easyfatt::EasyfattImport;
pub use openfatt_fbread::Dump as EasyfattDump;
pub use imports::{MetelImport, ShopImport};
pub use stats::{month_label, FlowRow, StatBy, StatRow};
pub use users::{hash_password, verify_password, Access, Area, Session, User, SESSION_TIMEOUT_SECS};
pub use yearend::{YearCheck, YearClosure};
pub use agents::{Agent, CommissionRow, Settlement};
pub use einvoices::{EInvoice, EInvoiceEvent, EInvoiceStatus, Direction};
pub use bank::{BankBatch, BankKind, BankSettings, BatchItem, BatchStatus};
pub use ledger::{DueRow, AccountBalance};
pub use mail::{MailKind, MailLogEntry, MailStatus, ReminderCandidate};
pub use inventory::{ga_core_count::CountRow, CountImport, InventoryApplied, InventoryLine, InventorySession, InventoryStatus, InventoryTotals};
pub use parties::PartyFilter;
pub use products::{StockInfo, StockRow, StockState};
pub use retail::RetailDay;
pub use tracking::{LotRow, TraceRow, LOT_UNKNOWN};
pub use warehouses::DEFAULT_WAREHOUSE;
pub use vat::VatSettings;

pub use openfatt_core::model;

#[derive(Debug)]
pub enum StoreError {
    Db(rusqlite::Error),
    Json(serde_json::Error),
    /// Versione superata: un altro salvataggio è avvenuto nel frattempo.
    Conflict(String),
    /// Operazione non ammessa o dati non validi.
    Invalid(String),
    NotFound(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "Errore archivio: {}", e),
            StoreError::Json(e) => write!(f, "Errore dati: {}", e),
            StoreError::Conflict(s) => write!(f, "Conflitto: {}", s),
            StoreError::Invalid(s) => write!(f, "{}", s),
            StoreError::NotFound(s) => write!(f, "Non trovato: {}", s),
        }
    }
}
impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Db(e)
    }
}
impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Json(e)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct Store {
    conn: Connection,
    path: PathBuf,
    savepoints: std::cell::Cell<u64>,
    /// Utente collegato (vuoto senza gestione utenti)
    user: std::cell::RefCell<String>,
    /// Postazione registrata nella tabella sessions (0 = nessuna)
    session: std::cell::Cell<i64>,
    network: bool,
}

/// Un archivio su una cartella di rete (\\server\cartella o unità di rete) non può usare
/// il registro WAL di SQLite, che richiede memoria condivisa sullo stesso computer.
pub fn is_network_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    if s.starts_with("\\\\") || s.starts_with("//") {
        return !s.starts_with("\\\\?\\") || s.to_uppercase().starts_with("\\\\?\\UNC\\");
    }
    #[cfg(windows)]
    {
        if let Some(c) = s.chars().next() {
            if s.chars().nth(1) == Some(':') && c.is_ascii_alphabetic() {
                return windows_drive_is_remote(c);
            }
        }
    }
    false
}

#[cfg(windows)]
fn windows_drive_is_remote(letter: char) -> bool {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    const DRIVE_REMOTE: u32 = 4;
    let root: Vec<u16> = format!("{}:\\", letter).encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
}

/// Transazione annidabile basata su SAVEPOINT: se non confermata viene annullata.
pub(crate) struct Tx<'a> {
    conn: &'a Connection,
    name: String,
    done: bool,
}

impl<'a> Tx<'a> {
    pub(crate) fn commit(mut self) -> Result<()> {
        self.conn.execute_batch(&format!("RELEASE {}", self.name))?;
        self.done = true;
        Ok(())
    }
}

impl<'a> Drop for Tx<'a> {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.conn.execute_batch(&format!("ROLLBACK TO {0}; RELEASE {0}", self.name));
        }
    }
}

pub(crate) fn dec_to_sql(d: Decimal) -> String {
    d.normalize().to_string()
}

pub(crate) fn dec_from_sql(s: Option<String>) -> Decimal {
    s.and_then(|v| Decimal::from_str(&v).ok()).unwrap_or_default()
}

pub(crate) fn date_to_sql(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub(crate) fn date_from_sql(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").unwrap_or_else(|_| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
}

pub(crate) fn now() -> NaiveDateTime {
    chrono::Local::now().naive_local()
}

pub(crate) fn now_sql() -> String {
    now().format("%Y-%m-%d %H:%M:%S").to_string()
}

impl Store {
    /// Apre (o crea) l'archivio e applica le migrazioni.
    pub fn open(path: impl AsRef<Path>) -> Result<Store> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        Self::init(conn, path)
    }

    /// Archivio in memoria, per i test.
    pub fn open_in_memory() -> Result<Store> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn, PathBuf::from(":memory:"))
    }

    fn init(conn: Connection, path: PathBuf) -> Result<Store> {
        let network = is_network_path(&path) || std::env::var_os("OPENFATT_NETWORK").is_some();
        conn.pragma_update(None, "foreign_keys", "ON")?;
        if network {
            // in rete: registro classico, scritture sincronizzate, attese più lunghe
            conn.pragma_update(None, "journal_mode", "DELETE")?;
            conn.pragma_update(None, "synchronous", "FULL")?;
            conn.busy_timeout(std::time::Duration::from_secs(30))?;
        } else {
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "synchronous", "NORMAL")?;
            conn.busy_timeout(std::time::Duration::from_secs(10))?;
        }
        conn.pragma_update(None, "cache_size", -32000)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        let mut s = Store { conn, path, savepoints: std::cell::Cell::new(0), user: Default::default(), session: Default::default(), network };
        migrations::migrate(&mut s.conn)?;
        s.seed_defaults()?;
        s.seed_warehouses()?;
        Ok(s)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Archivio aperto da una cartella di rete.
    pub fn is_network(&self) -> bool {
        self.network
    }

    /// Contatore che cambia a ogni modifica dell'archivio, anche da altre postazioni:
    /// le schermate lo usano per sapere quando rileggere gli elenchi.
    pub fn generation(&self) -> u64 {
        let other: i64 = self.conn.query_row("PRAGMA data_version", [], |r| r.get(0)).unwrap_or(0);
        (self.conn.total_changes() as u64).wrapping_mul(1_000_003).wrapping_add(other as u64)
    }

    /// Dimensione del file dell'archivio in byte.
    pub fn file_size(&self) -> u64 {
        std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Esegue più operazioni in un'unica transazione (importazioni massive): tutto o niente.
    pub fn with_transaction<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let tx = self.begin()?;
        let r = f()?;
        tx.commit()?;
        Ok(r)
    }

    pub(crate) fn begin(&self) -> Result<Tx<'_>> {
        let n = self.savepoints.get() + 1;
        self.savepoints.set(n);
        let name = format!("sp_{}", n);
        self.conn.execute_batch(&format!("SAVEPOINT {}", name))?;
        Ok(Tx { conn: &self.conn, name, done: false })
    }

    // ------------------------------------------------------------------
    // Impostazioni
    // ------------------------------------------------------------------

    pub fn get_setting<T: DeserializeOwned + Default>(&self, key: &str) -> Result<T> {
        let v: Option<String> = self.conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?;
        match v {
            Some(s) => Ok(serde_json::from_str(&s).unwrap_or_default()),
            None => Ok(T::default()),
        }
    }

    pub fn set_setting<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let s = serde_json::to_string(value)?;
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, s],
        )?;
        self.audit("settings", 0, "update", key)?;
        Ok(())
    }

    pub fn company(&self) -> Result<model::Company> {
        self.get_setting("company")
    }

    pub fn set_company(&self, c: &model::Company) -> Result<()> {
        self.set_setting("company", c)
    }

    /// Logo dell'azienda (immagine JPEG o PNG) salvato nell'archivio.
    pub fn logo(&self) -> Result<Option<Vec<u8>>> {
        use base64::Engine;
        let v: String = self.get_setting("logo")?;
        if v.is_empty() {
            return Ok(None);
        }
        Ok(base64::engine::general_purpose::STANDARD.decode(v).ok())
    }

    pub fn set_logo(&self, data: Option<&[u8]>) -> Result<()> {
        use base64::Engine;
        if let Some(d) = data {
            if d.len() > 2 * 1024 * 1024 {
                return Err(StoreError::Invalid("Il logo supera i 2 MB: usare un'immagine più leggera".into()));
            }
        }
        let v = data.map(|d| base64::engine::general_purpose::STANDARD.encode(d)).unwrap_or_default();
        self.set_setting("logo", &v)
    }

    // ------------------------------------------------------------------
    // Contatori e registro
    // ------------------------------------------------------------------

    /// Incrementa e restituisce un contatore (es. progressivo invio SdI).
    pub fn next_counter(&self, name: &str) -> Result<u64> {
        self.conn.execute(
            "INSERT INTO counters(name, value) VALUES (?1, 1) ON CONFLICT(name) DO UPDATE SET value = value + 1",
            [name],
        )?;
        let v: i64 = self.conn.query_row("SELECT value FROM counters WHERE name = ?1", [name], |r| r.get(0))?;
        Ok(v as u64)
    }

    pub fn peek_counter(&self, name: &str) -> Result<u64> {
        let v: Option<i64> = self.conn.query_row("SELECT value FROM counters WHERE name = ?1", [name], |r| r.get(0)).optional()?;
        Ok(v.unwrap_or(0) as u64)
    }

    pub fn set_counter(&self, name: &str, value: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO counters(name, value) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET value = excluded.value",
            params![name, value as i64],
        )?;
        Ok(())
    }

    pub(crate) fn audit(&self, entity: &str, id: i64, action: &str, details: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO audit(ts, entity, entity_id, action, details, user) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![now_sql(), entity, id, action, details, self.user.borrow().as_str()],
        )?;
        Ok(())
    }

    /// Ultime operazioni registrate (più recenti prima).
    pub fn audit_log(&self, limit: usize) -> Result<Vec<(String, String, i64, String, String)>> {
        let mut st = self.conn.prepare("SELECT ts, entity, entity_id, action, CASE WHEN user <> '' THEN details || ' [' || user || ']' ELSE details END FROM audit ORDER BY id DESC LIMIT ?1")?;
        let rows = st.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // ------------------------------------------------------------------
    // Backup
    // ------------------------------------------------------------------

    /// Copia coerente dell'archivio (anche mentre è in uso) in un nuovo file.
    pub fn backup_to(&self, dest: impl AsRef<Path>) -> Result<()> {
        let dest = dest.as_ref();
        if dest.exists() {
            std::fs::remove_file(dest).map_err(|e| StoreError::Invalid(format!("Impossibile sovrascrivere {}: {}", dest.display(), e)))?;
        }
        self.conn.execute("VACUUM INTO ?1", [dest.to_string_lossy().to_string()])?;
        self.audit("archive", 0, "backup", &dest.to_string_lossy())?;
        Ok(())
    }

    /// Controllo di integrità del file.
    pub fn integrity_check(&self) -> Result<String> {
        let s: String = self.conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        Ok(s)
    }

    // ------------------------------------------------------------------
    // Tabelle: aliquote IVA e pagamenti
    // ------------------------------------------------------------------

    fn seed_defaults(&mut self) -> Result<()> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM vat_codes", [], |r| r.get(0))?;
        if n == 0 {
            let d = |code: &str, desc: &str, rate: i64, natura: Option<&str>, rif: &str| model::VatCode {
                id: 0,
                code: code.into(),
                description: desc.into(),
                rate: Decimal::from(rate),
                natura: natura.map(|s| s.to_string()),
                legal_ref: rif.into(),
                active: true,
                split_payment: false,
            };
            let list = vec![
                d("22", "IVA 22%", 22, None, ""),
                d("10", "IVA 10%", 10, None, ""),
                d("5", "IVA 5%", 5, None, ""),
                d("4", "IVA 4%", 4, None, ""),
                d("N1", "Escluso art. 15", 0, Some("N1"), "Escluso art. 15 DPR 633/72"),
                d("N2.1", "Non soggetto artt. 7-7septies", 0, Some("N2.1"), "Non soggetto art. 7-ter DPR 633/72"),
                d("N2.2", "Non soggetto - forfettario/altri casi", 0, Some("N2.2"), "Operazione senza applicazione dell'IVA ai sensi dell'art. 1 c. 54-89 L. 190/2014"),
                d("N3.1", "Non imponibile esportazione", 0, Some("N3.1"), "Non imponibile art. 8 c. 1 DPR 633/72"),
                d("N3.2", "Non imponibile cessione intra UE", 0, Some("N3.2"), "Non imponibile art. 41 DL 331/93"),
                d("N3.5", "Non imponibile dichiarazione d'intento", 0, Some("N3.5"), "Non imponibile art. 8 c. 1 lett. c DPR 633/72"),
                d("N4", "Esente art. 10", 0, Some("N4"), "Esente art. 10 DPR 633/72"),
                d("N5", "Regime del margine", 0, Some("N5"), "Regime del margine art. 36 DL 41/95"),
                d("N6.3", "Reverse charge subappalto edile", 0, Some("N6.3"), "Inversione contabile art. 17 c. 6 lett. a DPR 633/72"),
                d("N6.7", "Reverse charge comparto edile", 0, Some("N6.7"), "Inversione contabile art. 17 c. 6 lett. a-ter DPR 633/72"),
                d("N6.9", "Reverse charge altri casi", 0, Some("N6.9"), "Inversione contabile art. 17 DPR 633/72"),
                d("N7", "IVA assolta in altro Stato UE", 0, Some("N7"), "IVA assolta in altro Stato UE"),
            ];
            let split = |code: &str, rate: i64| model::VatCode {
                id: 0,
                code: code.into(),
                description: format!("Imponibile {}% con scissione dei pagamenti", rate),
                rate: Decimal::from(rate),
                natura: None,
                legal_ref: "Scissione dei pagamenti art. 17-ter DPR 633/72".into(),
                active: true,
                split_payment: true,
            };
            let list: Vec<model::VatCode> = list.into_iter().chain([split("22sp", 22), split("10sp", 10), split("5sp", 5), split("4sp", 4)]).collect();
            for v in list {
                self.save_vat_code(&v)?;
            }
        }
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM payment_methods", [], |r| r.get(0))?;
        if n == 0 {
            use model::{Installment, PaymentMethod};
            let one = |days: u32, eom: bool| Installment { days, end_of_month: eom, extra_days: 0, percent: Decimal::ONE_HUNDRED };
            let list = vec![
                PaymentMethod { id: 0, name: "Rimessa diretta".into(), mp_code: "MP01".into(), installments: vec![one(0, false)], show_iban: false, active: true },
                PaymentMethod { id: 0, name: "Bonifico vista fattura".into(), mp_code: "MP05".into(), installments: vec![one(0, false)], show_iban: true, active: true },
                PaymentMethod { id: 0, name: "Bonifico 30 gg d.f.".into(), mp_code: "MP05".into(), installments: vec![one(30, false)], show_iban: true, active: true },
                PaymentMethod { id: 0, name: "Bonifico 30 gg d.f. f.m.".into(), mp_code: "MP05".into(), installments: vec![one(30, true)], show_iban: true, active: true },
                PaymentMethod {
                    id: 0,
                    name: "RiBa 30-60 gg d.f. f.m.".into(),
                    mp_code: "MP12".into(),
                    installments: vec![
                        Installment { days: 30, end_of_month: true, extra_days: 0, percent: Decimal::from(50) },
                        Installment { days: 60, end_of_month: true, extra_days: 0, percent: Decimal::from(50) },
                    ],
                    show_iban: false,
                    active: true,
                },
                PaymentMethod { id: 0, name: "SDD 30 gg d.f. f.m.".into(), mp_code: "MP19".into(), installments: vec![one(30, true)], show_iban: false, active: true },
                PaymentMethod { id: 0, name: "Carta di pagamento".into(), mp_code: "MP08".into(), installments: vec![one(0, false)], show_iban: false, active: true },
                PaymentMethod { id: 0, name: "Contanti".into(), mp_code: "MP01".into(), installments: vec![one(0, false)], show_iban: false, active: true },
            ];
            for p in list {
                self.save_payment_method(&p)?;
            }
        }
        Ok(())
    }

    pub fn vat_codes(&self) -> Result<Vec<model::VatCode>> {
        let mut st = self.conn.prepare("SELECT id, data FROM vat_codes ORDER BY id")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, data) = row?;
            let mut v: model::VatCode = serde_json::from_str(&data)?;
            v.id = id;
            out.push(v);
        }
        Ok(out)
    }

    pub fn vat_code(&self, id: i64) -> Result<Option<model::VatCode>> {
        Ok(self.vat_codes()?.into_iter().find(|v| v.id == id))
    }

    pub fn save_vat_code(&self, v: &model::VatCode) -> Result<i64> {
        if v.code.trim().is_empty() {
            return Err(StoreError::Invalid("Codice aliquota obbligatorio".into()));
        }
        if v.rate.is_zero() && v.natura.as_deref().unwrap_or("").is_empty() {
            return Err(StoreError::Invalid("Un'aliquota zero richiede la natura dell'operazione".into()));
        }
        if !v.rate.is_zero() && v.natura.as_deref().map(|n| !n.is_empty()).unwrap_or(false) {
            return Err(StoreError::Invalid("La natura si indica solo con aliquota zero".into()));
        }
        let data = serde_json::to_string(v)?;
        if v.id == 0 {
            self.conn.execute("INSERT INTO vat_codes(code, data) VALUES (?1, ?2)", params![v.code.trim(), data])?;
            Ok(self.conn.last_insert_rowid())
        } else {
            self.conn.execute("UPDATE vat_codes SET code = ?1, data = ?2 WHERE id = ?3", params![v.code.trim(), data, v.id])?;
            Ok(v.id)
        }
    }

    pub fn delete_vat_code(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM vat_codes WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn payment_methods(&self) -> Result<Vec<model::PaymentMethod>> {
        let mut st = self.conn.prepare("SELECT id, data FROM payment_methods ORDER BY name")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, data) = row?;
            let mut p: model::PaymentMethod = serde_json::from_str(&data)?;
            p.id = id;
            out.push(p);
        }
        Ok(out)
    }

    pub fn payment_method(&self, id: i64) -> Result<Option<model::PaymentMethod>> {
        Ok(self.payment_methods()?.into_iter().find(|p| p.id == id))
    }

    pub fn save_payment_method(&self, p: &model::PaymentMethod) -> Result<i64> {
        if p.name.trim().is_empty() {
            return Err(StoreError::Invalid("Nome del pagamento obbligatorio".into()));
        }
        let total: Decimal = p.installments.iter().map(|i| i.percent).sum();
        if !p.installments.is_empty() && total != Decimal::ONE_HUNDRED {
            return Err(StoreError::Invalid(format!("Le percentuali delle rate devono sommare a 100 (ora {})", total)));
        }
        let data = serde_json::to_string(p)?;
        if p.id == 0 {
            self.conn.execute("INSERT INTO payment_methods(name, data) VALUES (?1, ?2)", params![p.name.trim(), data])?;
            Ok(self.conn.last_insert_rowid())
        } else {
            self.conn.execute("UPDATE payment_methods SET name = ?1, data = ?2 WHERE id = ?3", params![p.name.trim(), data, p.id])?;
            Ok(p.id)
        }
    }

    pub fn delete_payment_method(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM payment_methods WHERE id = ?1", [id])?;
        Ok(())
    }
}

/// Tabelle contate nelle copie di sicurezza.
const SNAPSHOT_TABLES: [&str; 5] = ["documents", "parties", "products", "due_dates", "attachments"];

/// Controlla che un file sia un archivio integro e ne restituisce i conteggi principali.
pub fn check_archive(path: &Path) -> Result<std::collections::BTreeMap<String, i64>> {
    let c = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI)?;
    let check: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if check != "ok" {
        return Err(StoreError::Invalid(format!("integrità: {}", check)));
    }
    let mut out = std::collections::BTreeMap::new();
    for t in SNAPSHOT_TABLES {
        let n: i64 = c.query_row(&format!("SELECT COUNT(*) FROM {}", t), [], |r| r.get(0))?;
        out.insert(t.to_string(), n);
    }
    Ok(out)
}

/// Istantanea coerente di un archivio, anche in uso: con una connessione propria, quindi
/// si può fare da un'altra attività. La copia viene controllata prima di restituirla.
pub fn snapshot_archive(source: &Path, dest: &Path) -> Result<std::collections::BTreeMap<String, i64>> {
    if dest.exists() {
        std::fs::remove_file(dest).map_err(|e| StoreError::Invalid(e.to_string()))?;
    }
    let c = Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_URI)?;
    c.busy_timeout(std::time::Duration::from_secs(60))?;
    c.execute("VACUUM INTO ?1", [dest.to_string_lossy().to_string()])?;
    drop(c);
    let counts = check_archive(dest);
    if counts.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    counts
}

#[cfg(test)]
mod tests;
