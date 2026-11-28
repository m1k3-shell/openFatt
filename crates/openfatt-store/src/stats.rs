//! Statistiche di vendita e acquisto e analisi dei flussi di cassa previsti.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;

use openfatt_core::calc;
use openfatt_core::model::{DocKind, LineKind};

use crate::{Result, Store};

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
    /// Commessa interna: ricavi delle vendite, costo della merce e acquisti della commessa.
    Project,
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

#[derive(Debug, Clone)]
struct Brief {
    code: String,
    description: String,
    category: String,
    cost: Decimal,
}

/// Documento ridotto a quello che serve alle statistiche (importi già in euro e col segno).
#[derive(Debug)]
pub(crate) struct StatDoc {
    date: NaiveDate,
    party_id: Option<i64>,
    party_name: String,
    agent_id: Option<i64>,
    project: String,
    purchase: bool,
    lines: Vec<StatLine>,
}

#[derive(Debug)]
struct StatLine {
    product_id: Option<i64>,
    /// riga del documento (non una riga aggiunta dal calcolo, come il bollo)
    item: bool,
    qty: Decimal,
    revenue: Decimal,
    /// costo indicato sulla riga (zero: si usa quello dell'articolo)
    unit_cost: Decimal,
}

impl StatDoc {
    fn from(d: &openfatt_core::model::Document) -> Option<StatDoc> {
        // fattura di una vendita al banco: la vendita è già contata sullo scontrino
        if d.from_receipt_id.is_some() {
            return None;
        }
        let t = calc::compute(d).ok()?;
        let sign = if d.kind.is_credit() { Decimal::NEGATIVE_ONE } else { Decimal::ONE };
        let mut lines = Vec::new();
        for lc in t.lines.iter().filter(|l| !l.is_text) {
            let l = lc.index.and_then(|i| d.lines.get(i));
            if l.map(|l| l.kind != LineKind::Item).unwrap_or(false) {
                continue;
            }
            // la detrazione di un acconto non è un ricavo in meno: la fattura di saldo conta
            // per intero (le fatture d'acconto non entrano nelle statistiche)
            if l.and_then(crate::deducted_down_payment).is_some() {
                continue;
            }
            lines.push(StatLine {
                product_id: l.and_then(|l| l.product_id),
                item: lc.index.is_some(),
                qty: lc.qty * sign,
                revenue: d.to_eur(lc.total) * sign,
                unit_cost: l.map(|l| l.unit_cost).unwrap_or_default(),
            });
        }
        Some(StatDoc { date: d.date, party_id: d.party_id, party_name: d.party.display_name(), agent_id: d.agent_id, project: d.project.trim().to_string(), purchase: d.kind.is_purchase(), lines })
    }
}

const SALES_STAT: &[DocKind] = &[DocKind::Invoice, DocKind::AccompanyingInvoice, DocKind::CreditNote, DocKind::DebitNote, DocKind::Fee, DocKind::RetailSale];
const PURCHASE_STAT: &[DocKind] = &[DocKind::PurchaseInvoice, DocKind::PurchaseCreditNote];

impl Store {
    /// Statistica nel periodo, raggruppata come indicato. Le note di credito sottraggono.
    /// Le fatture differite che evadono DDT si contano una volta (sulla fattura).
    pub fn statistics(&self, by: StatBy, from: NaiveDate, to: NaiveDate) -> Result<Vec<StatRow>> {
        let kinds: Vec<DocKind> = match by {
            StatBy::Supplier => PURCHASE_STAT.to_vec(),
            StatBy::Project => SALES_STAT.iter().chain(PURCHASE_STAT.iter()).copied().collect(),
            _ => SALES_STAT.to_vec(),
        };
        let agents: BTreeMap<i64, String> = self.agents(true)?.into_iter().map(|a| (a.id, a.name)).collect();
        let mut rows: BTreeMap<String, StatRow> = BTreeMap::new();
        // dell'articolo servono solo codice, descrizione, categoria e costo: si leggono senza
        // aprire tutta la scheda (con molti articoli diversi era la parte più lenta)
        let mut product_cache: std::collections::HashMap<i64, Option<Brief>> = std::collections::HashMap::new();
        let mut brief = self.conn().prepare_cached("SELECT code, description, COALESCE(category, ''), json_extract(data, '$.cost') FROM products WHERE id = ?1")?;
        for (id, key) in self.issued_keys(&kinds, from, to)? {
            // ogni documento si rilegge solo se è cambiato dall'ultima statistica
            let cached = self.stat_docs.borrow().get(&id).filter(|(k, _)| *k == key).map(|(_, d)| d.clone());
            let d = match cached {
                Some(d) => d,
                None => {
                    let Some(d) = StatDoc::from(&self.document(id)?) else { continue };
                    let d = std::sync::Arc::new(d);
                    self.stat_docs.borrow_mut().insert(id, (key, d.clone()));
                    d
                }
            };
            let mut touched: Vec<String> = Vec::new();
            if by == StatBy::Project && d.project.is_empty() {
                continue;
            }
            for lc in &d.lines {
                let product = match lc.product_id {
                    Some(id) => product_cache
                        .entry(id)
                        .or_insert_with(|| {
                            brief
                                .query_row([id], |r| Ok(Brief { code: r.get(0)?, description: r.get(1)?, category: r.get(2)?, cost: crate::dec_from_sql(r.get::<_, Option<String>>(3)?) }))
                                .ok()
                        })
                        .clone(),
                    None => None,
                };
                let (key, label) = match by {
                    StatBy::Month => (d.date.format("%Y-%m").to_string(), month_label(d.date)),
                    StatBy::Customer | StatBy::Supplier => (d.party_id.map(|x| x.to_string()).unwrap_or_else(|| "-".into()), if d.party_name.is_empty() { "Vendite al banco".into() } else { d.party_name.clone() }),
                    StatBy::Product => match &product {
                        Some(p) => (p.code.clone(), format!("{}  {}", p.code, p.description)),
                        None => ("-".into(), "Righe senza articolo".into()),
                    },
                    StatBy::Category => {
                        let c = product.as_ref().map(|p| p.category.clone()).unwrap_or_default();
                        let c = if c.trim().is_empty() { "Senza categoria".to_string() } else { c };
                        (c.clone(), c)
                    }
                    StatBy::Project => (d.project.to_lowercase(), d.project.clone()),
                    StatBy::Agent => match d.agent_id {
                        Some(a) => (a.to_string(), agents.get(&a).cloned().unwrap_or_else(|| "(agente eliminato)".into())),
                        None => ("-".into(), "Senza agente".into()),
                    },
                };
                let r = rows.entry(key.clone()).or_insert_with(|| StatRow { key: key.clone(), label, ..Default::default() });
                // commessa: gli acquisti sono costi della commessa
                if by == StatBy::Project && d.purchase {
                    r.cost += lc.revenue;
                    if !touched.contains(&key) {
                        touched.push(key);
                    }
                    continue;
                }
                r.revenue += lc.revenue;
                if lc.item {
                    r.qty += lc.qty;
                    let unit_cost = Some(lc.unit_cost).filter(|c| !c.is_zero()).or_else(|| product.as_ref().map(|p| p.cost)).unwrap_or_default();
                    r.cost += unit_cost * lc.qty;
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

/// Classe ABC (analisi di Pareto) di ogni riga: in ordine di valore decrescente, "A" fino
/// all'80% del totale cumulato, "B" fino al 95%, "C" il resto (e i valori nulli o negativi).
/// Restituisce per chiave la classe e la percentuale cumulata.
pub fn abc_classes(rows: &[StatRow]) -> std::collections::HashMap<String, (char, Decimal)> {
    let mut v: Vec<&StatRow> = rows.iter().collect();
    v.sort_by(|a, b| b.revenue.cmp(&a.revenue));
    let total: Decimal = v.iter().map(|r| r.revenue.max(Decimal::ZERO)).sum();
    let mut out = std::collections::HashMap::new();
    let mut cum = Decimal::ZERO;
    for r in v {
        if r.revenue <= Decimal::ZERO || total.is_zero() {
            out.insert(r.key.clone(), ('C', Decimal::ONE_HUNDRED));
            continue;
        }
        let before = cum / total * Decimal::ONE_HUNDRED;
        cum += r.revenue;
        let class = if before < Decimal::from(80) {
            'A'
        } else if before < Decimal::from(95) {
            'B'
        } else {
            'C'
        };
        out.insert(r.key.clone(), (class, openfatt_core::money::round2(cum / total * Decimal::ONE_HUNDRED)));
    }
    out
}

#[cfg(test)]
mod abc_tests {
    use super::*;
    use openfatt_core::dec;
    use openfatt_core::model::*;

    fn row(k: &str, v: Decimal) -> StatRow {
        StatRow { key: k.into(), label: k.into(), revenue: v, ..Default::default() }
    }

    #[test]
    fn classi_abc_pareto() {
        let rows = vec![row("a", dec!(500)), row("b", dec!(300)), row("c", dec!(100)), row("d", dec!(60)), row("e", dec!(40)), row("z", dec!(-10))];
        let m = abc_classes(&rows);
        assert_eq!(m["a"], ('A', dec!(50.00)));
        // b parte dal 50%: è ancora A anche se porta il cumulato all'80%
        assert_eq!(m["b"].0, 'A');
        assert_eq!(m["c"].0, 'B');
        assert_eq!(m["d"].0, 'B');
        assert_eq!(m["e"].0, 'C');
        assert_eq!(m["z"].0, 'C');
        assert!(abc_classes(&[]).is_empty());
    }

    #[test]
    fn commessa_con_ricavi_e_acquisti() {
        let s = crate::Store::open_in_memory().unwrap();
        let v = VatRef::from_code(&s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap());
        let c = s.save_party(&Party { name: "Committente".into(), ..Default::default() }).unwrap();
        let f = s.save_party(&Party { role: PartyRole::Supplier, name: "Fornitore".into(), ..Default::default() }).unwrap();
        let d = NaiveDate::from_ymd_opt(2026, 5, 10).unwrap();
        let mk = |kind: DocKind, party: i64, amount: Decimal, project: &str| {
            let mut doc = Document::new(kind);
            doc.date = d;
            doc.party_id = Some(party);
            doc.project = project.into();
            doc.external_number = "1".into();
            doc.lines.push(DocLine { description: "Lavori".into(), qty: dec!(1), unit_price: amount, vat: v.clone(), ..Default::default() });
            s.issue_document(s.save_document(&doc).unwrap().id).unwrap();
        };
        mk(DocKind::Invoice, c.id, dec!(10000), "Cantiere Via Roma");
        mk(DocKind::PurchaseInvoice, f.id, dec!(6500), "Cantiere Via Roma");
        mk(DocKind::Invoice, c.id, dec!(999), "");
        let rows = s.statistics(StatBy::Project, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), NaiveDate::from_ymd_opt(2026, 12, 31).unwrap()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Cantiere Via Roma");
        assert_eq!(rows[0].revenue, dec!(10000.00));
        assert_eq!(rows[0].cost, dec!(6500.00));
        assert_eq!(rows[0].margin(), dec!(3500.00));
        assert_eq!(rows[0].documents, 2);
    }
}
