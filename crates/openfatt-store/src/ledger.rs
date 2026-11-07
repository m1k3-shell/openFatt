//! Scadenzario, incassi/pagamenti e prima nota.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;

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

impl Store {
    /// Scadenze (aperte o tutte) ordinate per data.
    pub fn due_list(&self, only_open: bool, to: Option<NaiveDate>) -> Result<Vec<DueRow>> {
        let mut st = self.conn().prepare(
            "SELECT s.id, s.document_id, d.kind, d.number, d.series, d.date, d.party_name, s.date, s.amount, s.paid, s.mp_code, s.writeoff, d.party_id,
                    (SELECT bi.batch_id FROM bank_batch_items bi JOIN bank_batches b ON b.id = bi.batch_id
                      WHERE bi.due_id = s.id AND b.status <> 'ANNULLATA' AND bi.status = '' ORDER BY b.id DESC LIMIT 1)
             FROM due_dates s JOIN documents d ON d.id = s.document_id
             WHERE d.status = 'EMESSO' AND (?1 IS NULL OR s.date <= ?1)
             ORDER BY s.date, s.id",
        )?;
        let rows = st.query_map([to.map(date_to_sql)], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, String>(11)?,
                r.get::<_, Option<i64>>(12)?,
                r.get::<_, Option<i64>>(13)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, doc_id, kind, number, series, ddate, party, date, amount, paid, mp, wo, party_id, batch_id) = row?;
            let kind = DocKind::from_code(&kind).unwrap_or_default();
            let receivable = kind.is_sale() != kind.is_credit();
            let r = DueRow {
                id,
                document_id: doc_id,
                party_id,
                batch_id,
                kind,
                doc_label: format!(
                    "{} {}{} del {}",
                    kind.label(),
                    number.map(|n| n.to_string()).unwrap_or_default(),
                    if series.is_empty() { String::new() } else { format!("/{}", series) },
                    date_from_sql(&ddate).format("%d/%m/%Y")
                ),
                party_name: party.unwrap_or_default(),
                date: date_from_sql(&date),
                amount: dec_from_sql(Some(amount)),
                paid: dec_from_sql(Some(paid)),
                writeoff: dec_from_sql(Some(wo)),
                mp_code: mp.unwrap_or_default(),
                receivable,
            };
            if !only_open || r.residual() > Decimal::ZERO {
                out.push(r);
            }
        }
        Ok(out)
    }

    /// Registra un incasso/pagamento su una scadenza, con movimento di prima nota.
    /// `writeoff` chiude la scadenza registrando la differenza come abbuono esplicito.
    pub fn pay_due(&self, due_id: i64, date: NaiveDate, amount: Decimal, account: &str, writeoff: bool) -> Result<()> {
        if amount <= Decimal::ZERO {
            return Err(StoreError::Invalid("L'importo deve essere positivo".into()));
        }
        let rows = self.due_list(false, None)?;
        let due = rows.into_iter().find(|r| r.id == due_id).ok_or_else(|| StoreError::NotFound("scadenza".into()))?;
        let residual = due.residual();
        if amount > residual {
            return Err(StoreError::Invalid(format!("L'importo supera il residuo della scadenza ({})", residual)));
        }
        let party_id = due.party_id;
        let tx = self.begin()?;
        let sign = if due.receivable { Decimal::ONE } else { Decimal::NEGATIVE_ONE };
        let verb = if due.receivable { "Incasso" } else { "Pagamento" };
        self.add_cash_entry(&CashEntry {
            id: 0,
            date,
            description: format!("{} {} - {}", verb, due.doc_label, due.party_name),
            account: account.to_string(),
            amount: amount * sign,
            document_id: Some(due.document_id),
            due_date_id: Some(due_id),
            party_id,
            category: if due.receivable { "Incassi clienti".into() } else { "Pagamenti fornitori".into() },
        })?;
        let new_paid = due.paid + amount;
        let writeoff = if writeoff { due.amount - new_paid } else { due.writeoff };
        self.conn().execute(
            "UPDATE due_dates SET paid = ?1, writeoff = ?2, paid_date = ?3 WHERE id = ?4",
            params![dec_to_sql(new_paid), dec_to_sql(writeoff), date_to_sql(date), due_id],
        )?;
        self.audit("due_date", due_id, "pay", &format!("{} {}", amount, account))?;
        tx.commit()?;
        Ok(())
    }

    /// Annulla tutti i pagamenti di una scadenza (eliminando i movimenti collegati).
    pub fn unpay_due(&self, due_id: i64) -> Result<()> {
        let tx = self.begin()?;
        self.conn().execute("DELETE FROM cash_entries WHERE due_date_id = ?1", [due_id])?;
        self.conn().execute("UPDATE due_dates SET paid = '0', writeoff = '0', paid_date = NULL WHERE id = ?1", [due_id])?;
        self.audit("due_date", due_id, "unpay", "")?;
        tx.commit()?;
        Ok(())
    }

    pub fn add_cash_entry(&self, e: &CashEntry) -> Result<i64> {
        if e.account.trim().is_empty() {
            return Err(StoreError::Invalid("Indicare il conto (cassa, banca, ...)".into()));
        }
        self.conn().execute(
            "INSERT INTO cash_entries(date, description, account, amount, document_id, due_date_id, party_id, category)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![date_to_sql(e.date), e.description, e.account.trim(), dec_to_sql(e.amount), e.document_id, e.due_date_id, e.party_id, e.category],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    pub fn delete_cash_entry(&self, id: i64) -> Result<()> {
        let linked: Option<i64> = self.conn().query_row("SELECT due_date_id FROM cash_entries WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten();
        if linked.is_some() {
            return Err(StoreError::Invalid("Movimento collegato a una scadenza: annullare il pagamento dallo scadenzario".into()));
        }
        self.conn().execute("DELETE FROM cash_entries WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn cash_entries(&self, from: Option<NaiveDate>, to: Option<NaiveDate>, account: Option<&str>) -> Result<Vec<CashEntry>> {
        let mut st = self.conn().prepare(
            "SELECT id, date, description, account, amount, document_id, due_date_id, party_id, category FROM cash_entries
             WHERE (?1 IS NULL OR date >= ?1) AND (?2 IS NULL OR date <= ?2) AND (?3 IS NULL OR account = ?3)
             ORDER BY date, id",
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
