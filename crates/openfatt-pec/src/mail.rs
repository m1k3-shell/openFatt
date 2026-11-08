//! Posta elettronica ordinaria: invio di documenti, solleciti e comunicazioni.
//!
//! Tre modi di invio:
//! - `Smtp`: una casella di posta dell'azienda (Gmail, Outlook.com, Aruba, il proprio dominio...);
//! - `Pec`: la stessa casella PEC usata per lo SdI (utile per i solleciti, che via PEC hanno
//!   valore di prova dell'invio e della consegna);
//! - `MailClient`: il messaggio viene preparato come file .eml "da inviare" e aperto nel
//!   programma di posta del computer (Outlook lo apre come nuova email pronta da spedire).
//!
//! La costruzione del messaggio è pura e testabile senza rete.

use lettre::message::{header::ContentType, Attachment as LAttachment, Mailbox, MultiPart, SinglePart};
use serde::{Deserialize, Serialize};

use crate::{PecConfig, PecError, TlsMode};

/// Dimensione massima del messaggio (molti server rifiutano oltre 20-25 MB).
pub const MAX_MAIL_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum MailMode {
    /// Casella email dell'azienda, server SMTP configurato qui.
    #[default]
    Smtp,
    /// Invio dalla casella PEC configurata per lo SdI.
    Pec,
    /// Apertura nel programma di posta del computer.
    MailClient,
}

impl MailMode {
    pub fn label(self) -> &'static str {
        match self {
            MailMode::Smtp => "Casella email (SMTP)",
            MailMode::Pec => "Casella PEC",
            MailMode::MailClient => "Programma di posta del computer",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MailConfig {
    pub mode: MailMode,
    /// Nome mostrato come mittente (di norma la ragione sociale).
    pub from_name: String,
    pub address: String,
    /// Nome utente (se diverso dall'indirizzo).
    pub username: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_tls: TlsMode,
    /// Indirizzo per le risposte, se diverso dal mittente.
    pub reply_to: String,
    /// Copia nascosta a sé stessi di ogni messaggio inviato.
    pub bcc_self: bool,
    /// Firma aggiunta in fondo ai messaggi (segnaposto {firma}).
    pub signature: String,
}

impl Default for MailConfig {
    fn default() -> Self {
        MailConfig {
            mode: MailMode::Smtp,
            from_name: String::new(),
            address: String::new(),
            username: String::new(),
            smtp_host: String::new(),
            smtp_port: 465,
            smtp_tls: TlsMode::Implicit,
            reply_to: String::new(),
            bcc_self: false,
            signature: String::new(),
        }
    }
}

/// Impostazioni note dei principali servizi di posta (da verificare con il proprio fornitore).
/// Gmail e Outlook.com richiedono una "password per le app" se è attiva la verifica in due passaggi.
pub fn smtp_presets() -> Vec<(&'static str, &'static str, u16, TlsMode)> {
    vec![
        ("Gmail", "smtp.gmail.com", 465, TlsMode::Implicit),
        ("Outlook / Microsoft 365", "smtp.office365.com", 587, TlsMode::StartTls),
        ("Aruba", "smtps.aruba.it", 465, TlsMode::Implicit),
        ("Libero", "smtp.libero.it", 465, TlsMode::Implicit),
        ("Register.it", "authsmtp.securemail.pro", 465, TlsMode::Implicit),
    ]
}

/// Server e credenziali effettivi per l'invio.
#[derive(Debug, Clone, PartialEq)]
pub struct SmtpAccount {
    pub from_name: String,
    pub address: String,
    pub username: String,
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
}

impl MailConfig {
    /// Account di invio secondo il modo scelto (per `MailClient` non serve un server).
    pub fn account(&self, pec: &PecConfig) -> Result<SmtpAccount, PecError> {
        match self.mode {
            MailMode::Smtp | MailMode::MailClient => {
                if !is_valid_address(self.address.trim()) {
                    return Err(PecError::Config("indirizzo email del mittente mancante o non valido (Opzioni > Email)".into()));
                }
                if self.mode == MailMode::Smtp && self.smtp_host.trim().is_empty() {
                    return Err(PecError::Config("server SMTP mancante (Opzioni > Email)".into()));
                }
                Ok(SmtpAccount {
                    from_name: self.from_name.trim().to_string(),
                    address: self.address.trim().to_string(),
                    username: if self.username.trim().is_empty() { self.address.trim().to_string() } else { self.username.trim().to_string() },
                    host: self.smtp_host.trim().to_string(),
                    port: self.smtp_port,
                    tls: self.smtp_tls,
                })
            }
            MailMode::Pec => {
                if !is_valid_address(pec.address.trim()) || pec.smtp_host.trim().is_empty() {
                    return Err(PecError::Config("casella PEC non configurata (Opzioni > PEC)".into()));
                }
                Ok(SmtpAccount {
                    from_name: self.from_name.trim().to_string(),
                    address: pec.address.trim().to_string(),
                    username: if pec.username.trim().is_empty() { pec.address.trim().to_string() } else { pec.username.trim().to_string() },
                    host: pec.smtp_host.trim().to_string(),
                    port: pec.smtp_port,
                    tls: pec.smtp_tls,
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MailAttachment {
    pub name: String,
    pub mime: String,
    pub data: Vec<u8>,
}

impl MailAttachment {
    pub fn pdf(name: impl Into<String>, data: Vec<u8>) -> Self {
        MailAttachment { name: name.into(), mime: "application/pdf".into(), data }
    }
    pub fn xml(name: impl Into<String>, data: Vec<u8>) -> Self {
        let name = name.into();
        let mime = if name.to_ascii_lowercase().ends_with(".p7m") { "application/pkcs7-mime" } else { "application/xml" };
        MailAttachment { name, mime: mime.into(), data }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutgoingMail {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub attachments: Vec<MailAttachment>,
}

impl OutgoingMail {
    pub fn size(&self) -> usize {
        self.body.len() + self.attachments.iter().map(|a| a.data.len()).sum::<usize>()
    }
}

/// Controllo sintattico essenziale di un indirizzo email.
pub fn is_valid_address(a: &str) -> bool {
    let a = a.trim();
    if a.contains(char::is_whitespace) || a.matches('@').count() != 1 {
        return false;
    }
    let (local, domain) = a.split_once('@').unwrap();
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && !domain.contains("..") && a.parse::<lettre::Address>().is_ok()
}

/// Divide un elenco di indirizzi separati da virgola, punto e virgola o spazi.
/// Restituisce gli indirizzi validi (senza doppioni) o l'elenco di quelli errati.
pub fn parse_addresses(s: &str) -> Result<Vec<String>, String> {
    let mut ok: Vec<String> = Vec::new();
    let mut bad = Vec::new();
    for part in s.split([',', ';', ' ', '\n', '\t']).map(str::trim).filter(|p| !p.is_empty()) {
        // accetta anche "Nome <indirizzo>" ridotto a "<indirizzo>"
        let p = part.trim_start_matches('<').trim_end_matches('>');
        if is_valid_address(p) {
            if !ok.iter().any(|x| x.eq_ignore_ascii_case(p)) {
                ok.push(p.to_string());
            }
        } else {
            bad.push(part.to_string());
        }
    }
    if bad.is_empty() {
        Ok(ok)
    } else {
        Err(format!("indirizzi non validi: {}", bad.join(", ")))
    }
}

fn mailbox(name: &str, addr: &str) -> Result<Mailbox, PecError> {
    let a: lettre::Address = addr.trim().parse().map_err(|e| PecError::Message(format!("indirizzo non valido {}: {}", addr, e)))?;
    Ok(Mailbox::new(if name.trim().is_empty() { None } else { Some(name.trim().to_string()) }, a))
}

/// Costruisce il messaggio pronto per l'invio.
pub fn build_message(acc: &SmtpAccount, reply_to: &str, bcc_self: bool, mail: &OutgoingMail) -> Result<lettre::Message, PecError> {
    if mail.to.is_empty() {
        return Err(PecError::Message("nessun destinatario".into()));
    }
    if mail.size() > MAX_MAIL_BYTES {
        return Err(PecError::Message(format!("il messaggio supera i {} MB: togliere qualche allegato", MAX_MAIL_BYTES / 1024 / 1024)));
    }
    let mut b = lettre::Message::builder().from(mailbox(&acc.from_name, &acc.address)?).subject(mail.subject.trim());
    for t in &mail.to {
        b = b.to(mailbox("", t)?);
    }
    for c in &mail.cc {
        b = b.cc(mailbox("", c)?);
    }
    if is_valid_address(reply_to) && !reply_to.trim().eq_ignore_ascii_case(&acc.address) {
        b = b.reply_to(mailbox(&acc.from_name, reply_to)?);
    }
    if bcc_self {
        b = b.bcc(mailbox(&acc.from_name, &acc.address)?);
    }
    let text = SinglePart::plain(mail.body.replace("\r\n", "\n"));
    let msg = if mail.attachments.is_empty() {
        b.singlepart(text)
    } else {
        let mut mp = MultiPart::mixed().singlepart(text);
        for a in &mail.attachments {
            let ct = ContentType::parse(&a.mime).unwrap_or(ContentType::parse("application/octet-stream").unwrap());
            mp = mp.singlepart(LAttachment::new(a.name.clone()).body(a.data.clone(), ct));
        }
        b.multipart(mp)
    };
    msg.map_err(|e| PecError::Message(e.to_string()))
}

/// Messaggio in formato .eml da aprire nel programma di posta come nuova email da inviare.
pub fn draft_eml(acc: &SmtpAccount, reply_to: &str, bcc_self: bool, mail: &OutgoingMail) -> Result<Vec<u8>, PecError> {
    let msg = build_message(acc, reply_to, bcc_self, mail)?;
    // X-Unsent: 1 fa aprire il file come bozza (Outlook, Posta di Windows)
    let mut out = b"X-Unsent: 1\r\n".to_vec();
    out.extend_from_slice(&msg.formatted());
    Ok(out)
}

fn transport(acc: &SmtpAccount, password: &str) -> Result<lettre::SmtpTransport, PecError> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::SmtpTransport;
    let builder = match acc.tls {
        TlsMode::Implicit => SmtpTransport::relay(&acc.host),
        TlsMode::StartTls => SmtpTransport::starttls_relay(&acc.host),
    }
    .map_err(|e| PecError::Smtp(e.to_string()))?;
    Ok(builder
        .port(acc.port)
        .credentials(Credentials::new(acc.username.clone(), password.to_string()))
        .timeout(Some(std::time::Duration::from_secs(60)))
        .build())
}

/// Invia un messaggio. Restituisce una descrizione dell'invio.
pub fn send(acc: &SmtpAccount, password: &str, reply_to: &str, bcc_self: bool, mail: &OutgoingMail) -> Result<String, PecError> {
    use lettre::Transport;
    let msg = build_message(acc, reply_to, bcc_self, mail)?;
    transport(acc, password)?.send(&msg).map_err(|e| PecError::Smtp(e.to_string()))?;
    Ok(format!("inviata da {} a {}", acc.address, mail.to.join(", ")))
}

/// Verifica le credenziali collegandosi al server SMTP.
pub fn test_smtp(acc: &SmtpAccount, password: &str) -> Result<String, PecError> {
    let ok = transport(acc, password)?.test_connection().map_err(|e| PecError::Smtp(e.to_string()))?;
    if ok {
        Ok(format!("Connessione riuscita a {}:{} come {}", acc.host, acc.port, acc.username))
    } else {
        Err(PecError::Smtp("il server SMTP non risponde".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc() -> SmtpAccount {
        SmtpAccount { from_name: "Rossi Arredamenti S.r.l.".into(), address: "amministrazione@rossi.example".into(), username: "x".into(), host: "smtp.example".into(), port: 465, tls: TlsMode::Implicit }
    }

    #[test]
    fn indirizzi() {
        assert!(is_valid_address("mario.rossi@esempio.it"));
        assert!(!is_valid_address("mario.rossi@esempio"));
        assert!(!is_valid_address("mario rossi@esempio.it"));
        assert!(!is_valid_address("@esempio.it"));
        assert_eq!(parse_addresses("a@b.it; c@d.com, a@b.it\n<e@f.org>").unwrap(), vec!["a@b.it", "c@d.com", "e@f.org"]);
        assert!(parse_addresses("a@b.it, sbagliato").unwrap_err().contains("sbagliato"));
        assert_eq!(parse_addresses("  ").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn messaggio_con_allegato() {
        let m = OutgoingMail {
            to: vec!["cliente@esempio.it".into()],
            cc: vec!["contabilita@esempio.it".into()],
            subject: "Fattura n. 12 del 03/09/2026".into(),
            body: "Gentile cliente,\nin allegato la fattura.\nÈ tutto.".into(),
            attachments: vec![MailAttachment::pdf("Fattura 12-2026.pdf", b"%PDF-1.4 prova".to_vec())],
        };
        let raw = String::from_utf8(build_message(&acc(), "risposte@rossi.example", true, &m).unwrap().formatted()).unwrap();
        assert!(raw.contains("To: cliente@esempio.it"));
        assert!(raw.contains("Cc: contabilita@esempio.it"));
        assert!(raw.contains("Reply-To:"));
        assert!(raw.contains("multipart/mixed"));
        assert!(raw.contains("application/pdf"));
        assert!(raw.contains("Fattura 12-2026.pdf"));
        // l'oggetto è codificato correttamente
        assert!(raw.contains("Subject: Fattura n. 12 del 03/09/2026"));
        let eml = draft_eml(&acc(), "", false, &m).unwrap();
        assert!(eml.starts_with(b"X-Unsent: 1\r\n"));
    }

    #[test]
    fn bozza_eml_riletta() {
        use mail_parser::{MessageParser, MimeHeaders};
        let pdf: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        let m = OutgoingMail {
            to: vec!["cliente@esempio.it".into()],
            subject: "Promemoria di pagamento - Rossi Arredamenti S.r.l.".into(),
            body: "Gentile Bianchi,\nrisultano da saldare € 1.570,50.\n".into(),
            attachments: vec![MailAttachment::pdf("Fattura 12.pdf", pdf.clone())],
            ..Default::default()
        };
        let eml = draft_eml(&acc(), "", false, &m).unwrap();
        let parsed = MessageParser::default().parse(&eml).unwrap();
        assert_eq!(parsed.subject(), Some("Promemoria di pagamento - Rossi Arredamenti S.r.l."));
        assert!(parsed.body_text(0).unwrap().contains("€ 1.570,50"));
        let att = parsed.attachments().next().unwrap();
        assert_eq!(att.attachment_name(), Some("Fattura 12.pdf"));
        assert_eq!(att.contents(), &pdf[..]);
        assert_eq!(parsed.from().unwrap().first().unwrap().address(), Some("amministrazione@rossi.example"));
    }

    #[test]
    fn controlli() {
        let mut m = OutgoingMail { to: vec![], subject: "x".into(), ..Default::default() };
        assert!(build_message(&acc(), "", false, &m).is_err());
        m.to = vec!["a@b.it".into()];
        m.attachments = vec![MailAttachment::pdf("grande.pdf", vec![0u8; MAX_MAIL_BYTES + 1])];
        assert!(build_message(&acc(), "", false, &m).unwrap_err().to_string().contains("MB"));
    }

    #[test]
    fn account_secondo_il_modo() {
        let pec = PecConfig { address: "azienda@pec.it".into(), smtp_host: "smtps.pec.aruba.it".into(), ..Default::default() };
        let mut c = MailConfig { address: "info@azienda.it".into(), smtp_host: "smtp.azienda.it".into(), ..Default::default() };
        assert_eq!(c.account(&pec).unwrap().address, "info@azienda.it");
        c.mode = MailMode::Pec;
        let a = c.account(&pec).unwrap();
        assert_eq!((a.address.as_str(), a.host.as_str()), ("azienda@pec.it", "smtps.pec.aruba.it"));
        c.mode = MailMode::MailClient;
        c.smtp_host.clear();
        assert!(c.account(&pec).is_ok());
        c.mode = MailMode::Smtp;
        assert!(c.account(&pec).is_err());
    }
}
