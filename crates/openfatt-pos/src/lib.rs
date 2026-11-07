//! Collegamento con i registratori telematici (RT) per l'emissione del documento commerciale.
//!
//! Il documento commerciale e la trasmissione dei corrispettivi all'Agenzia delle Entrate sono
//! compiti del registratore telematico omologato: il gestionale prepara la vendita e la invia
//! al dispositivo. Driver disponibili:
//!
//! - **Adattatore esterno (formato INI)**: file di interscambio pubblico già usato dai produttori
//!   di registratori per collegarsi ai gestionali; il percorso del file è passato come argomento
//!   all'eseguibile dell'adattatore.
//! - **Epson (XML via rete)**: protocollo XML su HTTP dei registratori Epson con interfaccia
//!   "Fiscal ePOS-Print" (cgi-bin/fpmate.cgi). Da collaudare sul modello in uso.
//! - **A modelli**: comandi di testo con segnaposto inviati con un file o una richiesta HTTP,
//!   per i programmi di collegamento di altri produttori (vedi `template`).
//! - **Simulatore**: scrive i file in una cartella e restituisce numeri fittizi; nessun valore fiscale.
//! - **Manuale**: la vendita è battuta a mano sul registratore e l'operatore ne riporta gli estremi.

pub mod epson;
pub mod ini;
pub mod template;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Riga del documento commerciale, già con importi IVA inclusa.
#[derive(Debug, Clone, PartialEq)]
pub struct ReceiptLine {
    pub description: String,
    pub qty: Decimal,
    /// Prezzo unitario IVA inclusa, prima dello sconto
    pub unit_price: Decimal,
    /// Importo totale dello sconto sulla riga (positivo)
    pub discount: Decimal,
    pub discount_label: String,
    pub vat_rate: Decimal,
    pub natura: Option<String>,
    pub department: u32,
    pub refund: bool,
}

impl ReceiptLine {
    pub fn total(&self) -> Decimal {
        let t = openfatt_core::money::round2(self.unit_price * self.qty) - self.discount;
        if self.refund {
            -t
        } else {
            t
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayKind {
    Cash,
    Electronic,
    Ticket,
    NotCollected,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReceiptPayment {
    pub kind: PayKind,
    pub description: String,
    pub amount: Decimal,
}

/// Dati di una vendita da emettere sul registratore.
#[derive(Debug, Clone, PartialEq)]
pub struct Receipt {
    pub lines: Vec<ReceiptLine>,
    pub payments: Vec<ReceiptPayment>,
    pub total: Decimal,
    pub change: Decimal,
    pub customer_tax_code: String,
    pub lottery_code: String,
    pub courtesy_message: String,
    /// Estremi del documento originale, per i resi: (matricola, n. chiusura, n. documento, data gg-mm-aaaa)
    pub refund_of: Option<(String, String, String, String)>,
    pub operator: u32,
}

/// Esito restituito dal registratore.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RtOutcome {
    pub receipt_number: String,
    pub z_number: String,
    pub serial: String,
    pub simulated: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtError(pub String);

impl std::fmt::Display for RtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for RtError {}

pub type Result<T> = std::result::Result<T, RtError>;

/// Corrispondenza aliquota/natura -> reparto del registratore.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DepartmentMap {
    /// Aliquota (es. "22") o natura (es. "N4") e reparto corrispondente.
    pub key: String,
    pub department: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum Driver {
    /// Nessun collegamento: estremi inseriti a mano
    #[default]
    Manual,
    /// Cartella in cui scrivere i file di prova
    Simulator { folder: String },
    /// Eseguibile dell'adattatore che riceve il percorso del file INI
    IniAdapter { exe: String, port: String, ip: String },
    /// Registratore Epson in rete
    EpsonXml { host: String, port: u16, timeout_ms: u32 },
    /// Comandi a modelli via file o HTTP
    Template { transport: template::Transport, templates: template::Templates },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RtConfig {
    pub driver: Driver,
    pub departments: Vec<DepartmentMap>,
    pub default_department: u32,
    pub operator: u32,
    pub courtesy_message: String,
    /// Conto di prima nota per i contanti e per i pagamenti elettronici
    pub cash_account: String,
    pub electronic_account: String,
    /// Magazzino da cui scaricano le vendite al banco (vuoto = predefinito)
    pub warehouse: String,
}

impl Default for RtConfig {
    fn default() -> Self {
        RtConfig {
            driver: Driver::Manual,
            departments: vec![
                DepartmentMap { key: "22".into(), department: 1 },
                DepartmentMap { key: "10".into(), department: 2 },
                DepartmentMap { key: "4".into(), department: 3 },
                DepartmentMap { key: "5".into(), department: 4 },
            ],
            default_department: 1,
            operator: 1,
            courtesy_message: "Grazie e arrivederci".into(),
            cash_account: "Cassa".into(),
            electronic_account: "Banca".into(),
            warehouse: String::new(),
        }
    }
}

impl RtConfig {
    /// Reparto per un'aliquota o natura.
    pub fn department_for(&self, rate: Decimal, natura: Option<&str>) -> u32 {
        let key = match natura {
            Some(n) if rate.is_zero() => n.to_string(),
            _ => rate.normalize().to_string(),
        };
        self.departments.iter().find(|d| d.key.trim() == key).map(|d| d.department).unwrap_or(self.default_department)
    }

    pub fn driver_label(&self) -> &'static str {
        match self.driver {
            Driver::Manual => "Manuale (estremi inseriti dall'operatore)",
            Driver::Simulator { .. } => "Simulatore (nessun valore fiscale)",
            Driver::IniAdapter { .. } => "Adattatore esterno (file INI)",
            Driver::EpsonXml { .. } => "Epson in rete (XML)",
            Driver::Template { .. } => "Comandi a modelli (file o HTTP)",
        }
    }
}

/// Emette il documento commerciale con il driver configurato.
pub fn emit(cfg: &RtConfig, r: &Receipt) -> Result<RtOutcome> {
    if r.lines.is_empty() {
        return Err(RtError("Nessuna riga da emettere".into()));
    }
    match &cfg.driver {
        Driver::Manual => Err(RtError("Driver manuale: battere la vendita sul registratore e inserire gli estremi".into())),
        Driver::Simulator { folder } => simulate(folder, r),
        Driver::IniAdapter { exe, port, ip } => ini::run_adapter(exe, port, ip, r),
        Driver::EpsonXml { host, port, timeout_ms } => {
            let xml = epson::receipt_xml(r);
            let resp = epson::post(host, *port, *timeout_ms, &xml)?;
            epson::parse_response(&resp)
        }
        Driver::Template { transport, templates } => template::emit(transport, templates, r),
    }
}

/// Chiusura giornaliera (rapporto Z): invio dei corrispettivi da parte del registratore.
pub fn daily_closure(cfg: &RtConfig) -> Result<String> {
    match &cfg.driver {
        Driver::EpsonXml { host, port, timeout_ms } => {
            let resp = epson::post(host, *port, *timeout_ms, &epson::z_report_xml(cfg.operator))?;
            epson::parse_response(&resp).map(|o| format!("Chiusura giornaliera eseguita. {}", o.message))
        }
        Driver::Template { transport, templates } => template::closure(transport, templates),
        Driver::Simulator { folder } => {
            let p = std::path::Path::new(folder).join(format!("chiusura_{}.txt", chrono::Local::now().format("%Y%m%d_%H%M%S")));
            std::fs::write(&p, "CHIUSURA GIORNALIERA SIMULATA").map_err(|e| RtError(e.to_string()))?;
            Ok(format!("Chiusura simulata: {}", p.display()))
        }
        _ => Err(RtError("Eseguire la chiusura giornaliera direttamente sul registratore".into())),
    }
}

fn simulate(folder: &str, r: &Receipt) -> Result<RtOutcome> {
    let dir = std::path::Path::new(folder);
    std::fs::create_dir_all(dir).map_err(|e| RtError(format!("Cartella simulatore: {}", e)))?;
    let n = std::fs::read_dir(dir).map(|d| d.filter(|e| e.as_ref().map(|e| e.path().extension().map(|x| x == "ini").unwrap_or(false)).unwrap_or(false)).count()).unwrap_or(0) + 1;
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    std::fs::write(dir.join(format!("scontrino_{}_{:04}.ini", stamp, n)), ini::to_ini(r, "Ethernet", "")).map_err(|e| RtError(e.to_string()))?;
    std::fs::write(dir.join(format!("scontrino_{}_{:04}.xml", stamp, n)), epson::receipt_xml(r)).map_err(|e| RtError(e.to_string()))?;
    Ok(RtOutcome { receipt_number: format!("{:04}", n), z_number: "0000".into(), serial: "SIMULATORE".into(), simulated: true, message: format!("File scritti in {}", dir.display()) })
}

/// Formatta un importo con due decimali e il separatore indicato.
pub fn amount(d: Decimal, sep: char) -> String {
    let s = openfatt_core::money::fmt_xml(d, 2);
    if sep == '.' {
        s
    } else {
        s.replace('.', &sep.to_string())
    }
}

/// Riduce un testo ai caratteri stampabili dal registratore e alla lunghezza massima.
pub fn printable(s: &str, max: usize) -> String {
    let t = openfatt_fatturapa::text::basic_latin(s);
    t.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

#[cfg(test)]
mod tests;
