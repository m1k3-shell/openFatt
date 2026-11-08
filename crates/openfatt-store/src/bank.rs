//! Distinte bancarie: Ri.Ba. e addebiti SDD presentati in banca, bonifici disposti.
//!
//! Una scadenza presentata in una distinta valida non viene riproposta; torna da presentare
//! se la distinta viene annullata o se la ricevuta risulta insoluta.

use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{date_from_sql, date_to_sql, dec_from_sql, dec_to_sql, now_sql, DueRow, Result, Store, StoreError};

/// Codici dell'azienda per i file bancari (li comunica la banca).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BankSettings {
    /// Codice SIA (Ri.Ba.).
    pub sia: String,
    /// Codice CUC (bonifici e SDD in formato CBI).
    pub cuc: String,
    /// Identificativo del creditore SEPA (SDD).
    pub creditor_id: String,
    /// Conto di prima nota su cui registrare incassi e pagamenti delle distinte.
    pub account: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BankKind {
    Riba,
    Sdd,
    Sct,
}

impl BankKind {
    pub const ALL: [BankKind; 3] = [BankKind::Riba, BankKind::Sdd, BankKind::Sct];

    pub fn code(self) -> &'static str {
        match self {
            BankKind::Riba => "RIBA",
            BankKind::Sdd => "SDD",
            BankKind::Sct => "SCT",
        }
    }
    pub fn from_code(s: &str) -> Option<BankKind> {
        BankKind::ALL.into_iter().find(|k| k.code() == s)
    }
    pub fn label(self) -> &'static str {
        match self {
            BankKind::Riba => "Ri.Ba.",
            BankKind::Sdd => "Addebiti SDD",
            BankKind::Sct => "Bonifici",
        }
    }
    /// Modalità di pagamento FatturaPA che alimentano questa distinta.
    pub fn mp_codes(self) -> &'static [&'static str] {
        match self {
            BankKind::Riba => &["MP12"],
            BankKind::Sdd => &["MP19", "MP20", "MP21"],
            BankKind::Sct => &["MP05"],
        }
    }
    /// Scadenze dei clienti (true) o dei fornitori (false).
    pub fn receivable(self) -> bool {
        self != BankKind::Sct
    }
    /// La scadenza può entrare in questo tipo di distinta?
    pub fn accepts(self, d: &DueRow) -> bool {
        d.receivable == self.receivable() && self.mp_codes().contains(&d.mp_code.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStatus {
    Issued,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct BankBatch {
    pub id: i64,
    pub kind: BankKind,
    pub year: i32,
    pub number: i64,
    pub created_at: NaiveDateTime,
    /// Data di esecuzione (bonifici) o prima scadenza (Ri.Ba./SDD).
    pub date: NaiveDate,
    pub file_name: String,
    pub content: String,
    pub total: Decimal,
    pub count: i64,
    pub status: BatchStatus,
    pub note: String,
    /// Voci ancora da incassare/pagare (escluse le insolute).
    pub open_items: i64,
    pub unpaid_items: i64,
}

impl BankBatch {
    pub fn label(&self) -> String {
        format!("{} n. {}/{}", self.kind.label(), self.number, self.year)
    }
}

#[derive(Debug, Clone)]
pub struct BatchItem {
    pub id: i64,
    pub due: DueRow,
    pub amount: Decimal,
    pub reference: String,
    /// Ricevuta o addebito tornato insoluto.
    pub unpaid: bool,
}

fn ts(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap_or_else(|_| date_from_sql(s).and_hms_opt(0, 0, 0).unwrap())
}

impl Store {
    pub fn bank_settings(&self) -> Result<BankSettings> {
        self.get_setting("bank")
    }

    pub fn set_bank_settings(&self, b: &BankSettings) -> Result<()> {
        self.set_setting("bank", b)
    }

    /// Scadenze aperte da presentare (o pagare) con questo tipo di distinta.
    pub fn bank_pending(&self, kind: BankKind) -> Result<Vec<DueRow>> {
        // anche le fatture salvate in bozza: numero e scadenze sono già registrati
        Ok(self.due_list_with_drafts(true, None)?.into_iter().filter(|d| kind.accepts(d) && d.batch_id.is_none() && d.residual() > Decimal::ZERO).collect())
    }

    /// Numero che avrà la prossima distinta di questo tipo nell'anno.
    pub fn next_batch_number(&self, kind: BankKind, year: i32) -> Result<i64> {
        let n: Option<i64> = self.conn().query_row("SELECT MAX(number) FROM bank_batches WHERE kind = ?1 AND year = ?2", params![kind.code(), year], |r| r.get(0))?;
        Ok(n.unwrap_or(0) + 1)
    }

    /// Registra una distinta già generata: `items` sono (scadenza, importo, riferimento).
    /// Le scadenze devono essere aperte, del tipo giusto e non già presentate.
    #[allow(clippy::too_many_arguments)]
    pub fn create_batch(&self, kind: BankKind, year: i32, number: i64, date: NaiveDate, file_name: &str, content: &str, items: &[(i64, Decimal, String)]) -> Result<BankBatch> {
        if items.is_empty() {
            return Err(StoreError::Invalid("Distinta senza voci".into()));
        }
        let pending = self.bank_pending(kind)?;
        for (due, amount, _) in items {
            let d = pending.iter().find(|d| d.id == *due).ok_or_else(|| StoreError::Invalid(format!("La scadenza {} non è più da presentare: aggiornare l'elenco", due)))?;
            if *amount > d.residual() || *amount <= Decimal::ZERO {
                return Err(StoreError::Invalid(format!("Importo non valido per {}", d.doc_label)));
            }
        }
        if number != self.next_batch_number(kind, year)? {
            return Err(StoreError::Conflict("numero di distinta già usato: ripetere l'operazione".into()));
        }
        let total: Decimal = items.iter().map(|i| i.1).sum();
        let tx = self.begin()?;
        self.conn().execute(
            "INSERT INTO bank_batches(kind, year, number, created_at, date, file_name, content, total, count, status, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'EMESSA', '')",
            params![kind.code(), year, number, now_sql(), date_to_sql(date), file_name, content, dec_to_sql(total), items.len() as i64],
        )?;
        let id = self.conn().last_insert_rowid();
        for (due, amount, reference) in items {
            self.conn().execute("INSERT INTO bank_batch_items(batch_id, due_id, amount, ref) VALUES (?1, ?2, ?3, ?4)", params![id, due, dec_to_sql(*amount), reference])?;
        }
        self.audit("bank_batch", id, "create", &format!("{} {}/{} - {} voci - {}", kind.code(), number, year, items.len(), dec_to_sql(total)))?;
        tx.commit()?;
        self.batch(id).map(|b| b.0)
    }

    /// Distinte, le più recenti prima.
    pub fn batches(&self, kind: Option<BankKind>) -> Result<Vec<BankBatch>> {
        let mut st = self.conn().prepare(
            "SELECT b.id, b.kind, b.year, b.number, b.created_at, b.date, b.file_name, b.content, b.total, b.count, b.status, b.note,
                    (SELECT COUNT(*) FROM bank_batch_items i JOIN due_dates s ON s.id = i.due_id
                      WHERE i.batch_id = b.id AND i.status = '' AND (CAST(s.amount AS REAL) - CAST(s.paid AS REAL) - CAST(s.writeoff AS REAL)) > 0.004),
                    (SELECT COUNT(*) FROM bank_batch_items i WHERE i.batch_id = b.id AND i.status = 'INSOLUTO')
             FROM bank_batches b WHERE (?1 IS NULL OR b.kind = ?1) ORDER BY b.id DESC",
        )?;
        let rows = st.query_map([kind.map(|k| k.code())], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i32>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, i64>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, Option<String>>(11)?,
                r.get::<_, i64>(12)?,
                r.get::<_, i64>(13)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, kind, year, number, created, date, file_name, content, total, count, status, note, open, unpaid) = row?;
            out.push(BankBatch {
                id,
                kind: BankKind::from_code(&kind).unwrap_or(BankKind::Riba),
                year,
                number,
                created_at: ts(&created),
                date: date_from_sql(&date),
                file_name,
                content,
                total: dec_from_sql(Some(total)),
                count,
                status: if status == "ANNULLATA" { BatchStatus::Cancelled } else { BatchStatus::Issued },
                note: note.unwrap_or_default(),
                open_items: open,
                unpaid_items: unpaid,
            });
        }
        Ok(out)
    }

    /// Distinta con le sue voci.
    pub fn batch(&self, id: i64) -> Result<(BankBatch, Vec<BatchItem>)> {
        let b = self.batches(None)?.into_iter().find(|b| b.id == id).ok_or_else(|| StoreError::NotFound("distinta".into()))?;
        let dues = self.due_list_with_drafts(false, None)?;
        let mut st = self.conn().prepare("SELECT id, due_id, amount, ref, status FROM bank_batch_items WHERE batch_id = ?1 ORDER BY id")?;
        let rows = st.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))?;
        let mut items = Vec::new();
        for row in rows {
            let (iid, due_id, amount, reference, status) = row?;
            if let Some(due) = dues.iter().find(|d| d.id == due_id) {
                items.push(BatchItem { id: iid, due: due.clone(), amount: dec_from_sql(Some(amount)), reference, unpaid: status == "INSOLUTO" });
            }
        }
        Ok((b, items))
    }

    /// Annulla una distinta (es. file mai caricato): le scadenze tornano da presentare.
    /// Non ammesso se sono già stati registrati incassi o pagamenti sulle sue scadenze.
    pub fn cancel_batch(&self, id: i64) -> Result<()> {
        let (b, items) = self.batch(id)?;
        if b.status == BatchStatus::Cancelled {
            return Ok(());
        }
        if items.iter().any(|i| !i.due.paid.is_zero() && !i.unpaid) {
            return Err(StoreError::Invalid("Sulla distinta sono già registrati incassi o pagamenti: annullarli prima dallo scadenzario".into()));
        }
        self.conn().execute("UPDATE bank_batches SET status = 'ANNULLATA' WHERE id = ?1", [id])?;
        self.audit("bank_batch", id, "cancel", &b.label())?;
        Ok(())
    }

    /// Registra l'incasso (Ri.Ba./SDD) o il pagamento (bonifici) di tutte le voci aperte
    /// della distinta, alla data indicata, sul conto indicato. Restituisce il numero di voci.
    pub fn settle_batch(&self, id: i64, date: NaiveDate, account: &str) -> Result<usize> {
        let (b, items) = self.batch(id)?;
        if b.status == BatchStatus::Cancelled {
            return Err(StoreError::Invalid("Distinta annullata".into()));
        }
        let tx = self.begin()?;
        let mut n = 0;
        for i in items.iter().filter(|i| !i.unpaid && i.due.residual() > Decimal::ZERO) {
            self.pay_due(i.due.id, date, i.due.residual(), account, false)?;
            n += 1;
        }
        self.audit("bank_batch", id, "settle", &format!("{} voci al {}", n, date_to_sql(date)))?;
        tx.commit()?;
        Ok(n)
    }

    /// Ricevuta o addebito tornato insoluto: annulla l'eventuale incasso registrato e
    /// rimette la scadenza tra quelle da presentare.
    pub fn mark_batch_item_unpaid(&self, item_id: i64, note: &str) -> Result<()> {
        let row: Option<(i64, i64)> = self.conn().query_row("SELECT batch_id, due_id FROM bank_batch_items WHERE id = ?1", [item_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        let (batch_id, due_id) = row.ok_or_else(|| StoreError::NotFound("voce di distinta".into()))?;
        let tx = self.begin()?;
        let paid: Option<String> = self.conn().query_row("SELECT paid FROM due_dates WHERE id = ?1", [due_id], |r| r.get(0)).optional()?;
        if !dec_from_sql(paid).is_zero() {
            self.unpay_due(due_id)?;
        }
        self.conn().execute("UPDATE bank_batch_items SET status = 'INSOLUTO' WHERE id = ?1", [item_id])?;
        self.audit("bank_batch", batch_id, "unpaid", &format!("scadenza {} {}", due_id, note))?;
        tx.commit()?;
        Ok(())
    }
}
