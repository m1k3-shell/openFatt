//! Archivio pulito per ricominciare (nuovo anno, nuova attività, prove): una copia
//! dell'archivio con anagrafiche, articoli, listini e impostazioni ma senza documenti né
//! movimenti. Facoltativi: le giacenze come carico iniziale, i saldi dei conti come saldo
//! iniziale e le scadenze ancora aperte riportate come promemoria in agenda.
//! L'archivio di partenza non si tocca.

use std::path::Path;

use chrono::NaiveDate;
use rusqlite::params;
use rust_decimal::Decimal;

use crate::{date_to_sql, dec_from_sql, Result, Store, StoreError};

#[derive(Debug, Clone, PartialEq)]
pub struct FreshOptions {
    /// Data di partenza (giacenze e saldi iniziali a questa data).
    pub start: NaiveDate,
    pub opening_stock: bool,
    pub opening_balances: bool,
    pub keep_users: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FreshReport {
    pub parties: i64,
    pub products: i64,
    pub stock_rows: usize,
    pub balances: usize,
    /// Scadenze aperte nell'archivio di partenza (da riportare a parte).
    pub open_dues: usize,
}

/// Tabelle svuotate nella copia: documenti e tutto ciò che ne deriva.
/// Punti delle carte fedeltà e credito dei buoni regalo restano (sono dei clienti): i loro
/// movimenti si tengono, senza il collegamento ai documenti.
const CLEARED: [&str; 24] = [
    "payment_allocations",
    "payments",
    "cash_entries",
    "stock_moves",
    "due_dates",
    "doc_lines",
    "einvoice_events",
    "einvoices",
    "bank_batch_items",
    "bank_batches",
    "mail_log",
    "mail_seen",
    "inventory_lines",
    "inventory_sessions",
    "recurring",
    "commission_settlements",
    "year_closures",
    "conservation",
    "f24_payments",
    "edit_locks",
    "sessions",
    "audit",
    "agenda",
    "documents",
];

impl Store {
    /// Crea in `dest` un archivio pulito a partire da questo.
    pub fn create_fresh_archive(&self, dest: &Path, o: &FreshOptions) -> Result<FreshReport> {
        if dest.exists() {
            return Err(StoreError::Invalid(format!("Il file {} esiste già", dest.display())));
        }
        // valori da riportare, letti prima di copiare
        // giacenze per articolo, magazzino, variante, lotto (con scadenza) e numero di serie
        type Key = (i64, String, String, String, Option<String>, String);
        let stock: Vec<(Key, Decimal, Decimal)> = if o.opening_stock {
            let mut st = self.conn().prepare(
                "SELECT m.product_id, COALESCE(m.warehouse, ''), COALESCE(m.variant, ''), COALESCE(m.lot, ''), m.expiry, COALESCE(m.serial, ''), m.qty, COALESCE(json_extract(p.data, '$.cost'), '0')
                 FROM stock_moves m JOIN products p ON p.id = m.product_id",
            )?;
            let rows = st.query_map([], |r| {
                Ok((
                    (r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?),
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })?;
            let mut m: std::collections::BTreeMap<Key, (Decimal, Decimal)> = Default::default();
            for r in rows {
                let (k, q, c) = r?;
                let e = m.entry(k).or_insert((Decimal::ZERO, dec_from_sql(c)));
                e.0 += dec_from_sql(Some(q));
            }
            m.into_iter().filter(|(_, (q, _))| !q.is_zero()).map(|(k, (q, c))| (k, q, c)).collect()
        } else {
            Vec::new()
        };
        let balances: Vec<(String, Decimal)> = if o.opening_balances {
            self.account_balances()?.into_iter().filter(|b| !b.balance.is_zero() && b.account != crate::COMPENSATION_ACCOUNT).map(|b| (b.account, b.balance)).collect()
        } else {
            Vec::new()
        };
        let open_dues = self.due_list(true, None)?.len();
        self.backup_to(dest)?;
        let fresh = Store::open(dest)?;
        {
            let tx = fresh.begin()?;
            for t in CLEARED {
                fresh.conn().execute(&format!("DELETE FROM {}", t), [])?;
            }
            if !o.keep_users {
                fresh.conn().execute("DELETE FROM users", [])?;
            }
            fresh.conn().execute("UPDATE card_moves SET document_id = NULL", [])?;
            fresh.conn().execute("DELETE FROM attachments WHERE entity = 'document'", [])?;
            tx.commit()?;
        }
        let mut report = FreshReport { open_dues, ..Default::default() };
        if !stock.is_empty() {
            // carico iniziale scritto direttamente: le giacenze per variante, lotto e seriale
            // sono già quelle verificate nell'archivio di partenza
            let tx = fresh.begin()?;
            for ((p, w, variant, lot, expiry, serial), q, cost) in &stock {
                fresh.conn().execute(
                    "INSERT INTO stock_moves(date, product_id, qty, warehouse, unit_value, reason, lot, variant, expiry, serial) VALUES (?1, ?2, ?3, ?4, ?5, 'Giacenza iniziale', ?6, ?7, ?8, ?9)",
                    params![date_to_sql(o.start), p, crate::dec_to_sql(*q), w, crate::dec_to_sql(*cost), lot, variant, expiry, serial],
                )?;
                report.stock_rows += 1;
            }
            tx.commit()?;
        }
        for (a, b) in &balances {
            fresh.set_opening_balance(a, o.start, *b)?;
            report.balances += 1;
        }
        report.parties = fresh.conn().query_row("SELECT COUNT(*) FROM parties", [], |r| r.get(0))?;
        report.products = fresh.conn().query_row("SELECT COUNT(*) FROM products", [], |r| r.get(0))?;
        fresh.conn().execute("VACUUM", [])?;
        fresh.audit("archive", 0, "fresh", &format!("archivio pulito dal {}", date_to_sql(o.start)))?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;
    use openfatt_core::model::*;

    #[test]
    fn archivio_pulito_con_giacenze_e_saldi() {
        let dir = std::env::temp_dir().join(format!("openfatt-fresh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src_path = dir.join("vecchio.gadb");
        let dest = dir.join("nuovo.gadb");
        let _ = std::fs::remove_file(&src_path);
        let _ = std::fs::remove_file(&dest);
        let s = Store::open(&src_path).unwrap();
        let v = VatRef::from_code(&s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap());
        let c = s.save_party(&Party { name: "Cliente".into(), ..Default::default() }).unwrap();
        let p = s.save_product(&Product { code: "A".into(), description: "Articolo".into(), manage_stock: true, cost: dec!(4), price: dec!(10), ..Default::default() }).unwrap();
        s.add_manual_stock_move(&StockMove { product_id: p.id, qty: dec!(10.5), date: NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(), ..Default::default() }).unwrap();
        let mut doc = Document::new(DocKind::Invoice);
        doc.party_id = Some(c.id);
        doc.lines.push(DocLine { product_id: Some(p.id), description: "A".into(), qty: dec!(3), unit_price: dec!(10), vat: v.clone(), ..Default::default() });
        s.issue_document(s.save_document(&doc).unwrap().id).unwrap();
        s.add_cash_entry(&CashEntry { date: NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(), description: "Incasso".into(), account: "Cassa".into(), amount: dec!(150), ..Default::default() }).unwrap();
        let start = NaiveDate::from_ymd_opt(2027, 1, 1).unwrap();
        let r = s.create_fresh_archive(&dest, &FreshOptions { start, opening_stock: true, opening_balances: true, keep_users: true }).unwrap();
        assert_eq!(r.parties, 1);
        assert_eq!(r.products, 1);
        assert_eq!(r.stock_rows, 1);
        assert_eq!(r.balances, 1);
        assert_eq!(r.open_dues, 1);
        // il vecchio archivio è intatto
        assert_eq!(s.documents(&crate::DocFilter::default()).unwrap().len(), 1);
        assert!(s.create_fresh_archive(&dest, &FreshOptions { start, opening_stock: false, opening_balances: false, keep_users: true }).is_err());
        drop(s);
        let n = Store::open(&dest).unwrap();
        assert!(n.documents(&crate::DocFilter::default()).unwrap().is_empty());
        assert_eq!(n.stock_of(p.id).unwrap(), dec!(7.5));
        assert_eq!(n.balance_at("Cassa", start).unwrap(), dec!(150));
        assert!(n.due_list(false, None).unwrap().is_empty());
        // la numerazione riparte da 1
        let mut d2 = Document::new(DocKind::Invoice);
        d2.party_id = Some(c.id);
        d2.date = start;
        d2.lines.push(DocLine { description: "B".into(), qty: dec!(1), unit_price: dec!(1), vat: v, ..Default::default() });
        assert_eq!(n.issue_document(n.save_document(&d2).unwrap().id).unwrap().number, Some(1));
        drop(n);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
