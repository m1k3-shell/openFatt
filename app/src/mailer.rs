//! Preparazione e invio delle email: documenti, solleciti, comunicazioni.
//!
//! La preparazione (testo dai modelli, allegati PDF) avviene sul thread dell'interfaccia;
//! l'invio SMTP in un thread separato, un messaggio alla volta; l'esito di ogni messaggio
//! torna all'interfaccia che lo registra nell'archivio.

use std::sync::mpsc::Sender;

use openfatt_core::calc;
use openfatt_core::mailtext::{self, MailTemplates, OverdueItem};
use openfatt_core::model::{Document, Party};
use openfatt_pec::mail::{self, MailAttachment, MailConfig, MailMode, OutgoingMail, SmtpAccount};
use openfatt_store::{MailKind, MailLogEntry, ReminderCandidate, Store};

use crate::config;
use crate::services;

pub fn mail_config(store: &Store) -> MailConfig {
    let mut c: MailConfig = store.get_setting("mail").unwrap_or_default();
    if c.from_name.trim().is_empty() {
        c.from_name = store.company().map(|c| c.display_name()).unwrap_or_default();
    }
    c
}

pub fn templates(store: &Store) -> MailTemplates {
    store.get_setting("mail_templates").unwrap_or_default()
}

/// Chiave nel Gestore credenziali per la casella email (distinta dalla PEC).
fn key(address: &str) -> String {
    format!("smtp:{}", address.trim())
}
pub fn load_password(address: &str) -> Option<String> {
    config::load_password(&key(address))
}
pub fn store_password(address: &str, pw: &str) -> Result<(), String> {
    config::store_password(&key(address), pw)
}
pub fn delete_password(address: &str) {
    config::delete_password(&key(address))
}

/// Un messaggio pronto da rivedere e inviare.
#[derive(Clone, Debug)]
pub struct Compose {
    pub kind: MailKind,
    pub party_id: Option<i64>,
    pub party_name: String,
    pub document_id: Option<i64>,
    pub level: u8,
    pub due_ids: Vec<i64>,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub body: String,
    pub attachments: Vec<MailAttachment>,
    /// Invio dalla casella PEC invece che dalla casella email.
    pub via_pec: bool,
    /// Avvertenza da mostrare (es. cliente senza indirizzo).
    pub note: String,
}

impl Compose {
    pub fn title(&self) -> String {
        match self.kind {
            MailKind::Document => "Invio del documento".into(),
            MailKind::Reminder => mailtext::reminder_level_label(self.level).to_string(),
            MailKind::Communication => "Comunicazione".into(),
        }
    }

    pub fn outgoing(&self) -> Result<OutgoingMail, String> {
        let to = mail::parse_addresses(&self.to).map_err(|e| format!("Destinatario: {}", e))?;
        if to.is_empty() {
            return Err("Indicare almeno un destinatario".into());
        }
        let cc = mail::parse_addresses(&self.cc).map_err(|e| format!("Copia: {}", e))?;
        if self.subject.trim().is_empty() {
            return Err("Indicare l'oggetto".into());
        }
        Ok(OutgoingMail { to, cc, subject: self.subject.trim().to_string(), body: self.body.clone(), attachments: self.attachments.clone() })
    }

    pub fn log_entry(&self) -> MailLogEntry {
        MailLogEntry {
            kind: self.kind,
            party_id: self.party_id,
            document_id: self.document_id,
            level: self.level,
            recipients: self.to.trim().to_string(),
            subject: self.subject.trim().to_string(),
            due_ids: self.due_ids.clone(),
            ..Default::default()
        }
    }
}

fn recipient(email: &str, pec: &str) -> (String, String) {
    if !email.trim().is_empty() {
        (email.trim().to_string(), String::new())
    } else if !pec.trim().is_empty() {
        (pec.trim().to_string(), "Il cliente non ha un indirizzo email: si usa la sua PEC.".into())
    } else {
        (String::new(), "Il cliente non ha né email né PEC nell'anagrafica: indicare l'indirizzo.".into())
    }
}

fn pdf_name(doc: &Document) -> String {
    format!("{} {}.pdf", doc.kind.label(), doc.number_label().replace(['/', '\\', '(', ')'], "-")).replace("  ", " ")
}

/// Email con il PDF del documento.
pub fn for_document(store: &Store, doc: &Document) -> Result<Compose, String> {
    let company = store.company().map_err(|e| e.to_string())?;
    let cfg = mail_config(store);
    let tpl = templates(store);
    let live: Option<Party> = doc.party_id.and_then(|id| store.party(id).ok().flatten());
    let party = live.as_ref().unwrap_or(&doc.party);
    let name = if doc.party.display_name().is_empty() { party.display_name() } else { doc.party.display_name() };
    let (to, note) = recipient(&party.email, &party.pec);
    let total = calc::compute(doc).map(|t| t.total).map_err(|e| e.0)?;
    let mut vars = mailtext::common_vars(&company, &name, &cfg.signature, chrono::Local::now().date_naive());
    vars.extend(mailtext::document_vars(doc, total));
    let pdf = services::document_pdf(store, doc)?;
    Ok(Compose {
        kind: MailKind::Document,
        party_id: doc.party_id,
        party_name: name,
        document_id: Some(doc.id),
        level: 0,
        due_ids: Vec::new(),
        to,
        cc: String::new(),
        subject: mailtext::render(&tpl.document.subject, &vars),
        body: mailtext::render(&tpl.document.body, &vars),
        attachments: vec![MailAttachment::pdf(pdf_name(doc), pdf)],
        via_pec: false,
        note,
    })
}

/// Sollecito di pagamento al livello indicato; con `attach_pdfs` allega le fatture scadute.
pub fn for_reminder(store: &Store, c: &ReminderCandidate, level: u8, attach_pdfs: bool) -> Compose {
    let company = store.company().unwrap_or_default();
    let cfg = mail_config(store);
    let tpl = templates(store);
    let today = chrono::Local::now().date_naive();
    let items: Vec<OverdueItem> = c.dues.iter().map(|d| OverdueItem { doc_label: d.doc_label.clone(), due: d.date, residual: d.residual() }).collect();
    let mut vars = mailtext::common_vars(&company, &c.party_name, &cfg.signature, today);
    vars.extend(mailtext::reminder_vars(&items, &company, today));
    let t = tpl.reminder(level);
    // l'ultimo avviso va di preferenza via PEC (prova di invio e consegna), se possibile
    let pec_ready = services::pec_config(store).validate().is_ok() || cfg.mode == MailMode::Pec;
    let use_pec = level >= 3 && pec_ready && !c.pec.is_empty() && cfg.mode != MailMode::MailClient;
    let (to, mut note) = if use_pec { (c.pec.clone(), "Ultimo avviso inviato via PEC all'indirizzo PEC del cliente: vale come prova di invio e consegna.".to_string()) } else { recipient(&c.email, &c.pec) };
    let mut attachments = Vec::new();
    if attach_pdfs {
        let mut seen = std::collections::BTreeSet::new();
        for d in &c.dues {
            if !seen.insert(d.document_id) {
                continue;
            }
            match store.document(d.document_id).map_err(|e| e.to_string()).and_then(|doc| services::document_pdf(store, &doc).map(|p| (pdf_name(&doc), p))) {
                Ok((n, p)) => attachments.push(MailAttachment::pdf(n, p)),
                Err(e) => note = format!("{} PDF di {} non allegato: {}", note, d.doc_label, e).trim().to_string(),
            }
        }
    }
    Compose {
        kind: MailKind::Reminder,
        party_id: c.party_id,
        party_name: c.party_name.clone(),
        document_id: None,
        level: level.clamp(1, 3),
        due_ids: c.dues.iter().map(|d| d.id).collect(),
        to,
        cc: String::new(),
        subject: mailtext::render(&t.subject, &vars),
        body: mailtext::render(&t.body, &vars),
        attachments,
        via_pec: use_pec,
        note,
    }
}

/// Comunicazione libera a un cliente o fornitore.
pub fn for_communication(store: &Store, p: &Party, subject: &str, body: &str) -> Compose {
    let company = store.company().unwrap_or_default();
    let cfg = mail_config(store);
    let vars = mailtext::common_vars(&company, &p.display_name(), &cfg.signature, chrono::Local::now().date_naive());
    let (to, note) = recipient(&p.email, &p.pec);
    Compose {
        kind: MailKind::Communication,
        party_id: Some(p.id),
        party_name: p.display_name(),
        document_id: None,
        level: 0,
        due_ids: Vec::new(),
        to,
        cc: String::new(),
        subject: mailtext::render(subject, &vars),
        body: mailtext::render(body, &vars),
        attachments: Vec::new(),
        via_pec: false,
        note,
    }
}

/// Account e password necessari per un messaggio.
pub fn account_for(store: &Store, via_pec: bool) -> Result<(SmtpAccount, Option<String>), String> {
    let mut cfg = mail_config(store);
    let pec = services::pec_config(store);
    if via_pec {
        cfg.mode = MailMode::Pec;
    }
    let acc = cfg.account(&pec).map_err(|e| e.to_string())?;
    let pw = if cfg.mode == MailMode::Pec { config::load_password(&pec.address) } else { load_password(&acc.address) };
    Ok((acc, pw))
}

pub struct Job {
    pub acc: SmtpAccount,
    pub password: String,
    pub reply_to: String,
    pub bcc_self: bool,
    pub mail: OutgoingMail,
    pub log: MailLogEntry,
}

pub enum MailJobResult {
    /// Esito di un messaggio.
    Done { log: MailLogEntry, result: Result<String, String> },
    /// Tutti i messaggi del lavoro sono stati elaborati.
    Finished,
    Tested(Result<String, String>),
}

pub fn spawn(jobs: Vec<Job>, tx: Sender<MailJobResult>) {
    std::thread::spawn(move || {
        for (i, j) in jobs.into_iter().enumerate() {
            if i > 0 {
                // pausa breve tra i messaggi: alcuni server limitano gli invii ravvicinati
                std::thread::sleep(std::time::Duration::from_millis(800));
            }
            let r = mail::send(&j.acc, &j.password, &j.reply_to, j.bcc_self, &j.mail).map_err(|e| e.to_string());
            let _ = tx.send(MailJobResult::Done { log: j.log, result: r });
        }
        let _ = tx.send(MailJobResult::Finished);
    });
}

pub fn spawn_test(acc: SmtpAccount, password: String, tx: Sender<MailJobResult>) {
    std::thread::spawn(move || {
        let _ = tx.send(MailJobResult::Tested(mail::test_smtp(&acc, &password).map_err(|e| e.to_string())));
    });
}

/// Apre il messaggio nel programma di posta del computer come nuova email da inviare.
pub fn open_in_client(store: &Store, c: &Compose) -> Result<(), String> {
    let cfg = mail_config(store);
    let acc = cfg.account(&services::pec_config(store)).map_err(|e| e.to_string())?;
    let eml = mail::draft_eml(&acc, &cfg.reply_to, cfg.bcc_self, &c.outgoing()?).map_err(|e| e.to_string())?;
    let name = format!("{}.eml", c.subject.chars().take(60).collect::<String>());
    services::open_bytes(&name, &eml)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::model::DocKind;

    #[test]
    fn documento_e_sollecito_dall_archivio_dimostrativo() {
        let s = Store::open_in_memory().unwrap();
        crate::demo::populate(&s).unwrap();
        let inv = s
            .documents(&openfatt_store::DocFilter::default())
            .unwrap()
            .into_iter()
            .find(|d| d.kind == DocKind::Invoice && d.number.is_some())
            .expect("fattura dimostrativa");
        let doc = s.document(inv.id).unwrap();
        let c = for_document(&s, &doc).unwrap();
        assert!(c.subject.starts_with("Fattura n."), "{}", c.subject);
        assert_eq!(c.attachments.len(), 1);
        assert!(c.attachments[0].data.starts_with(b"%PDF"));
        assert!(!c.body.contains('{'), "{}", c.body);
        // un sollecito di ultimo livello per un cliente senza PEC va per email ordinaria
        let today = chrono::Local::now().date_naive();
        let list = s.reminder_candidates(today, 0, 7).unwrap();
        // nei dati dimostrativi: Rossi (con email) e Bianchi (senza indirizzo)
        assert_eq!(list.len(), 2, "{:?}", list.iter().map(|c| &c.party_name).collect::<Vec<_>>());
        let no_addr = list.iter().find(|c| c.email.is_empty()).unwrap();
        assert!(for_reminder(&s, no_addr, 1, false).note.contains("né email né PEC"));
        {
            let cand = list.iter().find(|c| !c.email.is_empty()).unwrap();
            let r = for_reminder(&s, cand, 3, true);
            assert_eq!(r.to, "acquisti@rossi.example");
            assert!(!r.via_pec);
            assert_eq!(r.level, 3);
            assert!(r.body.contains("Totale scaduto"));
            assert!(!r.attachments.is_empty());
            assert!(!r.body.contains('{'), "{}", r.body);
        }
    }
}
