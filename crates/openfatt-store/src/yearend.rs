//! Operazioni di fine anno: controlli prima della chiusura, valore del magazzino al
//! 31 dicembre (per il libro degli inventari), copia di sicurezza e blocco dell'anno.
//!
//! La numerazione riparte da sola con il nuovo anno e giacenze, scadenzario e saldi di
//! cassa sono progressivi: non servono riporti manuali.

use chrono::NaiveDate;
use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use openfatt_core::model::{DocKind, DocStatus};

use crate::{date_to_sql, dec_from_sql, now_sql, DocFilter, Result, Store, StoreError};

/// Esito dei controlli di fine anno.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct YearCheck {
    pub year: i32,
    /// Documenti fiscali ancora in bozza
    pub drafts: Vec<String>,
    /// Fatture emesse senza fattura elettronica inviata
    pub not_sent: Vec<String>,
    /// Fatture elettroniche scartate
    pub rejected: Vec<String>,
    /// Vendite al banco non emesse sul registratore
    pub retail_pending: usize,
    /// Inventari fisici ancora aperti
    pub open_inventories: usize,
    /// Articoli con giacenza negativa al 31/12
    pub negative_stock: Vec<String>,
    /// Crediti e debiti aperti al 31/12
    pub open_receivables: Decimal,
    pub open_payables: Decimal,
    /// Valore del magazzino al costo al 31/12
    pub stock_value: Decimal,
    pub stock_items: usize,
}

impl YearCheck {
    /// Problemi che impediscono la chiusura.
    pub fn blocking(&self) -> Vec<String> {
        let mut v = Vec::new();
        if !self.drafts.is_empty() {
            v.push(if self.drafts.len() == 1 { "1 documento fiscale in bozza".into() } else { format!("{} documenti fiscali in bozza", self.drafts.len()) });
        }
        if !self.rejected.is_empty() {
            v.push(format!("{} fatture elettroniche scartate da correggere", self.rejected.len()));
        }
        if self.open_inventories > 0 {
            v.push(if self.open_inventories == 1 { "1 inventario aperto".into() } else { format!("{} inventari aperti", self.open_inventories) });
        }
        v
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct YearClosure {
    pub year: i32,
    pub closed_at: String,
    pub backup: String,
    pub check: YearCheck,
}

impl Store {
    pub fn year_check(&self, year: i32) -> Result<YearCheck> {
        let from = NaiveDate::from_ymd_opt(year, 1, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(year, 12, 31).unwrap();
        let mut c = YearCheck { year, ..Default::default() };
        let fiscal: Vec<DocKind> = DocKind::ALL.iter().copied().filter(|k| k.is_fiscal()).collect();
        for d in self.documents(&DocFilter { kinds: fiscal.clone(), status: Some(DocStatus::Draft), from: Some(from), to: Some(to), ..Default::default() })? {
            c.drafts.push(format!("{} {} del {}", d.kind.label(), d.number_label, d.date.format("%d/%m/%Y")));
        }
        for d in self.documents(&DocFilter { kinds: fiscal, status: Some(DocStatus::Issued), from: Some(from), to: Some(to), ..Default::default() })? {
            match self.einvoice_for_document(d.id)? {
                None => c.not_sent.push(format!("{} {} - {}", d.kind.label(), d.number_label, d.party_name)),
                Some(e) if e.status == crate::EInvoiceStatus::Rejected => c.rejected.push(format!("{} {}", d.kind.label(), d.number_label)),
                Some(e) if e.status == crate::EInvoiceStatus::ToSend => c.not_sent.push(format!("{} {} - {}", d.kind.label(), d.number_label, d.party_name)),
                _ => {}
            }
        }
        c.retail_pending = self
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE kind = 'BAN' AND status = 'EMESSO' AND date >= ?1 AND date <= ?2 AND json_extract(data, '$.retail.rt_status') IN ('Pending', 'Failed')",
                params![date_to_sql(from), date_to_sql(to)],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0) as usize;
        c.open_inventories = self.conn().query_row("SELECT COUNT(*) FROM inventory_sessions WHERE status = 'APERTO' AND date <= ?1", [date_to_sql(to)], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize;
        // giacenze e valore al 31/12 (costo dell'anagrafica)
        let mut st = self.conn().prepare("SELECT product_id, qty FROM stock_moves WHERE date <= ?1")?;
        let rows: Vec<(i64, String)> = st.query_map([date_to_sql(to)], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
        let mut qty: std::collections::HashMap<i64, Decimal> = std::collections::HashMap::new();
        for (p, q) in rows {
            *qty.entry(p).or_default() += dec_from_sql(Some(q));
        }
        for (pid, q) in qty {
            if q.is_zero() {
                continue;
            }
            let Some(p) = self.product(pid)? else { continue };
            if q < Decimal::ZERO {
                c.negative_stock.push(format!("{} {} ({})", p.code, p.description, q.normalize()));
            } else {
                c.stock_value += q * p.cost;
                c.stock_items += 1;
            }
        }
        c.stock_value = openfatt_core::money::round2(c.stock_value);
        for d in self.due_list(true, Some(to))? {
            if d.receivable {
                c.open_receivables += d.residual();
            } else {
                c.open_payables += d.residual();
            }
        }
        Ok(c)
    }

    /// Chiude l'anno: copia di sicurezza nel percorso indicato e blocco dei documenti
    /// dell'anno (non si modificano più, salvo riaprire l'anno).
    pub fn close_year(&self, year: i32, backup_path: &std::path::Path) -> Result<YearClosure> {
        if self.closed_years()?.contains(&year) {
            return Err(StoreError::Invalid(format!("L'anno {} è già chiuso", year)));
        }
        let check = self.year_check(year)?;
        let block = check.blocking();
        if !block.is_empty() {
            return Err(StoreError::Invalid(format!("Prima di chiudere il {}: {}", year, block.join("; "))));
        }
        self.backup_to(backup_path)?;
        let c = YearClosure { year, closed_at: now_sql(), backup: backup_path.to_string_lossy().to_string(), check };
        self.conn().execute("INSERT INTO year_closures(year, closed_at, data) VALUES (?1, ?2, ?3)", params![year, c.closed_at, serde_json::to_string(&c)?])?;
        self.audit("year", year as i64, "close", &c.backup)?;
        Ok(c)
    }

    pub fn reopen_year(&self, year: i32) -> Result<()> {
        self.conn().execute("DELETE FROM year_closures WHERE year = ?1", [year])?;
        self.audit("year", year as i64, "reopen", "")?;
        Ok(())
    }

    pub fn closed_years(&self) -> Result<Vec<i32>> {
        let mut st = self.conn().prepare("SELECT year FROM year_closures ORDER BY year")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn year_closure(&self, year: i32) -> Result<Option<YearClosure>> {
        let d: Option<String> = self.conn().query_row("SELECT data FROM year_closures WHERE year = ?1", [year], |r| r.get(0)).optional()?;
        Ok(match d {
            Some(d) => Some(serde_json::from_str(&d)?),
            None => None,
        })
    }

    pub(crate) fn year_is_closed(&self, year: i32) -> Result<bool> {
        Ok(self.conn().query_row("SELECT COUNT(*) FROM year_closures WHERE year = ?1", [year], |r| r.get::<_, i64>(0))? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;
    use openfatt_core::model::*;

    #[test]
    fn year_end_checks_and_lock() {
        let s = Store::open_in_memory().unwrap();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.rate == dec!(22)).unwrap();
        let c = s.save_party(&Party { name: "Cliente".into(), vat_number: "01234567890".into(), ..Default::default() }).unwrap();
        let p = s.save_product(&Product { code: "A".into(), description: "Articolo".into(), cost: dec!(10), ..Default::default() }).unwrap();
        s.add_manual_stock_move(&StockMove { product_id: p.id, qty: dec!(5), date: NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(), reason: "carico".into(), ..Default::default() }).unwrap();
        let mut d = Document::new(DocKind::Invoice);
        d.date = NaiveDate::from_ymd_opt(2025, 12, 10).unwrap();
        d.party_id = Some(c.id);
        d.lines.push(DocLine { product_id: Some(p.id), description: "x".into(), qty: dec!(2), unit_price: dec!(30), vat: VatRef::from_code(&v), ..Default::default() });
        let draft = s.save_document(&d).unwrap();
        let chk = s.year_check(2025).unwrap();
        assert_eq!(chk.drafts.len(), 1);
        assert_eq!(chk.stock_value, dec!(50));
        let dir = std::env::temp_dir().join(format!("ga-yearend-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bk = dir.join("chiusura2025.gadb");
        assert!(s.close_year(2025, &bk).is_err(), "bozze aperte");
        let issued = s.issue_document(draft.id).unwrap();
        let chk = s.year_check(2025).unwrap();
        assert_eq!(chk.not_sent.len(), 1);
        assert_eq!(chk.stock_value, dec!(30));
        s.close_year(2025, &bk).unwrap();
        assert!(bk.exists());
        assert_eq!(s.closed_years().unwrap(), vec![2025]);
        // anno chiuso: documento non modificabile
        assert!(s.document_lock_reason(issued.id).unwrap().is_some());
        assert!(s.save_document(&issued).is_err());
        s.reopen_year(2025).unwrap();
        assert!(s.document_lock_reason(issued.id).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
