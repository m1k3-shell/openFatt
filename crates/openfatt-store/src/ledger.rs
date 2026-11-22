//! Scadenzario, incassi/pagamenti e prima nota.
//!
//! Un incasso (o pagamento) è una registrazione con data, modalità (contanti, assegno,
//! bonifico...), conto e riferimento (numero dell'assegno, CRO...) che si ripartisce su una o
//! più scadenze dello stesso cliente o fornitore, anche solo in parte: un acconto su una
//! fattura o su un gruppo di fatture lascia un residuo sull'ultima. Per ogni scadenza
//! coperta si scrive un movimento di prima nota legato al documento (serve anche per l'IVA
//! per cassa).

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rusqlite::{params, params_from_iter, OptionalExtension, ToSql};
use rust_decimal::Decimal;

use openfatt_core::codes;
use openfatt_core::model::{CashEntry, DocKind};

use crate::{date_from_sql, date_to_sql, dec_from_sql, dec_to_sql, Result, Store, StoreError};

#[derive(Debug, Clone)]
pub struct DueRow {
    pub id: i64,
    pub document_id: i64,
    pub party_id: Option<i64>,
    /// Distinta bancaria in cui la scadenza è stata presentata (se ancora valida).
    pub batch_id: Option<i64>,
    pub kind: DocKind,
    pub doc_label: String,
    pub party_name: String,
    pub date: NaiveDate,
    pub amount: Decimal,
    pub paid: Decimal,
    pub writeoff: Decimal,
    pub mp_code: String,
    /// true = da incassare (vendite), false = da pagare (acquisti)
    pub receivable: bool,
    /// Il documento è ancora in bozza.
    pub draft: bool,
    /// Data dell'ultimo incasso/pagamento registrato.
    pub paid_date: Option<NaiveDate>,
}

impl DueRow {
    pub fn residual(&self) -> Decimal {
        self.amount - self.paid - self.writeoff
    }
}

#[derive(Debug, Clone)]
pub struct AccountBalance {
    pub account: String,
    pub balance: Decimal,
}

/// Quota di una registrazione assegnata a una scadenza.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Allocation {
    pub due_id: i64,
    pub amount: Decimal,
    /// Chiude la scadenza registrando come abbuono quello che resta dopo questa quota.
    pub writeoff: bool,
}

/// Incasso o pagamento da registrare.
#[derive(Debug, Clone)]
pub struct NewPayment {
    pub date: NaiveDate,
    /// Modalità FatturaPA (MP01 contanti, MP02 assegno, MP05 bonifico...).
    pub mp_code: String,
    pub account: String,
    /// Numero dell'assegno e banca, CRO/TRN del bonifico, ecc.
    pub reference: String,
    pub notes: String,
    pub allocations: Vec<Allocation>,
}

/// Scadenza coperta (in tutto o in parte) da una registrazione.
#[derive(Debug, Clone)]
pub struct PaymentLine {
    pub due_id: i64,
    pub document_id: i64,
    pub doc_label: String,
    pub due_date: NaiveDate,
    pub amount: Decimal,
    pub writeoff: Decimal,
}

/// Incasso o pagamento registrato.
#[derive(Debug, Clone)]
pub struct PaymentRow {
    pub id: i64,
    pub date: NaiveDate,
    pub party_id: Option<i64>,
    pub party_name: String,
    pub receivable: bool,
    pub mp_code: String,
    pub account: String,
    pub reference: String,
    pub notes: String,
    pub total: Decimal,
    pub lines: Vec<PaymentLine>,
}

impl PaymentRow {
    /// "Bonifico CRO 1234" / "Assegno n. 55 Banca X" / "Contanti".
    pub fn method_text(&self) -> String {
        method_text(&self.mp_code, &self.reference)
    }
}

/// Modalità e riferimento in una riga di testo.
pub fn method_text(mp_code: &str, reference: &str) -> String {
    let m = if mp_code.is_empty() { "Modalità non indicata".to_string() } else { codes::describe(codes::MODALITA_PAGAMENTO, mp_code).to_string() };
    if reference.trim().is_empty() {
        m
    } else {
        format!("{} {}", m, reference.trim())
    }
}

/// Modalità proposte per incassi e pagamenti, nell'ordine in cui si usano più spesso.
pub const COMMON_METHODS: [&str; 9] = ["MP01", "MP02", "MP03", "MP05", "MP08", "MP12", "MP19", "MP07", "MP23"];

/// Conto proposto per una modalità: i contanti in cassa, il resto in banca.
pub fn default_account(mp_code: &str) -> &'static str {
    if mp_code == "MP01" {
        "Cassa"
    } else {
        "Banca"
    }
}

/// Ripartisce un importo sulle scadenze nell'ordine dato (di solito dalla più vecchia):
/// ognuna riceve al massimo il suo residuo, l'ultima raggiunta resta in parte da saldare.
pub fn distribute(total: Decimal, residuals: &[Decimal]) -> Vec<Decimal> {
    let mut left = total.max(Decimal::ZERO);
    residuals
        .iter()
        .map(|r| {
            let a = left.min((*r).max(Decimal::ZERO));
            left -= a;
            a
        })
        .collect()
}

const DUE_SELECT: &str = "SELECT s.id, s.document_id, d.kind, d.number, d.series, d.date, d.party_name, s.date, s.amount, s.paid, s.mp_code, s.writeoff, d.party_id,
        (SELECT bi.batch_id FROM bank_batch_items bi JOIN bank_batches b ON b.id = bi.batch_id
          WHERE bi.due_id = s.id AND b.status <> 'ANNULLATA' AND bi.status = '' ORDER BY b.id DESC LIMIT 1),
        d.status, s.paid_date
 FROM due_dates s JOIN documents d ON d.id = s.document_id";

fn doc_label(kind: DocKind, number: Option<i64>, series: &str, date: NaiveDate) -> String {
    format!(
        "{} {}{} del {}",
        kind.label(),
        number.map(|n| n.to_string()).unwrap_or_default(),
        if series.is_empty() { String::new() } else { format!("/{}", series) },
        date.format("%d/%m/%Y")
    )
}

impl Store {
    /// Scadenze (aperte o tutte) ordinate per data.
    pub fn due_list(&self, only_open: bool, to: Option<NaiveDate>) -> Result<Vec<DueRow>> {
        self.due_rows(only_open, to, false)
    }

    /// Come `due_list`, comprese le scadenze dei documenti in bozza.
    pub fn due_list_with_drafts(&self, only_open: bool, to: Option<NaiveDate>) -> Result<Vec<DueRow>> {
        self.due_rows(only_open, to, true)
    }

    fn due_rows(&self, only_open: bool, to: Option<NaiveDate>, drafts: bool) -> Result<Vec<DueRow>> {
        let statuses = if drafts { "('EMESSO','BOZZA')" } else { "('EMESSO')" };
        let to = to.map(date_to_sql);
        let rows = self.query_dues(&format!("WHERE d.status IN {} AND (?1 IS NULL OR s.date <= ?1) ORDER BY s.date, s.id", statuses), &[&to as &dyn ToSql])?;
        Ok(rows.into_iter().filter(|r| !only_open || r.residual() > Decimal::ZERO).collect())
    }

    fn query_dues(&self, tail: &str, args: &[&dyn ToSql]) -> Result<Vec<DueRow>> {
        let mut st = self.conn().prepare(&format!("{} {}", DUE_SELECT, tail))?;
        let rows = st.query_map(args, |r| {
            let kind = DocKind::from_code(&r.get::<_, String>(2)?).unwrap_or_default();
            let receivable = kind.is_sale() != kind.is_credit();
            Ok(DueRow {
                id: r.get(0)?,
                document_id: r.get(1)?,
                party_id: r.get(12)?,
                batch_id: r.get(13)?,
                kind,
                doc_label: doc_label(kind, r.get(3)?, &r.get::<_, String>(4)?, date_from_sql(&r.get::<_, String>(5)?)),
                party_name: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                date: date_from_sql(&r.get::<_, String>(7)?),
                amount: dec_from_sql(r.get(8)?),
                paid: dec_from_sql(r.get(9)?),
                mp_code: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
                writeoff: dec_from_sql(r.get(11)?),
                receivable,
                draft: r.get::<_, String>(14)? == "BOZZA",
                paid_date: r.get::<_, Option<String>>(15)?.map(|d| date_from_sql(&d)),
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Scadenze indicate (di documenti emessi o in bozza), nell'ordine dato.
    pub fn dues_by_id(&self, ids: &[i64]) -> Result<Vec<DueRow>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let marks = vec!["?"; ids.len()].join(",");
        let args: Vec<&dyn ToSql> = ids.iter().map(|i| i as &dyn ToSql).collect();
        let found = self.query_dues(&format!("WHERE s.id IN ({})", marks), &args)?;
        Ok(ids.iter().filter_map(|id| found.iter().find(|d| d.id == *id).cloned()).collect())
    }

    /// Scadenze ancora da saldare di un cliente o fornitore (documenti emessi), dalla più
    /// vecchia. `receivable`: true = da incassare, false = da pagare.
    pub fn open_dues_of_party(&self, party_id: Option<i64>, party_name: &str, receivable: bool) -> Result<Vec<DueRow>> {
        let rows = match party_id {
            Some(id) => self.query_dues("WHERE d.status = 'EMESSO' AND d.party_id = ?1 ORDER BY s.date, s.id", &[&id as &dyn ToSql])?,
            None => self.query_dues("WHERE d.status = 'EMESSO' AND d.party_id IS NULL AND d.party_name = ?1 ORDER BY s.date, s.id", &[&party_name as &dyn ToSql])?,
        };
        Ok(rows.into_iter().filter(|r| r.receivable == receivable && r.residual() > Decimal::ZERO).collect())
    }

    /// Registra un incasso/pagamento su una scadenza, con movimento di prima nota.
    /// `writeoff` chiude la scadenza registrando la differenza come abbuono esplicito.
    pub fn pay_due(&self, due_id: i64, date: NaiveDate, amount: Decimal, account: &str, writeoff: bool) -> Result<()> {
        let due = self.dues_by_id(&[due_id])?.pop().ok_or_else(|| StoreError::NotFound("scadenza".into()))?;
        let mp_code = if account.eq_ignore_ascii_case("cassa") { "MP01".to_string() } else { due.mp_code.clone() };
        self.register_payment(&NewPayment {
            date,
            mp_code,
            account: account.to_string(),
            reference: String::new(),
            notes: String::new(),
            allocations: vec![Allocation { due_id, amount, writeoff }],
        })?;
        Ok(())
    }

    /// Registra un incasso o un pagamento ripartito su una o più scadenze dello stesso
    /// cliente o fornitore. Ogni quota non può superare il residuo della sua scadenza; una
    /// quota minore lascia il residuo da saldare (acconto), a meno di chiuderla con abbuono.
    /// Restituisce il numero della registrazione.
    pub fn register_payment(&self, p: &NewPayment) -> Result<i64> {
        let allocs: Vec<&Allocation> = p.allocations.iter().filter(|a| a.amount > Decimal::ZERO || a.writeoff).collect();
        if allocs.is_empty() {
            return Err(StoreError::Invalid("Indicare l'importo incassato o pagato su almeno una scadenza".into()));
        }
        if allocs.iter().any(|a| a.amount < Decimal::ZERO) {
            return Err(StoreError::Invalid("Gli importi devono essere positivi".into()));
        }
        let total: Decimal = allocs.iter().map(|a| a.amount).sum();
        if total <= Decimal::ZERO {
            return Err(StoreError::Invalid("L'importo deve essere positivo".into()));
        }
        if p.account.trim().is_empty() {
            return Err(StoreError::Invalid("Indicare il conto (cassa, banca, ...)".into()));
        }
        let mut ids: Vec<i64> = allocs.iter().map(|a| a.due_id).collect();
        ids.sort_unstable();
        ids.dedup();
        if ids.len() != allocs.len() {
            return Err(StoreError::Invalid("La stessa scadenza compare due volte".into()));
        }
        let dues = self.dues_by_id(&allocs.iter().map(|a| a.due_id).collect::<Vec<_>>())?;
        if dues.len() != allocs.len() {
            return Err(StoreError::NotFound("scadenza".into()));
        }
        let first = &dues[0];
        for d in &dues {
            if d.draft {
                return Err(StoreError::Invalid(format!("{} è ancora in bozza: emetterlo prima di registrare incassi", d.doc_label)));
            }
            if d.receivable != first.receivable {
                return Err(StoreError::Invalid("Non si possono registrare insieme incassi e pagamenti".into()));
            }
            if d.party_id != first.party_id || (d.party_id.is_none() && d.party_name != first.party_name) {
                return Err(StoreError::Invalid("Le scadenze devono essere dello stesso cliente o fornitore".into()));
            }
        }
        for (a, d) in allocs.iter().zip(&dues) {
            if a.amount > d.residual() {
                return Err(StoreError::Invalid(format!("{}: l'importo {} supera il residuo della scadenza ({})", d.doc_label, a.amount, d.residual())));
            }
        }
        let receivable = first.receivable;
        let sign = if receivable { Decimal::ONE } else { Decimal::NEGATIVE_ONE };
        let verb = if receivable { "Incasso" } else { "Pagamento" };
        let how = method_text(&p.mp_code, &p.reference);
        let tx = self.begin()?;
        self.conn().execute(
            "INSERT INTO payments(date, party_id, party_name, receivable, mp_code, account, reference, notes, amount) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![date_to_sql(p.date), first.party_id, first.party_name, receivable as i64, p.mp_code, p.account.trim(), p.reference.trim(), p.notes.trim(), dec_to_sql(total)],
        )?;
        let pid = self.conn().last_insert_rowid();
        for (a, d) in allocs.iter().zip(&dues) {
            let new_paid = d.paid + a.amount;
            let left = d.amount - new_paid - d.writeoff;
            let wo_now = if a.writeoff { left.max(Decimal::ZERO) } else { Decimal::ZERO };
            self.conn().execute(
                "INSERT INTO payment_allocations(payment_id, due_id, amount, writeoff) VALUES (?1, ?2, ?3, ?4)",
                params![pid, d.id, dec_to_sql(a.amount), dec_to_sql(wo_now)],
            )?;
            if a.amount > Decimal::ZERO {
                let what = if left - wo_now > Decimal::ZERO { "acconto" } else if d.paid.is_zero() { "saldo" } else { "saldo residuo" };
                self.add_cash_entry(&CashEntry {
                    id: 0,
                    date: p.date,
                    description: format!("{} {} {} - {} ({})", verb, what, d.doc_label, d.party_name, how),
                    account: p.account.trim().to_string(),
                    amount: a.amount * sign,
                    document_id: Some(d.document_id),
                    due_date_id: Some(d.id),
                    party_id: d.party_id,
                    category: if receivable { "Incassi clienti".into() } else { "Pagamenti fornitori".into() },
                    payment_id: Some(pid),
                    ..Default::default()
                })?;
            }
            self.conn().execute(
                "UPDATE due_dates SET paid = ?1, writeoff = ?2, paid_date = ?3 WHERE id = ?4",
                params![dec_to_sql(new_paid), dec_to_sql(d.writeoff + wo_now), date_to_sql(p.date), d.id],
            )?;
        }
        self.audit("payment", pid, "register", &format!("{} {} {} su {} scadenze", total, p.account.trim(), how, allocs.len()))?;
        tx.commit()?;
        Ok(pid)
    }

    /// Annulla una registrazione: le scadenze tornano con il residuo di prima e i movimenti
    /// di prima nota collegati vengono eliminati.
    pub fn cancel_payment(&self, id: i64) -> Result<()> {
        let allocs: Vec<(i64, i64, Decimal, Decimal)> = {
            let mut st = self.conn().prepare("SELECT id, due_id, amount, writeoff FROM payment_allocations WHERE payment_id = ?1")?;
            let rows = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, dec_from_sql(r.get(2)?), dec_from_sql(r.get(3)?))))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        let exists: Option<i64> = self.conn().query_row("SELECT id FROM payments WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        if exists.is_none() {
            return Err(StoreError::NotFound("registrazione".into()));
        }
        let tx = self.begin()?;
        self.conn().execute("DELETE FROM cash_entries WHERE payment_id = ?1", [id])?;
        for (alloc_id, due_id, amount, wo) in allocs {
            self.conn().execute("DELETE FROM payment_allocations WHERE id = ?1", [alloc_id])?;
            self.reduce_due(due_id, amount, wo)?;
        }
        self.conn().execute("DELETE FROM payments WHERE id = ?1", [id])?;
        self.audit("payment", id, "cancel", "")?;
        tx.commit()?;
        Ok(())
    }

    /// Toglie da una scadenza un importo pagato e un abbuono, aggiornando la data dell'ultimo
    /// pagamento con quelli che restano.
    fn reduce_due(&self, due_id: i64, amount: Decimal, wo: Decimal) -> Result<()> {
        let row: Option<(String, String, Option<String>)> =
            self.conn().query_row("SELECT paid, writeoff, paid_date FROM due_dates WHERE id = ?1", [due_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
        let Some((paid, writeoff, before)) = row else { return Ok(()) };
        let paid = (dec_from_sql(Some(paid)) - amount).max(Decimal::ZERO);
        let writeoff = (dec_from_sql(Some(writeoff)) - wo).max(Decimal::ZERO);
        let last: Option<String> = self
            .conn()
            .query_row("SELECT MAX(p.date) FROM payment_allocations a JOIN payments p ON p.id = a.payment_id WHERE a.due_id = ?1", [due_id], |r| r.get(0))?;
        // pagamenti precedenti senza registrazione (archivi importati): resta la loro data
        let date = if paid.is_zero() && writeoff.is_zero() { None } else { last.or(before) };
        self.conn().execute(
            "UPDATE due_dates SET paid = ?1, writeoff = ?2, paid_date = ?3 WHERE id = ?4",
            params![dec_to_sql(paid), dec_to_sql(writeoff), date, due_id],
        )?;
        Ok(())
    }

    /// Annulla tutti i pagamenti di una scadenza (eliminando i movimenti collegati). Le
    /// registrazioni che coprivano anche altre scadenze restano per quelle.
    pub fn unpay_due(&self, due_id: i64) -> Result<()> {
        let allocs: Vec<(i64, i64, Decimal)> = {
            let mut st = self.conn().prepare("SELECT id, payment_id, amount FROM payment_allocations WHERE due_id = ?1")?;
            let rows = st.query_map([due_id], |r| Ok((r.get(0)?, r.get(1)?, dec_from_sql(r.get(2)?))))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        let tx = self.begin()?;
        for (alloc_id, pid, amount) in allocs {
            self.conn().execute("DELETE FROM payment_allocations WHERE id = ?1", [alloc_id])?;
            let left: i64 = self.conn().query_row("SELECT COUNT(*) FROM payment_allocations WHERE payment_id = ?1", [pid], |r| r.get(0))?;
            if left == 0 {
                self.conn().execute("DELETE FROM payments WHERE id = ?1", [pid])?;
            } else {
                let total: Option<String> = self.conn().query_row("SELECT amount FROM payments WHERE id = ?1", [pid], |r| r.get(0)).optional()?;
                let total = dec_from_sql(total) - amount;
                self.conn().execute("UPDATE payments SET amount = ?1 WHERE id = ?2", params![dec_to_sql(total.max(Decimal::ZERO)), pid])?;
            }
        }
        self.conn().execute("DELETE FROM cash_entries WHERE due_date_id = ?1", [due_id])?;
        self.conn().execute("UPDATE due_dates SET paid = '0', writeoff = '0', paid_date = NULL WHERE id = ?1", [due_id])?;
        self.audit("due_date", due_id, "unpay", "")?;
        tx.commit()?;
        Ok(())
    }

    /// Sposta a una nuova data quello che resta da saldare di una scadenza. Se sulla
    /// scadenza ci sono già pagamenti, la parte pagata resta alla data originale e il residuo
    /// diventa una nuova scadenza.
    pub fn reschedule_residual(&self, due_id: i64, date: NaiveDate) -> Result<()> {
        let due = self.dues_by_id(&[due_id])?.pop().ok_or_else(|| StoreError::NotFound("scadenza".into()))?;
        let residual = due.residual();
        if residual <= Decimal::ZERO {
            return Err(StoreError::Invalid("La scadenza è già saldata".into()));
        }
        if due.batch_id.is_some() {
            return Err(StoreError::Invalid("La scadenza è in una distinta bancaria: annullare prima la distinta".into()));
        }
        let tx = self.begin()?;
        if due.paid.is_zero() && due.writeoff.is_zero() {
            self.conn().execute("UPDATE due_dates SET date = ?1 WHERE id = ?2", params![date_to_sql(date), due_id])?;
        } else {
            self.conn().execute("UPDATE due_dates SET amount = ?1 WHERE id = ?2", params![dec_to_sql(due.paid + due.writeoff), due_id])?;
            self.conn().execute(
                "INSERT INTO due_dates(document_id, date, amount, mp_code, paid) VALUES (?1, ?2, ?3, ?4, '0')",
                params![due.document_id, date_to_sql(date), dec_to_sql(residual), due.mp_code],
            )?;
        }
        self.audit("due_date", due_id, "reschedule", &format!("{} al {}", residual, date_to_sql(date)))?;
        tx.commit()?;
        Ok(())
    }

    fn payment_rows(&self, tail: &str, args: &[&dyn ToSql]) -> Result<Vec<PaymentRow>> {
        let mut out: Vec<PaymentRow> = {
            let mut st = self.conn().prepare(&format!(
                "SELECT p.id, p.date, p.party_id, p.party_name, p.receivable, p.mp_code, p.account, p.reference, p.notes, p.amount FROM payments p {}",
                tail
            ))?;
            let rows = st.query_map(params_from_iter(args.iter()), |r| {
                Ok(PaymentRow {
                    id: r.get(0)?,
                    date: date_from_sql(&r.get::<_, String>(1)?),
                    party_id: r.get(2)?,
                    party_name: r.get(3)?,
                    receivable: r.get::<_, i64>(4)? != 0,
                    mp_code: r.get(5)?,
                    account: r.get(6)?,
                    reference: r.get(7)?,
                    notes: r.get(8)?,
                    total: dec_from_sql(r.get(9)?),
                    lines: Vec::new(),
                })
            })?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        if out.is_empty() {
            return Ok(out);
        }
        let marks = vec!["?"; out.len()].join(",");
        let mut st = self.conn().prepare(&format!(
            "SELECT a.payment_id, a.due_id, s.document_id, d.kind, d.number, d.series, d.date, s.date, a.amount, a.writeoff
             FROM payment_allocations a JOIN due_dates s ON s.id = a.due_id JOIN documents d ON d.id = s.document_id
             WHERE a.payment_id IN ({}) ORDER BY s.date, a.id",
            marks
        ))?;
        let rows = st.query_map(params_from_iter(out.iter().map(|p| p.id)), |r| {
            let kind = DocKind::from_code(&r.get::<_, String>(3)?).unwrap_or_default();
            Ok((
                r.get::<_, i64>(0)?,
                PaymentLine {
                    due_id: r.get(1)?,
                    document_id: r.get(2)?,
                    doc_label: doc_label(kind, r.get(4)?, &r.get::<_, String>(5)?, date_from_sql(&r.get::<_, String>(6)?)),
                    due_date: date_from_sql(&r.get::<_, String>(7)?),
                    amount: dec_from_sql(r.get(8)?),
                    writeoff: dec_from_sql(r.get(9)?),
                },
            ))
        })?;
        for row in rows {
            let (pid, line) = row?;
            if let Some(p) = out.iter_mut().find(|p| p.id == pid) {
                p.lines.push(line);
            }
        }
        Ok(out)
    }

    /// Incassi e pagamenti registrati nel periodo, dal più recente.
    pub fn payments(&self, from: Option<NaiveDate>, to: Option<NaiveDate>) -> Result<Vec<PaymentRow>> {
        let (f, t) = (from.map(date_to_sql), to.map(date_to_sql));
        self.payment_rows("WHERE (?1 IS NULL OR p.date >= ?1) AND (?2 IS NULL OR p.date <= ?2) ORDER BY p.date DESC, p.id DESC", &[&f as &dyn ToSql, &t])
    }

    /// Incassi e pagamenti che toccano le scadenze di un documento (in ordine di data).
    pub fn payments_of_document(&self, doc_id: i64) -> Result<Vec<PaymentRow>> {
        self.payment_rows(
            "WHERE p.id IN (SELECT a.payment_id FROM payment_allocations a JOIN due_dates s ON s.id = a.due_id WHERE s.document_id = ?1) ORDER BY p.date, p.id",
            &[&doc_id as &dyn ToSql],
        )
    }

    pub fn add_cash_entry(&self, e: &CashEntry) -> Result<i64> {
        if e.account.trim().is_empty() {
            return Err(StoreError::Invalid("Indicare il conto (cassa, banca, ...)".into()));
        }
        self.conn().execute(
            "INSERT INTO cash_entries(date, description, account, amount, document_id, due_date_id, party_id, category, payment_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![date_to_sql(e.date), e.description, e.account.trim(), dec_to_sql(e.amount), e.document_id, e.due_date_id, e.party_id, e.category, e.payment_id],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    pub fn delete_cash_entry(&self, id: i64) -> Result<()> {
        let linked: Option<(Option<i64>, Option<i64>)> = self.conn().query_row("SELECT due_date_id, payment_id FROM cash_entries WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        if let Some((due, payment)) = linked {
            if due.is_some() || payment.is_some() {
                return Err(StoreError::Invalid("Movimento collegato a un incasso o pagamento: annullarlo dallo scadenzario (Incassi registrati)".into()));
            }
        }
        self.conn().execute("DELETE FROM cash_entries WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn cash_entries(&self, from: Option<NaiveDate>, to: Option<NaiveDate>, account: Option<&str>) -> Result<Vec<CashEntry>> {
        let mut st = self.conn().prepare(
            "SELECT c.id, c.date, c.description, c.account, c.amount, c.document_id, c.due_date_id, c.party_id, c.category, c.payment_id, p.mp_code, p.reference
             FROM cash_entries c LEFT JOIN payments p ON p.id = c.payment_id
             WHERE (?1 IS NULL OR c.date >= ?1) AND (?2 IS NULL OR c.date <= ?2) AND (?3 IS NULL OR c.account = ?3)
             ORDER BY c.date, c.id",
        )?;
        let rows = st.query_map(params![from.map(date_to_sql), to.map(date_to_sql), account], |r| {
            Ok(CashEntry {
                id: r.get(0)?,
                date: date_from_sql(&r.get::<_, String>(1)?),
                description: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                account: r.get(3)?,
                amount: dec_from_sql(r.get(4)?),
                document_id: r.get(5)?,
                due_date_id: r.get(6)?,
                party_id: r.get(7)?,
                category: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
                payment_id: r.get(9)?,
                method: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
                reference: r.get::<_, Option<String>>(11)?.unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn account_balances(&self) -> Result<Vec<AccountBalance>> {
        let mut m: BTreeMap<String, Decimal> = BTreeMap::new();
        for e in self.cash_entries(None, None, None)? {
            *m.entry(e.account).or_default() += e.amount;
        }
        Ok(m.into_iter().map(|(account, balance)| AccountBalance { account, balance }).collect())
    }

    /// Conti usati finora (per suggerimenti nell'interfaccia).
    pub fn accounts(&self) -> Result<Vec<String>> {
        let mut st = self.conn().prepare("SELECT DISTINCT account FROM cash_entries ORDER BY account")?;
        let mut v: Vec<String> = st.query_map([], |r| r.get(0))?.filter_map(|r| r.ok()).collect();
        for d in ["Cassa", "Banca"] {
            if !v.iter().any(|a| a == d) {
                v.push(d.to_string());
            }
        }
        Ok(v)
    }
}
