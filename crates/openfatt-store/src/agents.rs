//! Agenti e provvigioni: anagrafica, calcolo sulle fatture (sul fatturato o sull'incassato),
//! insoluti e liquidazioni.

use chrono::NaiveDate;
use rusqlite::params;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use openfatt_core::calc;
use openfatt_core::model::{DocKind, DocStatus, LineKind};
use openfatt_core::money::{pct_of, round2};

use crate::{date_from_sql, date_to_sql, dec_from_sql, dec_to_sql, now_sql, DocFilter, Result, Store, StoreError};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Agent {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub phone: String,
    pub tax_code: String,
    pub vat_number: String,
    /// Provvigione predefinita (%)
    pub rate: Decimal,
    /// Provvigione maturata solo sugli importi incassati
    pub on_collected: bool,
    /// Provvigioni per categoria di articoli (sostituiscono quella predefinita)
    pub category_rates: Vec<(String, Decimal)>,
    pub notes: String,
    pub active: bool,
}

impl Default for Agent {
    fn default() -> Self {
        Agent { id: 0, name: String::new(), email: String::new(), phone: String::new(), tax_code: String::new(), vat_number: String::new(), rate: Decimal::from(5), on_collected: true, category_rates: Vec::new(), notes: String::new(), active: true }
    }
}

/// Provvigione di un documento.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CommissionRow {
    pub document_id: i64,
    pub title: String,
    pub party: String,
    pub date: NaiveDate,
    /// Imponibile provvigionale (negativo per le note di credito)
    pub base: Decimal,
    pub commission: Decimal,
    /// Quota incassata del documento (0-1)
    pub collected: Decimal,
    /// Provvigione maturata (sull'incassato o sul fatturato)
    pub matured: Decimal,
    /// Scadenze scadute e non pagate (insoluti)
    pub overdue: Decimal,
    /// Già liquidata in una liquidazione precedente
    pub settled: Decimal,
}

impl CommissionRow {
    pub fn to_settle(&self) -> Decimal {
        self.matured - self.settled
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Settlement {
    pub id: i64,
    pub agent_id: i64,
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,
    pub created_at: String,
    pub total: Decimal,
    pub rows: Vec<(i64, Decimal)>,
}

impl Store {
    pub fn agents(&self, include_inactive: bool) -> Result<Vec<Agent>> {
        let mut st = self.conn().prepare("SELECT id, data FROM agents WHERE (?1 = 1 OR active = 1) ORDER BY name COLLATE NOCASE")?;
        let rows = st.query_map([include_inactive as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, data) = row?;
            let mut a: Agent = serde_json::from_str(&data)?;
            a.id = id;
            out.push(a);
        }
        Ok(out)
    }

    pub fn agent(&self, id: i64) -> Result<Option<Agent>> {
        Ok(self.agents(true)?.into_iter().find(|a| a.id == id))
    }

    pub fn save_agent(&self, a: &Agent) -> Result<Agent> {
        if a.name.trim().is_empty() {
            return Err(StoreError::Invalid("Indicare il nome dell'agente".into()));
        }
        if a.rate < Decimal::ZERO || a.rate > Decimal::ONE_HUNDRED {
            return Err(StoreError::Invalid("Provvigione non valida".into()));
        }
        let mut a = a.clone();
        let data = serde_json::to_string(&a)?;
        if a.id == 0 {
            self.conn().execute("INSERT INTO agents(name, active, data) VALUES (?1, ?2, ?3)", params![a.name.trim(), a.active as i64, data])?;
            a.id = self.conn().last_insert_rowid();
        } else {
            self.conn().execute("UPDATE agents SET name=?1, active=?2, data=?3 WHERE id=?4", params![a.name.trim(), a.active as i64, data, a.id])?;
        }
        self.audit("agent", a.id, "save", &a.name)?;
        Ok(a)
    }

    pub fn delete_agent(&self, id: i64) -> Result<()> {
        let used: i64 = self.conn().query_row("SELECT COUNT(*) FROM documents WHERE agent_id = ?1", [id], |r| r.get(0))?;
        if used > 0 {
            return Err(StoreError::Invalid(format!("L'agente è indicato su {} documenti: disattivarlo invece di eliminarlo", used)));
        }
        self.conn().execute("DELETE FROM agents WHERE id = ?1", [id])?;
        self.audit("agent", id, "delete", "")?;
        Ok(())
    }

    /// Provvigioni dell'agente sulle fatture e note di credito emesse nel periodo.
    pub fn commissions(&self, agent_id: i64, from: NaiveDate, to: NaiveDate, today: NaiveDate) -> Result<Vec<CommissionRow>> {
        let agent = self.agent(agent_id)?.ok_or_else(|| StoreError::NotFound("agente".into()))?;
        let kinds = vec![DocKind::Invoice, DocKind::AccompanyingInvoice, DocKind::CreditNote, DocKind::DebitNote, DocKind::Fee];
        let list = self.documents(&DocFilter { kinds, status: Some(DocStatus::Issued), from: Some(from), to: Some(to), ..Default::default() })?;
        let settled = self.settled_by_document(agent_id)?;
        let mut out = Vec::new();
        for s in list {
            let d = self.document(s.id)?;
            if d.agent_id != Some(agent_id) {
                continue;
            }
            let Ok(t) = calc::compute(&d) else { continue };
            let sign = if d.kind.is_credit() { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
            let mut base = Decimal::ZERO;
            let mut commission = Decimal::ZERO;
            for lc in t.lines.iter().filter(|l| !l.is_text && !l.is_generated) {
                let Some(l) = lc.index.and_then(|i| d.lines.get(i)) else { continue };
                if l.kind != LineKind::Item {
                    continue;
                }
                let product = match l.product_id {
                    Some(id) => self.product(id)?,
                    None => None,
                };
                let rate = l
                    .commission
                    .or_else(|| product.as_ref().and_then(|p| p.commission))
                    .or_else(|| product.as_ref().and_then(|p| agent.category_rates.iter().find(|(c, _)| !c.is_empty() && p.category.starts_with(c.as_str())).map(|(_, r)| *r)))
                    .or(d.commission_rate)
                    .unwrap_or(agent.rate);
                let b = d.to_eur(lc.total);
                base += b;
                commission += pct_of(b, rate);
            }
            let base = round2(base) * sign;
            let commission = round2(commission) * sign;
            let total_due: Decimal = d.due_dates.iter().map(|x| x.amount).sum();
            let paid: Decimal = d.due_dates.iter().map(|x| x.paid + x.writeoff).sum();
            let collected = if total_due.is_zero() { Decimal::ONE } else { (paid / total_due).min(Decimal::ONE) };
            let overdue: Decimal = d.due_dates.iter().filter(|x| x.date < today && !x.is_paid()).map(|x| x.residual()).sum();
            let matured = if agent.on_collected && !d.kind.is_credit() { round2(commission * collected) } else { commission };
            out.push(CommissionRow {
                document_id: d.id,
                title: d.title(),
                party: d.party.display_name(),
                date: d.date,
                base,
                commission,
                collected,
                matured,
                overdue: d.to_eur(overdue),
                settled: settled.get(&d.id).copied().unwrap_or_default(),
            });
        }
        out.sort_by(|a, b| a.date.cmp(&b.date).then(a.document_id.cmp(&b.document_id)));
        Ok(out)
    }

    fn settled_by_document(&self, agent_id: i64) -> Result<std::collections::HashMap<i64, Decimal>> {
        let mut m = std::collections::HashMap::new();
        for s in self.settlements(agent_id)? {
            for (doc, amount) in s.rows {
                *m.entry(doc).or_insert(Decimal::ZERO) += amount;
            }
        }
        Ok(m)
    }

    /// Registra la liquidazione delle provvigioni maturate e non ancora liquidate.
    pub fn settle_commissions(&self, agent_id: i64, from: NaiveDate, to: NaiveDate, today: NaiveDate) -> Result<Settlement> {
        let rows: Vec<(i64, Decimal)> = self.commissions(agent_id, from, to, today)?.into_iter().map(|r| (r.document_id, r.to_settle())).filter(|(_, a)| !a.is_zero()).collect();
        if rows.is_empty() {
            return Err(StoreError::Invalid("Nessuna provvigione da liquidare nel periodo".into()));
        }
        let total: Decimal = rows.iter().map(|(_, a)| *a).sum();
        let s = Settlement { id: 0, agent_id, date_from: Some(from), date_to: Some(to), created_at: now_sql(), total, rows };
        self.conn().execute(
            "INSERT INTO commission_settlements(agent_id, date_from, date_to, created_at, total, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![agent_id, date_to_sql(from), date_to_sql(to), s.created_at, dec_to_sql(total), serde_json::to_string(&s)?],
        )?;
        let id = self.conn().last_insert_rowid();
        self.audit("commission", id, "settle", &format!("agente {} totale {}", agent_id, total))?;
        Ok(Settlement { id, ..s })
    }

    pub fn settlements(&self, agent_id: i64) -> Result<Vec<Settlement>> {
        let mut st = self.conn().prepare("SELECT id, date_from, date_to, total, data FROM commission_settlements WHERE agent_id = ?1 ORDER BY id")?;
        let rows = st.query_map([agent_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, f, t, total, data) = row?;
            let mut s: Settlement = serde_json::from_str(&data)?;
            s.id = id;
            s.date_from = Some(date_from_sql(&f));
            s.date_to = Some(date_from_sql(&t));
            s.total = dec_from_sql(Some(total));
            out.push(s);
        }
        Ok(out)
    }

    pub fn delete_settlement(&self, id: i64) -> Result<()> {
        self.conn().execute("DELETE FROM commission_settlements WHERE id = ?1", [id])?;
        self.audit("commission", id, "delete", "")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;
    use openfatt_core::model::*;

    #[test]
    fn commissions_on_collected_and_settlement() {
        let s = Store::open_in_memory().unwrap();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.rate == dec!(22)).unwrap();
        let a = s.save_agent(&Agent { name: "Mario Agente".into(), rate: dec!(10), on_collected: true, category_rates: vec![("Sconti".into(), dec!(2))], ..Default::default() }).unwrap();
        let c = s.save_party(&Party { name: "Cliente".into(), vat_number: "01234567890".into(), agent_id: Some(a.id), ..Default::default() }).unwrap();
        let p = s.save_product(&Product { code: "P".into(), description: "Prodotto".into(), commission: Some(dec!(5)), ..Default::default() }).unwrap();
        let pay = s.payment_methods().unwrap().into_iter().next().unwrap();
        let mut d = Document::new(DocKind::Invoice);
        d.date = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        d.party_id = Some(c.id);
        d.agent_id = Some(a.id);
        d.payment_id = Some(pay.id);
        d.lines.push(DocLine { description: "Servizio".into(), unit_price: dec!(1000), vat: VatRef::from_code(&v), ..Default::default() });
        d.lines.push(DocLine { product_id: Some(p.id), description: "Prodotto".into(), unit_price: dec!(200), vat: VatRef::from_code(&v), ..Default::default() });
        let d = s.save_document(&d).unwrap();
        let d = s.issue_document(d.id).unwrap();
        let (from, to, today) = (NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(), NaiveDate::from_ymd_opt(2026, 11, 7).unwrap());
        let rows = s.commissions(a.id, from, to, today).unwrap();
        assert_eq!(rows.len(), 1);
        // 10% di 1000 + 5% di 200
        assert_eq!(rows[0].commission, dec!(110));
        assert_eq!(rows[0].matured, dec!(0), "nulla incassato");
        // incasso di metà del documento
        let due = d.due_dates[0].clone();
        s.conn().execute("UPDATE due_dates SET paid = ?1 WHERE id = ?2", params![dec_to_sql(due.amount / dec!(2)), due.id]).unwrap();
        let rows = s.commissions(a.id, from, to, today).unwrap();
        assert_eq!(rows[0].matured, dec!(55));
        let st = s.settle_commissions(a.id, from, to, today).unwrap();
        assert_eq!(st.total, dec!(55));
        let rows = s.commissions(a.id, from, to, today).unwrap();
        assert_eq!(rows[0].to_settle(), dec!(0));
        assert!(s.settle_commissions(a.id, from, to, today).is_err());
        assert!(s.delete_agent(a.id).is_err(), "usato in un documento");
    }
}
