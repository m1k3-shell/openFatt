//! Canale PEC verso il Sistema di Interscambio.
//!
//! - Invio: messaggio PEC all'indirizzo dello SdI con la fattura in allegato. Il primo
//!   invio va a sdi01@pec.fatturapa.it; lo SdI risponde da un indirizzo dedicato
//!   (sdiNN@pec.fatturapa.it) da usare per gli invii successivi.
//! - Ricezione: lettura IMAP della casella PEC; le ricevute PEC contengono il messaggio
//!   originale in `postacert.eml`, dal quale si estraggono notifiche e fatture.
//!
//! La classificazione dei messaggi è una funzione pura (`classify_message`), testabile
//! senza rete.

pub mod mail;

use mail_parser::{Message, MessageParser, MimeHeaders, PartType};
use serde::{Deserialize, Serialize};

pub const SDI_FIRST_ADDRESS: &str = "sdi01@pec.fatturapa.it";
/// Dimensione massima del messaggio accettata dallo SdI via PEC.
pub const MAX_MESSAGE_BYTES: usize = 30 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TlsMode {
    /// TLS implicito (porta 465 SMTP / 993 IMAP)
    #[default]
    Implicit,
    /// STARTTLS (porta 587 SMTP)
    StartTls,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PecConfig {
    /// Indirizzo PEC del mittente.
    pub address: String,
    /// Nome utente (di norma coincide con l'indirizzo).
    pub username: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_tls: TlsMode,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_folder: String,
    /// Indirizzo SdI a cui inviare (aggiornato automaticamente dopo la prima risposta).
    pub sdi_address: String,
    /// Giorni da rileggere nella casella a ogni controllo.
    pub fetch_days: u32,
}

impl Default for PecConfig {
    fn default() -> Self {
        PecConfig {
            address: String::new(),
            username: String::new(),
            smtp_host: String::new(),
            smtp_port: 465,
            smtp_tls: TlsMode::Implicit,
            imap_host: String::new(),
            imap_port: 993,
            imap_folder: "INBOX".into(),
            sdi_address: SDI_FIRST_ADDRESS.into(),
            fetch_days: 30,
        }
    }
}

/// Impostazioni note dei principali gestori PEC (da verificare con il proprio gestore).
pub fn provider_presets() -> Vec<(&'static str, &'static str, u16, &'static str, u16)> {
    vec![
        ("Aruba PEC", "smtps.pec.aruba.it", 465, "imaps.pec.aruba.it", 993),
        ("Legalmail (InfoCert)", "sendm.cert.legalmail.it", 465, "mbox.cert.legalmail.it", 993),
        ("Namirial / Sicurezza Postale", "smtps.sicurezzapostale.it", 465, "imaps.sicurezzapostale.it", 993),
        ("Register.it PEC", "smtps.pec.register.it", 465, "imaps.pec.register.it", 993),
    ]
}

#[derive(Debug)]
pub enum PecError {
    Config(String),
    Smtp(String),
    Imap(String),
    Message(String),
}

impl std::fmt::Display for PecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PecError::Config(s) => write!(f, "Configurazione PEC: {}", s),
            PecError::Smtp(s) => write!(f, "Invio PEC non riuscito: {}", s),
            PecError::Imap(s) => write!(f, "Lettura casella PEC non riuscita: {}", s),
            PecError::Message(s) => write!(f, "Messaggio non valido: {}", s),
        }
    }
}
impl std::error::Error for PecError {}

impl PecConfig {
    pub fn validate(&self) -> Result<(), PecError> {
        if !self.address.contains('@') {
            return Err(PecError::Config("indirizzo PEC mancante".into()));
        }
        if self.smtp_host.trim().is_empty() || self.imap_host.trim().is_empty() {
            return Err(PecError::Config("server SMTP/IMAP mancanti".into()));
        }
        if !self.sdi_address.ends_with("@pec.fatturapa.it") {
            return Err(PecError::Config("l'indirizzo SdI deve essere del dominio pec.fatturapa.it".into()));
        }
        Ok(())
    }
    fn user(&self) -> &str {
        if self.username.trim().is_empty() {
            self.address.trim()
        } else {
            self.username.trim()
        }
    }
}

/// Costruisce il messaggio di invio (senza spedirlo): utile per anteprima e test.
pub fn build_invoice_message(cfg: &PecConfig, file_name: &str, content: &[u8]) -> Result<lettre::Message, PecError> {
    use lettre::message::{header::ContentType, Attachment, MultiPart, SinglePart};
    if content.len() > MAX_MESSAGE_BYTES {
        return Err(PecError::Message("il file supera i 30 MB ammessi dallo SdI via PEC".into()));
    }
    let from = cfg.address.trim().parse().map_err(|e| PecError::Config(format!("mittente non valido: {}", e)))?;
    let to = cfg.sdi_address.trim().parse().map_err(|e| PecError::Config(format!("destinatario non valido: {}", e)))?;
    let ctype = if file_name.to_ascii_lowercase().ends_with(".p7m") { "application/pkcs7-mime" } else { "application/xml" };
    lettre::Message::builder()
        .from(from)
        .to(to)
        .subject(format!("Invio file {}", file_name))
        .multipart(
            MultiPart::mixed()
                .singlepart(SinglePart::plain(format!("Trasmissione al Sistema di Interscambio del file {}.", file_name)))
                .singlepart(Attachment::new(file_name.to_string()).body(content.to_vec(), ContentType::parse(ctype).unwrap())),
        )
        .map_err(|e| PecError::Message(e.to_string()))
}

/// Invia una fattura allo SdI. Restituisce una descrizione dell'invio.
pub fn send_invoice(cfg: &PecConfig, password: &str, file_name: &str, content: &[u8]) -> Result<String, PecError> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{SmtpTransport, Transport};
    cfg.validate()?;
    let msg = build_invoice_message(cfg, file_name, content)?;
    let builder = match cfg.smtp_tls {
        TlsMode::Implicit => SmtpTransport::relay(cfg.smtp_host.trim()),
        TlsMode::StartTls => SmtpTransport::starttls_relay(cfg.smtp_host.trim()),
    }
    .map_err(|e| PecError::Smtp(e.to_string()))?;
    let mailer = builder
        .port(cfg.smtp_port)
        .credentials(Credentials::new(cfg.user().to_string(), password.to_string()))
        .timeout(Some(std::time::Duration::from_secs(60)))
        .build();
    mailer.send(&msg).map_err(|e| PecError::Smtp(e.to_string()))?;
    Ok(format!("PEC inviata da {} a {}", cfg.address, cfg.sdi_address))
}

/// Verifica le credenziali collegandosi ai server SMTP e IMAP.
pub fn test_connection(cfg: &PecConfig, password: &str) -> Result<String, PecError> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::SmtpTransport;
    cfg.validate()?;
    let builder = match cfg.smtp_tls {
        TlsMode::Implicit => SmtpTransport::relay(cfg.smtp_host.trim()),
        TlsMode::StartTls => SmtpTransport::starttls_relay(cfg.smtp_host.trim()),
    }
    .map_err(|e| PecError::Smtp(e.to_string()))?;
    let mailer = builder.port(cfg.smtp_port).credentials(Credentials::new(cfg.user().to_string(), password.to_string())).build();
    let ok = mailer.test_connection().map_err(|e| PecError::Smtp(e.to_string()))?;
    if !ok {
        return Err(PecError::Smtp("il server SMTP non risponde".into()));
    }
    let mut session = imap_session(cfg, password)?;
    let mbox = session.select(&cfg.imap_folder).map_err(|e| PecError::Imap(e.to_string()))?;
    let _ = session.logout();
    Ok(format!("Connessione riuscita: SMTP ok, IMAP ok ({} messaggi in {})", mbox.exists, cfg.imap_folder))
}

fn imap_session(cfg: &PecConfig, password: &str) -> Result<imap::Session<native_tls::TlsStream<std::net::TcpStream>>, PecError> {
    let tls = native_tls::TlsConnector::builder().build().map_err(|e| PecError::Imap(e.to_string()))?;
    let host = cfg.imap_host.trim();
    let client = imap::connect((host, cfg.imap_port), host, &tls).map_err(|e| PecError::Imap(e.to_string()))?;
    client.login(cfg.user(), password).map_err(|(e, _)| PecError::Imap(format!("accesso negato: {}", e)))
}

/// Un file rilevante estratto da un messaggio PEC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundFile {
    pub file_name: String,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundMessage {
    pub message_id: String,
    pub subject: String,
    /// Mittente del messaggio originale (es. sdi23@pec.fatturapa.it).
    pub original_from: String,
    /// Notifiche SdI (RC, NS, MC, NE, DT, AT, ...).
    pub notifications: Vec<InboundFile>,
    /// Fatture ricevute (.xml o .xml.p7m).
    pub invoices: Vec<InboundFile>,
    /// File metadati (MT) che accompagnano le fatture ricevute.
    pub metadata: Vec<InboundFile>,
    /// È una ricevuta della PEC (accettazione/consegna del gestore), non un messaggio SdI.
    pub is_pec_receipt: bool,
}

impl InboundMessage {
    pub fn is_from_sdi(&self) -> bool {
        self.original_from.to_ascii_lowercase().ends_with("@pec.fatturapa.it")
    }
    pub fn is_relevant(&self) -> bool {
        !self.notifications.is_empty() || !self.invoices.is_empty()
    }
}

fn first_address(m: &Message) -> String {
    m.from()
        .and_then(|a| a.first())
        .and_then(|a| a.address())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn collect_files(m: &Message, out: &mut Vec<InboundFile>, inner: &mut Option<String>, depth: usize) {
    if depth > 4 {
        return;
    }
    for part in m.attachments() {
        let name = part.attachment_name().unwrap_or("").to_string();
        if let PartType::Message(nested) = &part.body {
            // postacert.eml: messaggio originale incapsulato dal gestore PEC
            if inner.is_none() {
                *inner = Some(first_address(nested));
            }
            collect_files(nested, out, inner, depth + 1);
            continue;
        }
        if name.is_empty() {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if lower == "daticert.xml" || lower.ends_with(".p7s") || lower == "postacert.eml" {
            if lower == "postacert.eml" {
                if let Some(nested) = MessageParser::default().parse(part.contents()) {
                    if inner.is_none() {
                        *inner = Some(first_address(&nested));
                    }
                    collect_files(&nested, out, inner, depth + 1);
                }
            }
            continue;
        }
        out.push(InboundFile { file_name: name, content: part.contents().to_vec() });
    }
}

/// Analizza un messaggio (formato RFC 822) e ne estrae notifiche e fatture.
pub fn classify_message(raw: &[u8]) -> Result<InboundMessage, PecError> {
    let m = MessageParser::default().parse(raw).ok_or_else(|| PecError::Message("formato email non riconosciuto".into()))?;
    let message_id = m.message_id().map(|s| s.to_string()).unwrap_or_else(|| {
        // in assenza di Message-ID si usa un'impronta del contenuto
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        raw.hash(&mut h);
        format!("nomsgid-{:x}", h.finish())
    });
    let subject = m.subject().unwrap_or("").to_string();
    let mut files = Vec::new();
    let mut inner_from = None;
    collect_files(&m, &mut files, &mut inner_from, 0);
    let header_type = m.header_raw("X-Ricevuta").map(|s| s.trim().to_ascii_lowercase());
    let is_receipt = matches!(header_type.as_deref(), Some("accettazione") | Some("avvenuta-consegna") | Some("non-accettazione") | Some("preavviso-errore-consegna") | Some("errore-consegna"));
    let mut msg = InboundMessage {
        message_id,
        subject,
        original_from: inner_from.unwrap_or_else(|| first_address(&m)),
        notifications: vec![],
        invoices: vec![],
        metadata: vec![],
        is_pec_receipt: is_receipt,
    };
    for f in files {
        let lower = f.file_name.to_ascii_lowercase();
        if !(lower.ends_with(".xml") || lower.ends_with(".p7m") || lower.ends_with(".zip")) {
            continue;
        }
        match openfatt_fatturapa::naming::parse_file_name(&f.file_name).and_then(|p| p.message_type) {
            Some(t) if t == "MT" => msg.metadata.push(f),
            Some(_) => msg.notifications.push(f),
            None => msg.invoices.push(f),
        }
    }
    Ok(msg)
}

fn imap_date(days_ago: u32) -> String {
    let d = chrono::Local::now().date_naive() - chrono::Duration::days(days_ago as i64);
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    use chrono::Datelike;
    format!("{:02}-{}-{}", d.day(), M[d.month0() as usize], d.year())
}

/// Legge i messaggi recenti della casella PEC. `seen` indica i Message-ID già elaborati.
pub fn fetch_messages(cfg: &PecConfig, password: &str, seen: &dyn Fn(&str) -> bool) -> Result<Vec<InboundMessage>, PecError> {
    cfg.validate()?;
    let mut session = imap_session(cfg, password)?;
    session.select(&cfg.imap_folder).map_err(|e| PecError::Imap(e.to_string()))?;
    let uids = session.uid_search(format!("SINCE {}", imap_date(cfg.fetch_days.max(1)))).map_err(|e| PecError::Imap(e.to_string()))?;
    let mut uids: Vec<u32> = uids.into_iter().collect();
    uids.sort();
    let mut out = Vec::new();
    for chunk in uids.chunks(20) {
        let set: Vec<String> = chunk.iter().map(|u| u.to_string()).collect();
        // BODY.PEEK non modifica il flag "letto" sul server
        let fetched = session.uid_fetch(set.join(","), "BODY.PEEK[]").map_err(|e| PecError::Imap(e.to_string()))?;
        for f in fetched.iter() {
            let Some(body) = f.body() else { continue };
            match classify_message(body) {
                Ok(m) => {
                    if !seen(&m.message_id) {
                        out.push(m);
                    }
                }
                Err(_) => continue,
            }
        }
    }
    let _ = session.logout();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pec_envelope(inner: &str) -> String {
        format!(
            "From: \"Per conto di: sdi23@pec.fatturapa.it\" <posta-certificata@pec.aruba.it>\r\n\
To: azienda@pec.example.it\r\n\
Subject: POSTA CERTIFICATA: Notifica di scarto\r\n\
Message-ID: <opec-123@pec.aruba.it>\r\n\
X-Trasporto: posta-certificata\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"OUTER\"\r\n\r\n\
--OUTER\r\nContent-Type: text/plain\r\n\r\nMessaggio di posta certificata\r\n\
--OUTER\r\nContent-Type: application/xml; name=\"daticert.xml\"\r\nContent-Disposition: attachment; filename=\"daticert.xml\"\r\n\r\n<postacert/>\r\n\
--OUTER\r\nContent-Type: message/rfc822; name=\"postacert.eml\"\r\nContent-Disposition: attachment; filename=\"postacert.eml\"\r\n\r\n{}\r\n\
--OUTER--\r\n",
            inner
        )
    }

    #[test]
    fn extracts_notification_from_pec_envelope() {
        let inner = "From: sdi23@pec.fatturapa.it\r\n\
To: azienda@pec.example.it\r\n\
Subject: Notifica di scarto\r\n\
Message-ID: <sdi-1@pec.fatturapa.it>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"INNER\"\r\n\r\n\
--INNER\r\nContent-Type: text/plain\r\n\r\nNotifica\r\n\
--INNER\r\nContent-Type: application/xml; name=\"IT01234567890_00001_NS_001.xml\"\r\nContent-Disposition: attachment; filename=\"IT01234567890_00001_NS_001.xml\"\r\nContent-Transfer-Encoding: base64\r\n\r\nPE5vdGlmaWNhU2NhcnRvLz4=\r\n\
--INNER--";
        let raw = pec_envelope(inner);
        let m = classify_message(raw.as_bytes()).unwrap();
        assert_eq!(m.original_from, "sdi23@pec.fatturapa.it");
        assert!(m.is_from_sdi());
        assert_eq!(m.notifications.len(), 1);
        assert_eq!(m.notifications[0].file_name, "IT01234567890_00001_NS_001.xml");
        assert_eq!(m.notifications[0].content, b"<NotificaScarto/>");
        assert!(m.invoices.is_empty());
    }

    #[test]
    fn extracts_received_invoice_and_metadata() {
        let inner = "From: sdi05@pec.fatturapa.it\r\nTo: a@b.it\r\nSubject: Invio File 123\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"I\"\r\n\r\n\
--I\r\nContent-Type: application/octet-stream; name=\"IT09876543210_A1B2C.xml.p7m\"\r\nContent-Disposition: attachment; filename=\"IT09876543210_A1B2C.xml.p7m\"\r\nContent-Transfer-Encoding: base64\r\n\r\nAAEC\r\n\
--I\r\nContent-Type: application/xml; name=\"IT09876543210_A1B2C_MT_001.xml\"\r\nContent-Disposition: attachment; filename=\"IT09876543210_A1B2C_MT_001.xml\"\r\n\r\n<MetadatiInvioFile/>\r\n\
--I--";
        let m = classify_message(pec_envelope(inner).as_bytes()).unwrap();
        assert_eq!(m.invoices.len(), 1);
        assert_eq!(m.invoices[0].content, vec![0u8, 1, 2]);
        assert_eq!(m.metadata.len(), 1);
    }

    #[test]
    fn builds_outgoing_message() {
        let cfg = PecConfig { address: "azienda@pec.example.it".into(), smtp_host: "smtp".into(), imap_host: "imap".into(), ..Default::default() };
        let msg = build_invoice_message(&cfg, "IT01234567890_00001.xml", b"<x/>").unwrap();
        let raw = String::from_utf8(msg.formatted()).unwrap();
        assert!(raw.contains("To: sdi01@pec.fatturapa.it"));
        assert!(raw.contains("IT01234567890_00001.xml"));
        assert!(cfg.validate().is_ok());
    }
}
