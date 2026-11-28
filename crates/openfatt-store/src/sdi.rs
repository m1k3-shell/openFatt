//! Fatture elettroniche da gestire: per ogni esito dello SdI che richiede un'azione, cosa
//! fare ed entro quando.
//!
//! - Da inviare: la fattura immediata va trasmessa entro 12 giorni dall'operazione (art. 21
//!   c. 4 DPR 633/72).
//! - Inviata senza esito da più di 5 giorni: lo SdI risponde entro 5 giorni (Specifiche
//!   tecniche SdI), quindi va controllata la casella PEC.
//! - Scartata (NS): la fattura si considera non emessa; va corretta e reinviata entro 5 giorni
//!   dallo scarto con lo stesso numero e la stessa data (circolare AdE 13/E del 2/7/2018).
//! - Non consegnata (MC) a un privato: è nell'area riservata del cliente; gli si manda una
//!   copia di cortesia dicendoglielo. Per la PA lo SdI ritenta per 10 giorni.
//! - Consegnata alla PA: la PA ha 15 giorni per accettare o rifiutare, poi decorrenza termini
//!   (Specifiche tecniche SdI 1.8.4).
//! - Rifiutata dalla PA (EC02): la fattura resta emessa; si storna con una nota di credito e si
//!   riemette corretta.

use chrono::NaiveDate;
use rusqlite::params;

use crate::{date_from_sql, now_sql, EInvoice, EInvoiceStatus, Result, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdiAction {
    Send,
    CheckMailbox,
    Resend,
    CourtesyCopy,
    WaitPa,
    CreditNote,
}

impl SdiAction {
    pub fn label(self) -> &'static str {
        match self {
            SdiAction::Send => "Inviare",
            SdiAction::CheckMailbox => "Controllare la PEC",
            SdiAction::Resend => "Correggere e reinviare",
            SdiAction::CourtesyCopy => "Copia di cortesia al cliente",
            SdiAction::WaitPa => "In attesa della PA",
            SdiAction::CreditNote => "Stornare con nota di credito",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SdiTask {
    pub einvoice_id: i64,
    pub document_id: Option<i64>,
    pub file_name: String,
    pub party_name: String,
    pub doc_number: String,
    pub doc_date: String,
    pub status: EInvoiceStatus,
    /// Da quando (invio, notifica).
    pub since: NaiveDate,
    pub deadline: Option<NaiveDate>,
    pub action: SdiAction,
    pub detail: String,
}

impl SdiTask {
    pub fn overdue(&self, today: NaiveDate) -> bool {
        self.deadline.map(|d| d < today).unwrap_or(false)
    }
    /// Richiede un intervento (non è solo un'attesa).
    pub fn needs_action(&self) -> bool {
        self.action != SdiAction::WaitPa
    }
}

const COURTESY: &str = "COPIA";

impl Store {
    fn last_event_date(&self, id: i64, kinds: &[&str]) -> Result<Option<NaiveDate>> {
        let marks: Vec<String> = kinds.iter().map(|k| format!("'{}'", k)).collect();
        let d: Option<String> = self
            .conn()
            .query_row(&format!("SELECT MAX(received_at) FROM einvoice_events WHERE einvoice_id = ?1 AND kind IN ({})", marks.join(",")), [id], |r| r.get(0))
            .unwrap_or(None);
        Ok(d.map(|s| date_from_sql(&s)))
    }

    /// Registra che la copia di cortesia è stata preparata per il cliente.
    pub fn mark_courtesy_copy(&self, einvoice_id: i64) -> Result<()> {
        self.conn().execute(
            "INSERT INTO einvoice_events(einvoice_id, file_name, kind, sdi_id, received_at, description) VALUES (?1, ?2, ?3, '', ?4, ?5)",
            params![einvoice_id, format!("copia-cortesia-{}-{}", einvoice_id, chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)), COURTESY, now_sql(), "Copia di cortesia preparata per il cliente"],
        )?;
        self.audit("einvoice", einvoice_id, "courtesy_copy", "")?;
        Ok(())
    }

    /// La fattura è già stata stornata da una nota di credito emessa che la cita.
    fn credited(&self, e: &EInvoice) -> Result<bool> {
        let Some(doc_id) = e.document_id else { return Ok(false) };
        let d = self.document(doc_id)?;
        let number = d.number_label();
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM documents d, json_each(d.data, '$.related') r
             WHERE d.kind = 'NDC' AND d.status <> 'ANNULLATO' AND json_extract(r.value, '$.kind') = 'FATTURA' AND json_extract(r.value, '$.number') = ?1 AND json_extract(r.value, '$.date') = ?2",
            params![number, d.date.format("%Y-%m-%d").to_string()],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Fatture elettroniche inviate che richiedono un'azione o sono in attesa di un esito.
    pub fn sdi_tasks(&self, today: NaiveDate) -> Result<Vec<SdiTask>> {
        let mut out = Vec::new();
        let list = self.einvoices(crate::Direction::Out, "", None)?;
        for e in list {
            let created = date_from_sql(&e.created_at);
            let sent = e.sent_at.as_deref().map(date_from_sql).unwrap_or(created);
            let doc_date = NaiveDate::parse_from_str(e.doc_date.get(..10).unwrap_or(""), "%Y-%m-%d").unwrap_or(created);
            let base = |since: NaiveDate, deadline: Option<NaiveDate>, action: SdiAction, detail: String| SdiTask {
                einvoice_id: e.id,
                document_id: e.document_id,
                file_name: e.file_name.clone(),
                party_name: e.party_name.clone(),
                doc_number: e.doc_number.clone(),
                doc_date: e.doc_date.clone(),
                status: e.status,
                since,
                deadline,
                action,
                detail,
            };
            let task = match e.status {
                EInvoiceStatus::ToSend if e.doc_type == "TD24" || e.doc_type == "TD25" => {
                    use chrono::Datelike;
                    let (y, m) = if doc_date.month() == 12 { (doc_date.year() + 1, 1) } else { (doc_date.year(), doc_date.month() + 1) };
                    Some(base(created, NaiveDate::from_ymd_opt(y, m, 15), SdiAction::Send, "Fattura differita: da trasmettere entro il 15 del mese successivo".into()))
                }
                EInvoiceStatus::ToSend => Some(base(created, Some(doc_date + chrono::Duration::days(12)), SdiAction::Send, "Fattura immediata: da trasmettere entro 12 giorni dalla data dell'operazione".into())),
                EInvoiceStatus::Sent if (today - sent).num_days() > 5 => Some(base(sent, None, SdiAction::CheckMailbox, format!("Inviata il {} e nessun esito dopo {} giorni: scaricare la posta PEC o controllare la ricevuta di consegna", sent.format("%d/%m/%Y"), (today - sent).num_days()))),
                EInvoiceStatus::Rejected => {
                    let since = self.last_event_date(e.id, &["NS"])?.unwrap_or(sent);
                    Some(base(since, Some(since + chrono::Duration::days(5)), SdiAction::Resend, format!("Scartata: la fattura non è emessa. Correggere e reinviare con lo stesso numero e la stessa data entro 5 giorni. {}", e.last_message)))
                }
                EInvoiceStatus::NotDelivered if e.is_pa() => {
                    let since = self.last_event_date(e.id, &["MC"])?.unwrap_or(sent);
                    Some(base(since, None, SdiAction::WaitPa, "Non consegnata alla PA: lo SdI ritenta per 10 giorni, poi comunica l'impossibilità di recapito".into()))
                }
                EInvoiceStatus::NotDelivered => {
                    if self.last_event_date(e.id, &[COURTESY])?.is_some() {
                        None
                    } else {
                        let since = self.last_event_date(e.id, &["MC"])?.unwrap_or(sent);
                        Some(base(since, None, SdiAction::CourtesyCopy, "Non recapitata: la fattura è emessa ed è nell'area riservata del cliente (Fatture e Corrispettivi). Mandargli una copia di cortesia dicendoglielo".into()))
                    }
                }
                EInvoiceStatus::Delivered if e.is_pa() => {
                    let since = self.last_event_date(e.id, &["RC"])?.unwrap_or(sent);
                    Some(base(since, Some(since + chrono::Duration::days(15)), SdiAction::WaitPa, "Consegnata alla PA: 15 giorni per accettarla o rifiutarla, poi decorrenza termini".into()))
                }
                EInvoiceStatus::Refused => {
                    if self.credited(&e)? {
                        None
                    } else {
                        let since = self.last_event_date(e.id, &["NE", "EC"])?.unwrap_or(sent);
                        Some(base(since, None, SdiAction::CreditNote, format!("Rifiutata dalla PA: la fattura resta emessa. Stornarla con una nota di credito e riemetterla corretta. {}", e.last_message)))
                    }
                }
                _ => None,
            };
            if let Some(t) = task {
                out.push(t);
            }
        }
        out.sort_by_key(|t| (t.action == SdiAction::WaitPa, t.deadline.unwrap_or(NaiveDate::MAX), t.since));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    //! Prova virtuale del ciclo SdI: fatture generate e "inviate", notifiche costruite dagli
    //! esempi ufficiali delle Specifiche (specs/notifiche) con il nome del nostro file.
    use super::*;
    use openfatt_core::dec;
    use openfatt_core::model::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    fn sample(kind: &str, file: &str) -> String {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../specs/notifiche/IT01234567890_11111_{}_001.xml", kind));
        std::fs::read_to_string(p).expect("esempi ufficiali delle notifiche in specs/notifiche").replace("IT01234567890_11111.xml.p7m", file)
    }

    fn notify(s: &Store, kind: &str, file: &str, n: u32, edit: impl Fn(String) -> String) {
        let xml = edit(sample(kind, file));
        let parsed = openfatt_fatturapa::notify::parse_notification(&xml).unwrap();
        let stem = file.trim_end_matches(".xml");
        assert!(s.apply_notification(&format!("{}_{}_{:03}.xml", stem, kind, n), &parsed, xml.as_bytes()).unwrap().is_some(), "{} non applicata", kind);
    }

    struct Env {
        s: Store,
        company: Company,
        v: VatRef,
        b2b: Party,
        pa: Party,
    }

    fn env() -> Env {
        let s = Store::open_in_memory().unwrap();
        let company = Company { name: "Azienda Srl".into(), vat_country: "IT".into(), vat_number: "06363391001".into(), regime_fiscale: "RF01".into(), address: "Via X".into(), civic: "1".into(), zip: "00100".into(), city: "Roma".into(), province: "RM".into(), country: "IT".into(), ..Default::default() };
        s.set_company(&company).unwrap();
        let v = VatRef::from_code(&s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap());
        let b2b = s.save_party(&Party { name: "Cliente Spa".into(), vat_number: "12345678903".into(), address: "Via Y".into(), zip: "38122".into(), city: "Trento".into(), province: "TN".into(), sdi_code: "ABCDEFG".into(), ..Default::default() }).unwrap();
        let pa = s.save_party(&Party { name: "Comune di Prova".into(), tax_code: "00514490010".into(), address: "Piazza Z".into(), zip: "38122".into(), city: "Trento".into(), province: "TN".into(), sdi_code: "UFABCD".into(), is_pa: true, ..Default::default() }).unwrap();
        Env { s, company, v, b2b, pa }
    }

    fn send(e: &Env, p: &Party, n: &str) -> (Document, EInvoice) {
        let mut doc = Document::new(DocKind::Invoice);
        doc.date = chrono::Local::now().date_naive();
        doc.party_id = Some(p.id);
        if p.is_pa {
            doc.vat_liability = "S".into();
        }
        doc.lines.push(DocLine { description: "Servizio".into(), qty: dec!(1), unit_price: dec!(100), vat: e.v.clone(), ..Default::default() });
        let doc = e.s.issue_document(e.s.save_document(&doc).unwrap().id).unwrap();
        let t = openfatt_core::calc::compute(&doc).unwrap();
        let g = openfatt_fatturapa::generate(&e.company, &doc, &t, &openfatt_fatturapa::BuildOptions { progressivo: n.into(), ..Default::default() }).unwrap();
        assert!(g.is_sendable(), "{:?}", g.issues);
        let file = format!("IT06363391001_{}.xml", n);
        let inv = e.s.create_outgoing_einvoice(&doc, &file, &g.xml, &g.fattura).unwrap();
        e.s.mark_einvoice_sent(inv.id, "PEC").unwrap();
        (doc, e.s.einvoice(inv.id).unwrap())
    }

    fn task(s: &Store, id: i64, today: NaiveDate) -> Option<SdiTask> {
        s.sdi_tasks(today).unwrap().into_iter().find(|t| t.einvoice_id == id)
    }

    #[test]
    fn ciclo_sdi_simulato_scarto_mancata_consegna_e_rifiuto_pa() {
        let e = env();
        let today = chrono::Local::now().date_naive();

        // 1. scarto: correggere e reinviare entro 5 giorni con stesso numero e data
        let (doc, inv) = send(&e, &e.b2b, "00001");
        assert!(task(&e.s, inv.id, today).is_none(), "appena inviata non è un'attività");
        notify(&e.s, "NS", &inv.file_name, 1, |x| x.replace("<Codice>00100</Codice>", "<Codice>00305</Codice>").replace("Certificato di firma scaduto", "IdFiscaleIVA del cessionario non valido"));
        let t = task(&e.s, inv.id, today).unwrap();
        assert_eq!(t.status, EInvoiceStatus::Rejected);
        assert_eq!(t.action, SdiAction::Resend);
        assert_eq!(t.deadline, Some(today + chrono::Duration::days(5)));
        assert!(t.detail.contains("00305"), "{}", t.detail);
        assert!(t.overdue(today + chrono::Duration::days(6)));
        // correzione: si rigenera con un nuovo nome di file, stesso numero e data del documento
        e.s.delete_einvoice(inv.id).unwrap();
        let t = openfatt_core::calc::compute(&doc).unwrap();
        let g = openfatt_fatturapa::generate(&e.company, &doc, &t, &openfatt_fatturapa::BuildOptions { progressivo: "00002".into(), ..Default::default() }).unwrap();
        let again = e.s.create_outgoing_einvoice(&doc, "IT06363391001_00002.xml", &g.xml, &g.fattura).unwrap();
        assert_eq!(task(&e.s, again.id, today).unwrap().action, SdiAction::Send);
        e.s.mark_einvoice_sent(again.id, "PEC").unwrap();
        notify(&e.s, "RC", &again.file_name, 1, |x| x);
        assert_eq!(e.s.einvoice(again.id).unwrap().status, EInvoiceStatus::Delivered);
        assert!(task(&e.s, again.id, today).is_none());

        // 2. mancata consegna a un privato: copia di cortesia, poi più nulla da fare
        let (_, mc) = send(&e, &e.b2b, "00003");
        notify(&e.s, "MC", &mc.file_name, 1, |x| x);
        let t = task(&e.s, mc.id, today).unwrap();
        assert_eq!(t.action, SdiAction::CourtesyCopy);
        assert!(e.s.einvoice(mc.id).unwrap().status.is_final_ok(), "con la MC la fattura è emessa");
        e.s.mark_courtesy_copy(mc.id).unwrap();
        assert!(task(&e.s, mc.id, today).is_none());

        // 3. PA: consegnata, 15 giorni per l'esito; rifiutata: nota di credito che la storna
        let (pa_doc, pa) = send(&e, &e.pa, "00004");
        assert!(e.s.einvoice(pa.id).unwrap().is_pa());
        notify(&e.s, "RC", &pa.file_name, 1, |x| x);
        let t = task(&e.s, pa.id, today).unwrap();
        assert_eq!(t.action, SdiAction::WaitPa);
        assert_eq!(t.deadline, Some(today + chrono::Duration::days(15)));
        assert!(!t.needs_action());
        notify(&e.s, "NE", &pa.file_name, 1, |x| x.replace("<Esito>EC01</Esito>", "<Esito>EC02</Esito>").replace("Notifica di esempio", "CIG mancante"));
        assert_eq!(e.s.einvoice(pa.id).unwrap().status, EInvoiceStatus::Refused);
        let t = task(&e.s, pa.id, today).unwrap();
        assert_eq!(t.action, SdiAction::CreditNote);
        // nota di credito che cita la fattura rifiutata
        let mut nc = Document::new(DocKind::CreditNote);
        nc.date = today;
        nc.party_id = Some(e.pa.id);
        nc.vat_liability = "S".into();
        nc.related.push(RelatedDoc { kind: "FATTURA".into(), number: pa_doc.number_label(), date: Some(pa_doc.date), ..Default::default() });
        nc.lines.push(DocLine { description: "Storno".into(), qty: dec!(1), unit_price: dec!(100), vat: e.v.clone(), ..Default::default() });
        e.s.issue_document(e.s.save_document(&nc).unwrap().id).unwrap();
        assert!(task(&e.s, pa.id, today).is_none());

        // 4. nessun esito dopo 5 giorni: controllare la PEC
        let (_, quiet) = send(&e, &e.b2b, "00005");
        assert!(task(&e.s, quiet.id, today).is_none());
        let t = task(&e.s, quiet.id, today + chrono::Duration::days(6)).unwrap();
        assert_eq!(t.action, SdiAction::CheckMailbox);

        // conservazione: pacchetto con le fatture consegnate e le ricevute, impronte verificabili
        e.s.set_conservation_settings(&crate::ConservationSettings { conservator: "Agenzia delle Entrate".into(), responsible: "Mario Rossi".into(), ..Default::default() }).unwrap();
        let year = chrono::Datelike::year(&today);
        let pkg = e.s.conservation_package(year).unwrap();
        // consegnate: la riemessa dopo lo scarto, la non recapitata (MC) e la PA rifiutata no
        assert_eq!(pkg.count, 2, "{}", pkg.count);
        let check = crate::verify_conservation_package(&pkg.zip).unwrap();
        assert!(check.problems.is_empty(), "{:?}", check.problems);
        assert!(check.files >= 4, "fatture e ricevute: {}", check.files);
        assert_eq!(check.ok, check.files);
        // un file alterato nel pacchetto viene scoperto
        let tampered = {
            use std::io::{Read, Write};
            let mut src = zip::ZipArchive::new(std::io::Cursor::new(pkg.zip.clone())).unwrap();
            let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            for i in 0..src.len() {
                let mut f = src.by_index(i).unwrap();
                let name = f.name().unwrap().to_string();
                let mut data = Vec::new();
                f.read_to_end(&mut data).unwrap();
                if name.starts_with("emesse/") && name.ends_with(".xml") && !name.contains("ricevute_sdi") {
                    data.extend_from_slice(b"<!-- -->");
                }
                out.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
                out.write_all(&data).unwrap();
            }
            out.finish().unwrap().into_inner()
        };
        let bad = crate::verify_conservation_package(&tampered).unwrap();
        assert_eq!(bad.problems.len(), 2, "{:?}", bad.problems);
        assert!(bad.problems.iter().all(|p| p.ends_with("impronta diversa")));
        let manual = String::from_utf8(zip::ZipArchive::new(std::io::Cursor::new(pkg.zip.clone())).unwrap().by_name("manuale_conservazione_bozza.txt").map(|mut f| { let mut s = String::new(); std::io::Read::read_to_string(&mut f, &mut s).unwrap(); s.into_bytes() }).unwrap()).unwrap();
        assert!(manual.contains("Responsabile della conservazione: Mario Rossi"));

        // le attività che chiedono un intervento vengono prima delle attese
        let all = e.s.sdi_tasks(today + chrono::Duration::days(6)).unwrap();
        assert!(all.first().map(|t| t.needs_action()).unwrap_or(false));
        let _ = d(2026, 1, 1);
    }
}
