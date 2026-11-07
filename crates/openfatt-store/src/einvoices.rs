//! Fatture elettroniche inviate e ricevute, con la cronologia delle notifiche SdI.

use chrono::Datelike;
use rusqlite::{params, OptionalExtension, Row};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};

use openfatt_core::model::{DocKind, DocStatus, Document};
use openfatt_fatturapa::notify::{Notification, NotificationKind};
use openfatt_fatturapa::FatturaElettronica;

use crate::{dec_from_sql, dec_to_sql, now_sql, Result, Store, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Out,
    In,
}

impl Direction {
    fn code(self) -> &'static str {
        match self {
            Direction::Out => "OUT",
            Direction::In => "IN",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EInvoiceStatus {
    ToSend,
    Sent,
    Delivered,
    Rejected,
    NotDelivered,
    Accepted,
    Refused,
    Expired,
    Undeliverable,
    Received,
    Registered,
}

impl EInvoiceStatus {
    pub fn code(self) -> &'static str {
        match self {
            EInvoiceStatus::ToSend => "DA_INVIARE",
            EInvoiceStatus::Sent => "INVIATA",
            EInvoiceStatus::Delivered => "CONSEGNATA",
            EInvoiceStatus::Rejected => "SCARTATA",
            EInvoiceStatus::NotDelivered => "MANCATA_CONSEGNA",
            EInvoiceStatus::Accepted => "ACCETTATA",
            EInvoiceStatus::Refused => "RIFIUTATA",
            EInvoiceStatus::Expired => "DECORRENZA_TERMINI",
            EInvoiceStatus::Undeliverable => "NON_RECAPITABILE",
            EInvoiceStatus::Received => "RICEVUTA",
            EInvoiceStatus::Registered => "REGISTRATA",
        }
    }
    pub fn from_code(s: &str) -> Self {
        match s {
            "INVIATA" => EInvoiceStatus::Sent,
            "CONSEGNATA" => EInvoiceStatus::Delivered,
            "SCARTATA" => EInvoiceStatus::Rejected,
            "MANCATA_CONSEGNA" => EInvoiceStatus::NotDelivered,
            "ACCETTATA" => EInvoiceStatus::Accepted,
            "RIFIUTATA" => EInvoiceStatus::Refused,
            "DECORRENZA_TERMINI" => EInvoiceStatus::Expired,
            "NON_RECAPITABILE" => EInvoiceStatus::Undeliverable,
            "RICEVUTA" => EInvoiceStatus::Received,
            "REGISTRATA" => EInvoiceStatus::Registered,
            _ => EInvoiceStatus::ToSend,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            EInvoiceStatus::ToSend => "Da inviare",
            EInvoiceStatus::Sent => "Inviata, in attesa di esito",
            EInvoiceStatus::Delivered => "Consegnata",
            EInvoiceStatus::Rejected => "Scartata",
            EInvoiceStatus::NotDelivered => "Non consegnata (nel cassetto fiscale)",
            EInvoiceStatus::Accepted => "Accettata dalla PA",
            EInvoiceStatus::Refused => "Rifiutata dalla PA",
            EInvoiceStatus::Expired => "Decorrenza termini",
            EInvoiceStatus::Undeliverable => "Trasmessa, non recapitabile",
            EInvoiceStatus::Received => "Ricevuta, da registrare",
            EInvoiceStatus::Registered => "Registrata",
        }
    }
    /// La fattura è considerata emessa ai fini fiscali.
    pub fn is_final_ok(self) -> bool {
        matches!(
            self,
            EInvoiceStatus::Delivered | EInvoiceStatus::NotDelivered | EInvoiceStatus::Accepted | EInvoiceStatus::Expired | EInvoiceStatus::Undeliverable
        )
    }
}

#[derive(Debug, Clone)]
pub struct EInvoice {
    pub id: i64,
    pub direction: Direction,
    pub document_id: Option<i64>,
    pub file_name: String,
    pub sdi_id: String,
    pub status: EInvoiceStatus,
    pub xml: Vec<u8>,
    pub original: Option<Vec<u8>>,
    pub hash: String,
    pub party_name: String,
    pub party_vat: String,
    pub doc_type: String,
    pub doc_number: String,
    pub doc_date: String,
    pub total: Decimal,
    pub created_at: String,
    pub sent_at: Option<String>,
    pub last_message: String,
}

#[derive(Debug, Clone)]
pub struct EInvoiceEvent {
    pub id: i64,
    pub kind: String,
    pub sdi_id: String,
    pub received_at: String,
    pub description: String,
    pub file_name: String,
}

const COLS: &str = "id, direction, document_id, file_name, sdi_id, status, xml, original, hash, party_name, party_vat, doc_type, doc_number, doc_date, total, created_at, sent_at, last_message";

fn row_to_einvoice(r: &Row) -> rusqlite::Result<EInvoice> {
    Ok(EInvoice {
        id: r.get(0)?,
        direction: if r.get::<_, String>(1)? == "IN" { Direction::In } else { Direction::Out },
        document_id: r.get(2)?,
        file_name: r.get(3)?,
        sdi_id: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        status: EInvoiceStatus::from_code(&r.get::<_, String>(5)?),
        xml: r.get(6)?,
        original: r.get(7)?,
        hash: r.get(8)?,
        party_name: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
        party_vat: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
        doc_type: r.get::<_, Option<String>>(11)?.unwrap_or_default(),
        doc_number: r.get::<_, Option<String>>(12)?.unwrap_or_default(),
        doc_date: r.get::<_, Option<String>>(13)?.unwrap_or_default(),
        total: dec_from_sql(r.get(14)?),
        created_at: r.get(15)?,
        sent_at: r.get(16)?,
        last_message: r.get::<_, Option<String>>(17)?.unwrap_or_default(),
    })
}

impl EInvoice {
    /// File da trasmettere: la versione firmata se presente, altrimenti l'XML.
    pub fn payload(&self) -> (&str, &[u8]) {
        match &self.original {
            Some(o) if self.direction == Direction::Out => (&self.file_name, o),
            _ => (&self.file_name, &self.xml),
        }
    }
    pub fn is_signed(&self) -> bool {
        self.original.is_some() && self.file_name.to_ascii_lowercase().ends_with(".p7m")
    }
    /// Fattura verso la Pubblica Amministrazione (formato FPA12).
    pub fn is_pa(&self) -> bool {
        let head = &self.xml[..self.xml.len().min(600)];
        String::from_utf8_lossy(head).contains("versione=\"FPA12\"")
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    let h = Sha256::digest(data);
    h.iter().map(|b| format!("{:02x}", b)).collect()
}

fn stem(name: &str) -> String {
    let n = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let n = n.strip_suffix(".p7m").or_else(|| n.strip_suffix(".P7M")).unwrap_or(n);
    let n = n.strip_suffix(".xml").or_else(|| n.strip_suffix(".XML")).unwrap_or(n);
    n.to_ascii_uppercase()
}

impl Store {
    /// Registra la fattura elettronica generata per un documento (sostituisce una
    /// precedente versione non ancora inviata).
    pub fn create_outgoing_einvoice(&self, doc: &Document, file_name: &str, xml: &str, fattura: &FatturaElettronica) -> Result<EInvoice> {
        if doc.status != DocStatus::Issued {
            return Err(StoreError::Invalid("Il documento deve essere emesso".into()));
        }
        let tx = self.begin()?;
        self.conn().execute("DELETE FROM einvoices WHERE document_id = ?1 AND direction = 'OUT' AND status = 'DA_INVIARE'", [doc.id])?;
        let active: Option<String> = self
            .conn()
            .query_row(
                "SELECT status FROM einvoices WHERE document_id = ?1 AND direction = 'OUT' AND status NOT IN ('SCARTATA') LIMIT 1",
                [doc.id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(s) = active {
            return Err(StoreError::Invalid(format!("Per questo documento esiste già una fattura elettronica in stato {}", s)));
        }
        let cess = &fattura.header.cessionario;
        let body = &fattura.bodies[0];
        let total = openfatt_fatturapa::to_domain::declared_total(body);
        self.conn().execute(
            "INSERT INTO einvoices(direction, document_id, file_name, status, xml, hash, party_name, party_vat, doc_type, doc_number, doc_date, total, created_at)
             VALUES ('OUT', ?1, ?2, 'DA_INVIARE', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                doc.id,
                file_name,
                xml.as_bytes(),
                sha256_hex(xml.as_bytes()),
                cess.anagrafica.display(),
                cess.id_fiscale_iva.as_ref().map(|i| format!("{}{}", i.paese, i.codice)).or(cess.codice_fiscale.clone()).unwrap_or_default(),
                body.dati_generali.documento.tipo_documento,
                body.dati_generali.documento.numero,
                body.dati_generali.documento.data.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default(),
                dec_to_sql(total),
                now_sql()
            ],
        )?;
        let id = self.conn().last_insert_rowid();
        self.audit("einvoice", id, "create", file_name)?;
        tx.commit()?;
        self.einvoice(id)
    }

    pub fn einvoice(&self, id: i64) -> Result<EInvoice> {
        self.conn()
            .query_row(&format!("SELECT {} FROM einvoices WHERE id = ?1", COLS), [id], row_to_einvoice)
            .optional()?
            .ok_or_else(|| StoreError::NotFound("fattura elettronica".into()))
    }

    pub fn einvoices(&self, dir: Direction, search: &str, year: Option<i32>) -> Result<Vec<EInvoice>> {
        let like = format!("%{}%", search.trim());
        let mut st = self.conn().prepare(&format!(
            "SELECT {} FROM einvoices WHERE direction = ?1
             AND (?2 = '%%' OR party_name LIKE ?2 OR file_name LIKE ?2 OR doc_number LIKE ?2 OR party_vat LIKE ?2)
             AND (?3 IS NULL OR substr(doc_date, 1, 4) = ?3)
             ORDER BY doc_date DESC, id DESC",
            COLS
        ))?;
        let rows = st.query_map(params![dir.code(), like, year.map(|y| y.to_string())], row_to_einvoice)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Numero di fatture elettroniche per stato, senza leggere i file (per i contatori dei menu).
    pub fn einvoice_count(&self, dir: Direction, status: EInvoiceStatus) -> Result<usize> {
        Ok(self.conn().query_row("SELECT COUNT(*) FROM einvoices WHERE direction = ?1 AND status = ?2", params![dir.code(), status.code()], |r| r.get::<_, i64>(0))? as usize)
    }

    pub fn einvoice_for_document(&self, doc_id: i64) -> Result<Option<EInvoice>> {
        let id: Option<i64> = self
            .conn()
            .query_row("SELECT id FROM einvoices WHERE document_id = ?1 AND direction = 'OUT' ORDER BY id DESC LIMIT 1", [doc_id], |r| r.get(0))
            .optional()?;
        match id {
            Some(id) => Ok(Some(self.einvoice(id)?)),
            None => Ok(None),
        }
    }

    pub fn einvoice_events(&self, id: i64) -> Result<Vec<EInvoiceEvent>> {
        let mut st = self.conn().prepare("SELECT id, kind, sdi_id, received_at, description, file_name FROM einvoice_events WHERE einvoice_id = ?1 ORDER BY id")?;
        let rows = st.query_map([id], |r| {
            Ok(EInvoiceEvent {
                id: r.get(0)?,
                kind: r.get(1)?,
                sdi_id: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                received_at: r.get(3)?,
                description: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                file_name: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn mark_einvoice_sent(&self, id: i64, channel_note: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE einvoices SET status = 'INVIATA', sent_at = ?1, updated_at = ?1, last_message = ?2 WHERE id = ?3 AND status IN ('DA_INVIARE','INVIATA')",
            params![now_sql(), channel_note, id],
        )?;
        self.audit("einvoice", id, "sent", channel_note)?;
        Ok(())
    }

    /// Applica una notifica SdI. Restituisce l'id della fattura aggiornata, oppure None
    /// se la notifica era già stata acquisita o non corrisponde a nessuna fattura.
    pub fn apply_notification(&self, notification_file: &str, n: &Notification, raw: &[u8]) -> Result<Option<i64>> {
        let already: Option<i64> = self.conn().query_row("SELECT id FROM einvoice_events WHERE file_name = ?1", [notification_file], |r| r.get(0)).optional()?;
        if already.is_some() {
            return Ok(None);
        }
        let target_stem = stem(&n.file_name);
        let mut st = self.conn().prepare("SELECT id, file_name FROM einvoices WHERE direction = 'OUT'")?;
        let candidates: Vec<(i64, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|r| r.ok()).collect();
        let Some((inv_id, _)) = candidates.into_iter().find(|(_, f)| stem(f) == target_stem) else {
            return Ok(None);
        };
        let new_status = match n.kind {
            NotificationKind::Delivered => Some(EInvoiceStatus::Delivered),
            NotificationKind::Rejected => Some(EInvoiceStatus::Rejected),
            NotificationKind::NotDelivered => Some(EInvoiceStatus::NotDelivered),
            NotificationKind::Outcome => match n.outcome.as_deref() {
                Some("EC01") => Some(EInvoiceStatus::Accepted),
                Some("EC02") => Some(EInvoiceStatus::Refused),
                _ => None,
            },
            NotificationKind::Expired => Some(EInvoiceStatus::Expired),
            NotificationKind::TransmittedUndeliverable => Some(EInvoiceStatus::Undeliverable),
            _ => None,
        };
        let tx = self.begin()?;
        self.conn().execute(
            "INSERT INTO einvoice_events(einvoice_id, file_name, kind, sdi_id, received_at, description, raw) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![inv_id, notification_file, n.kind.code(), n.sdi_id, now_sql(), n.summary(), raw],
        )?;
        if let Some(s) = new_status {
            // gli esiti finali della PA non vengono sovrascritti da notifiche arrivate fuori ordine
            let current = EInvoiceStatus::from_code(
                &self.conn().query_row("SELECT status FROM einvoices WHERE id = ?1", [inv_id], |r| r.get::<_, String>(0))?,
            );
            let keep = matches!(current, EInvoiceStatus::Accepted | EInvoiceStatus::Refused | EInvoiceStatus::Expired)
                && matches!(s, EInvoiceStatus::Delivered | EInvoiceStatus::NotDelivered);
            if !keep {
                self.conn().execute(
                    "UPDATE einvoices SET status = ?1, sdi_id = COALESCE(NULLIF(?2, ''), sdi_id), updated_at = ?3, last_message = ?4 WHERE id = ?5",
                    params![s.code(), n.sdi_id, now_sql(), n.summary(), inv_id],
                )?;
            }
        }
        self.audit("einvoice", inv_id, "notification", n.kind.code())?;
        tx.commit()?;
        Ok(Some(inv_id))
    }

    /// Acquisisce una fattura ricevuta. Restituisce None se già presente (stesso file o contenuto).
    pub fn import_incoming(&self, file_name: &str, xml: &[u8], original: Option<&[u8]>, f: &FatturaElettronica, sdi_id: &str) -> Result<Option<i64>> {
        let hash = sha256_hex(xml);
        let dup: Option<i64> = self
            .conn()
            .query_row("SELECT id FROM einvoices WHERE direction = 'IN' AND (file_name = ?1 OR hash = ?2)", params![file_name, hash], |r| r.get(0))
            .optional()?;
        if dup.is_some() {
            return Ok(None);
        }
        let ced = &f.header.cedente;
        let body = f.bodies.first().ok_or_else(|| StoreError::Invalid("fattura senza corpo".into()))?;
        let d = &body.dati_generali.documento;
        self.conn().execute(
            "INSERT INTO einvoices(direction, file_name, sdi_id, status, xml, original, hash, party_name, party_vat, doc_type, doc_number, doc_date, total, created_at)
             VALUES ('IN', ?1, ?2, 'RICEVUTA', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                file_name,
                sdi_id,
                xml,
                original,
                hash,
                ced.anagrafica.display(),
                format!("{}{}", ced.id_fiscale_iva.paese, ced.id_fiscale_iva.codice),
                d.tipo_documento,
                d.numero,
                d.data.map(|x| x.format("%Y-%m-%d").to_string()).unwrap_or_default(),
                dec_to_sql(openfatt_fatturapa::to_domain::declared_total(body)),
                now_sql()
            ],
        )?;
        let id = self.conn().last_insert_rowid();
        self.audit("einvoice", id, "import", file_name)?;
        Ok(Some(id))
    }

    /// Registra una fattura ricevuta come documento di acquisto (crea il fornitore se nuovo).
    pub fn register_incoming(&self, einvoice_id: i64) -> Result<Vec<i64>> {
        let e = self.einvoice(einvoice_id)?;
        if e.direction != Direction::In {
            return Err(StoreError::Invalid("Non è una fattura ricevuta".into()));
        }
        if e.status == EInvoiceStatus::Registered {
            return Err(StoreError::Invalid("Fattura già registrata".into()));
        }
        let f = openfatt_fatturapa::parse_bytes(&e.xml).map_err(|x| StoreError::Invalid(x.0))?;
        let vats = self.vat_codes()?;
        let tx = self.begin()?;
        let supplier_tpl = openfatt_fatturapa::to_domain::supplier_from(&f.header.cedente);
        let supplier = match self.find_party_by_tax_id(&supplier_tpl.vat_number, &supplier_tpl.tax_code)? {
            Some(mut p) => {
                if !p.role.is_supplier() {
                    p.role = openfatt_core::model::PartyRole::Both;
                    p = self.save_party(&p)?;
                }
                p
            }
            None => self.save_party(&supplier_tpl)?,
        };
        let mut ids = Vec::new();
        for bi in 0..f.bodies.len() {
            let Some(mut doc) = openfatt_fatturapa::to_domain::purchase_document(&f, bi) else { continue };
            doc.party_id = Some(supplier.id);
            doc.party = supplier.clone();
            for l in doc.lines.iter_mut() {
                if let Some(v) = vats.iter().find(|v| v.rate == l.vat.rate && v.natura == l.vat.natura) {
                    l.vat.vat_id = Some(v.id);
                    l.vat.legal_ref = v.legal_ref.clone();
                }
                if !l.code.is_empty() {
                    if let Some(p) = self.product_by_code(&l.code)? {
                        // articoli con varianti o seriali: il collegamento richiede dati che la fattura non porta
                        if p.variants.is_empty() && p.tracking != openfatt_core::model::Tracking::Serial {
                            l.product_id = Some(p.id);
                        }
                    }
                }
            }
            let dues = std::mem::take(&mut doc.due_dates);
            doc.status = DocStatus::Draft;
            let saved = self.save_document(&doc)?;
            // numero di protocollo interno
            let mut issued = saved.clone();
            issued.number = Some(self.next_number(issued.kind, "", issued.date.year())?);
            issued.status = DocStatus::Issued;
            let issued = self.save_document(&issued)?;
            // scadenze come indicate dal fornitore, se presenti
            if !dues.is_empty() {
                self.conn().execute("DELETE FROM due_dates WHERE document_id = ?1", [issued.id])?;
                for d in dues {
                    self.conn().execute(
                        "INSERT INTO due_dates(document_id, date, amount, mp_code, paid) VALUES (?1, ?2, ?3, ?4, '0')",
                        params![issued.id, d.date.format("%Y-%m-%d").to_string(), dec_to_sql(d.amount), d.mp_code],
                    )?;
                }
            }
            ids.push(issued.id);
        }
        self.conn().execute(
            "UPDATE einvoices SET status = 'REGISTRATA', document_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![ids.first(), now_sql(), einvoice_id],
        )?;
        self.audit("einvoice", einvoice_id, "register", &format!("{:?}", ids))?;
        tx.commit()?;
        Ok(ids)
    }

    /// Associa la versione firmata (.p7m) a una fattura da inviare. Il contenuto firmato
    /// deve coincidere byte per byte con l'XML generato.
    pub fn attach_signed_file(&self, id: i64, p7m: &[u8]) -> Result<EInvoice> {
        let e = self.einvoice(id)?;
        if e.direction != Direction::Out || e.status != EInvoiceStatus::ToSend {
            return Err(StoreError::Invalid("Si può firmare solo una fattura emessa non ancora inviata".into()));
        }
        let content = openfatt_fatturapa::p7m::extract(p7m).map_err(|x| StoreError::Invalid(format!("File firmato non leggibile: {}", x)))?;
        if content != e.xml {
            return Err(StoreError::Invalid(
                "Il file firmato non contiene esattamente questa fattura: firmare il file XML salvato da questa schermata senza modificarlo".into(),
            ));
        }
        let base = e.file_name.trim_end_matches(".p7m").to_string();
        let name = format!("{}.p7m", base);
        self.conn().execute(
            "UPDATE einvoices SET original = ?1, file_name = ?2, updated_at = ?3, last_message = 'Firmata digitalmente (CAdES)' WHERE id = ?4",
            params![p7m, name, now_sql(), id],
        )?;
        self.audit("einvoice", id, "signed", &name)?;
        self.einvoice(id)
    }

    /// Rimuove la firma (prima dell'invio), tornando all'XML semplice.
    pub fn remove_signature(&self, id: i64) -> Result<()> {
        let e = self.einvoice(id)?;
        if e.status != EInvoiceStatus::ToSend {
            return Err(StoreError::Invalid("La fattura è già stata inviata".into()));
        }
        let name = e.file_name.trim_end_matches(".p7m").to_string();
        self.conn().execute("UPDATE einvoices SET original = NULL, file_name = ?1, last_message = '' WHERE id = ?2", params![name, id])?;
        self.audit("einvoice", id, "unsigned", &name)?;
        Ok(())
    }

    pub fn delete_einvoice(&self, id: i64) -> Result<()> {
        let e = self.einvoice(id)?;
        if e.direction == Direction::Out && !matches!(e.status, EInvoiceStatus::ToSend | EInvoiceStatus::Rejected) {
            return Err(StoreError::Invalid("Una fattura trasmessa allo SdI non può essere eliminata".into()));
        }
        self.conn().execute("DELETE FROM einvoices WHERE id = ?1", [id])?;
        self.audit("einvoice", id, "delete", &e.file_name)?;
        Ok(())
    }

    /// Messaggi email già elaborati (per non rielaborarli).
    pub fn mail_already_seen(&self, message_id: &str) -> Result<bool> {
        let n: i64 = self.conn().query_row("SELECT COUNT(*) FROM mail_seen WHERE message_id = ?1", [message_id], |r| r.get(0))?;
        Ok(n > 0)
    }

    pub fn mark_mail_seen(&self, message_id: &str, outcome: &str) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO mail_seen(message_id, seen_at, outcome) VALUES (?1, ?2, ?3)",
            params![message_id, now_sql(), outcome],
        )?;
        Ok(())
    }

    /// Fatture emesse non ancora trasmesse (per promemoria all'avvio).
    pub fn documents_without_einvoice(&self) -> Result<Vec<i64>> {
        let kinds: Vec<String> = DocKind::ALL.iter().filter(|k| k.is_fiscal()).map(|k| format!("'{}'", k.code())).collect();
        let mut st = self.conn().prepare(&format!(
            "SELECT d.id FROM documents d WHERE d.status = 'EMESSO' AND d.kind IN ({}) AND COALESCE(json_extract(d.data, '$.legacy_fe'), 0) = 0
             AND NOT EXISTS (SELECT 1 FROM einvoices e WHERE e.document_id = d.id AND e.direction = 'OUT' AND e.status <> 'DA_INVIARE')
             ORDER BY d.date",
            kinds.join(",")
        ))?;
        let rows = st.query_map([], |r| r.get::<_, i64>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}
