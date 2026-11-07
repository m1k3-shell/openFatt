//! Statistiche di vendita e acquisto e analisi dei flussi di cassa previsti.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;

use openfatt_core::calc;
use openfatt_core::model::{DocKind, DocStatus, LineKind};

use crate::{DocFilter, Result, Store};

/// Riga di statistica: vendite (imponibile in euro), costo e margine.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatRow {
    pub key: String,
    pub label: String,
    pub qty: Decimal,
    pub revenue: Decimal,
    pub cost: Decimal,
    pub documents: usize,
}

impl StatRow {
    pub fn margin(&self) -> Decimal {
        self.revenue - self.cost
    }
    pub fn margin_pct(&self) -> Option<Decimal> {
        if self.revenue.is_zero() {
            None
        } else {
            Some(openfatt_core::money::round_dp(self.margin() / self.revenue * Decimal::ONE_HUNDRED, 1))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatBy {
    Month,
    Customer,
    Product,
    Category,
    Agent,
    Supplier,
}

/// Flussi previsti per mese: incassi e pagamenti da scadenzario.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlowRow {
    pub month: String,
    pub incoming: Decimal,
    pub outgoing: Decimal,
    pub overdue_in: Decimal,
    pub overdue_out: Decimal,
}

const SALES_STAT: &[DocKind] = &[DocKind::Invoice, DocKind::AccompanyingInvoice, DocKind::CreditNote, DocKind::DebitNote, DocKind::Fee, DocKind::RetailSale];
const PURCHASE_STAT: &[DocKind] = &[DocKind::PurchaseInvoice, DocKind::PurchaseCreditNote];

impl Store {
    /// Statistica nel periodo, raggruppata come indicato. Le note di credito sottraggono.
    /// Le fatture differite che evadono DDT si contano una volta (sulla fattura).
    pub fn statistics(&self, by: StatBy, from: NaiveDate, to: NaiveDate) -> Result<Vec<StatRow>> {
        let kinds: Vec<DocKind> = if by == StatBy::Supplier { PURCHASE_STAT.to_vec() } else { SALES_STAT.to_vec() };
        let list = self.documents(&DocFilter { kinds, status: Some(DocStatus::Issued), from: Some(from), to: Some(to), ..Default::default() })?;
        let agents: BTreeMap<i64, String> = self.agents(true)?.into_iter().map(|a| (a.id, a.name)).collect();
        let mut rows: BTreeMap<String, StatRow> = BTreeMap::new();
        let mut product_cache: std::collections::HashMap<i64, Option<openfatt_core::model::Product>> = std::collections::HashMap::new();
        for s in list {
            let d = self.document(s.id)?;
            let Ok(t) = calc::compute(&d) else { continue };
            let sign = if d.kind.is_credit() { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
            let mut touched: Vec<String> = Vec::new();
            for lc in t.lines.iter().filter(|l| !l.is_text) {
                let l = lc.index.and_then(|i| d.lines.get(i));
                if l.map(|l| l.kind != LineKind::Item).unwrap_or(false) {
                    continue;
                }
                let product = match l.and_then(|l| l.product_id) {
                    Some(id) => product_cache.entry(id).or_insert_with(|| self.product(id).ok().flatten()).clone(),
                    None => None,
                };
                let (key, label) = match by {
                    StatBy::Month => (d.date.format("%Y-%m").to_string(), month_label(d.date)),
                    StatBy::Customer | StatBy::Supplier => (d.party_id.map(|x| x.to_string()).unwrap_or_else(|| "-".into()), if d.party.display_name().is_empty() { "Vendite al banco".into() } else { d.party.display_name() }),
                    StatBy::Product => match &product {
                        Some(p) => (p.code.clone(), format!("{}  {}", p.code, p.description)),
                        None => ("-".into(), "Righe senza articolo".into()),
                    },
                    StatBy::Category => {
                        let c = product.as_ref().map(|p| p.category.clone()).unwrap_or_default();
                        let c = if c.trim().is_empty() { "Senza categoria".to_string() } else { c };
                        (c.clone(), c)
                    }
                    StatBy::Agent => match d.agent_id {
                        Some(a) => (a.to_string(), agents.get(&a).cloned().unwrap_or_else(|| "(agente eliminato)".into())),
                        None => ("-".into(), "Senza agente".into()),
                    },
                };
                let r = rows.entry(key.clone()).or_insert_with(|| StatRow { key: key.clone(), label, ..Default::default() });
                r.revenue += d.to_eur(lc.total) * sign;
                if lc.index.is_some() {
                    r.qty += lc.qty * sign;
                    let unit_cost = l.map(|l| l.unit_cost).filter(|c| !c.is_zero()).or_else(|| product.as_ref().map(|p| p.cost)).unwrap_or_default();
                    r.cost += unit_cost * lc.qty * sign;
                }
                if !touched.contains(&key) {
                    touched.push(key);
                }
            }
            for k in touched {
                if let Some(r) = rows.get_mut(&k) {
                    r.documents += 1;
                }
            }
        }
        let mut out: Vec<StatRow> = rows.into_values().map(|mut r| {
            r.revenue = openfatt_core::money::round2(r.revenue);
            r.cost = openfatt_core::money::round2(r.cost);
            r
        }).collect();
        if by != StatBy::Month {
            out.sort_by(|a, b| b.revenue.cmp(&a.revenue));
        }
        Ok(out)
    }

    /// Venduto per mese di due anni a confronto: (mese 1-12, anno corrente, anno precedente).
    pub fn sales_by_month(&self, year: i32) -> Result<Vec<(u32, Decimal, Decimal)>> {
        let cur = self.statistics(StatBy::Month, NaiveDate::from_ymd_opt(year, 1, 1).unwrap(), NaiveDate::from_ymd_opt(year, 12, 31).unwrap())?;
        let prev = self.statistics(StatBy::Month, NaiveDate::from_ymd_opt(year - 1, 1, 1).unwrap(), NaiveDate::from_ymd_opt(year - 1, 12, 31).unwrap())?;
        Ok((1..=12)
            .map(|m| {
                let k1 = format!("{}-{:02}", year, m);
                let k0 = format!("{}-{:02}", year - 1, m);
                (m, cur.iter().find(|r| r.key == k1).map(|r| r.revenue).unwrap_or_default(), prev.iter().find(|r| r.key == k0).map(|r| r.revenue).unwrap_or_default())
            })
            .collect())
    }

    /// Incassi e pagamenti previsti nei prossimi mesi, dallo scadenzario.
    pub fn cash_flow(&self, today: NaiveDate, months: u32) -> Result<Vec<FlowRow>> {
        let dues = self.due_list(true, None)?;
        let mut out: Vec<FlowRow> = (0..months)
            .map(|i| {
                let d = crate::add_months(NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap(), i);
                FlowRow { month: month_label(d), ..Default::default() }
            })
            .collect();
        let start = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
        for d in dues {
            let residual = d.residual();
            if residual.is_zero() {
                continue;
            }
            let incoming = d.receivable;
            if d.date < today {
                if let Some(first) = out.first_mut() {
                    if incoming {
                        first.overdue_in += residual;
                    } else {
                        first.overdue_out += residual;
                    }
                }
                continue;
            }
            let idx = (d.date.year() - start.year()) * 12 + d.date.month() as i32 - start.month() as i32;
            if idx < 0 || idx >= months as i32 {
                continue;
            }
            let r = &mut out[idx as usize];
            if incoming {
                r.incoming += residual;
            } else {
                r.outgoing += residual;
            }
        }
        Ok(out)
    }
}

pub fn month_label(d: NaiveDate) -> String {
    const M: [&str; 12] = ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"];
    format!("{} {}", M[d.month0() as usize], d.year())
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;
    use openfatt_core::model::*;

    #[test]
    fn statistics_by_customer_product_and_month() {
        let s = Store::open_in_memory().unwrap();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.rate == dec!(22)).unwrap();
        let a = s.save_party(&Party { name: "Alfa".into(), vat_number: "01234567890".into(), ..Default::default() }).unwrap();
        let b = s.save_party(&Party { name: "Beta".into(), vat_number: "09876543210".into(), ..Default::default() }).unwrap();
        let p = s.save_product(&Product { code: "P1".into(), description: "Prodotto".into(), cost: dec!(40), category: "Arredo".into(), manage_stock: false, ..Default::default() }).unwrap();
        let mk = |party: &Party, kind: DocKind, day: u32, qty: Decimal| {
            let mut d = Document::new(kind);
            d.date = NaiveDate::from_ymd_opt(2026, 2, day).unwrap();
            d.party_id = Some(party.id);
            d.lines.push(DocLine { product_id: Some(p.id), description: "x".into(), qty, unit_price: dec!(100), unit_cost: dec!(40), vat: VatRef::from_code(&v), ..Default::default() });
            let d = s.save_document(&d).unwrap();
            s.issue_document(d.id).unwrap();
        };
        mk(&a, DocKind::Invoice, 1, dec!(3));
        mk(&b, DocKind::Invoice, 2, dec!(1));
        mk(&a, DocKind::CreditNote, 3, dec!(1));
        let (from, to) = (NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), NaiveDate::from_ymd_opt(2026, 12, 31).unwrap());
        let c = s.statistics(StatBy::Customer, from, to).unwrap();
        assert_eq!(c[0].label, "Alfa");
        assert_eq!(c[0].revenue, dec!(200));
        assert_eq!(c[0].margin(), dec!(120));
        let pr = s.statistics(StatBy::Product, from, to).unwrap();
        assert_eq!((pr[0].qty, pr[0].revenue), (dec!(3), dec!(300)));
        assert_eq!(pr[0].margin_pct(), Some(dec!(60.0)));
        assert_eq!(s.statistics(StatBy::Category, from, to).unwrap()[0].label, "Arredo");
        let m = s.sales_by_month(2026).unwrap();
        assert_eq!(m[1].1, dec!(300));
        // flussi: le fatture senza pagamento non hanno scadenze
        assert_eq!(s.cash_flow(NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(), 3).unwrap().len(), 3);
    }
}
