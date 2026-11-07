//! Magazzini dell'azienda e giacenze per magazzino.

use std::collections::BTreeMap;

use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;

use openfatt_core::model::Warehouse;

use crate::{dec_from_sql, Result, Store, StoreError};

pub const DEFAULT_WAREHOUSE: &str = "Principale";

impl Store {
    pub(crate) fn seed_warehouses(&self) -> Result<()> {
        let n: i64 = self.conn().query_row("SELECT COUNT(*) FROM warehouses", [], |r| r.get(0))?;
        if n > 0 {
            return Ok(());
        }
        // magazzini già usati nei movimenti (archivi precedenti) più quello predefinito
        let mut st = self.conn().prepare("SELECT DISTINCT warehouse FROM stock_moves WHERE warehouse <> ''")?;
        let mut names: Vec<String> = st.query_map([], |r| r.get::<_, String>(0))?.filter_map(|r| r.ok()).collect();
        if !names.iter().any(|n| n == DEFAULT_WAREHOUSE) {
            names.insert(0, DEFAULT_WAREHOUSE.into());
        }
        for n in names {
            let w = Warehouse { name: n.clone(), is_default: n == DEFAULT_WAREHOUSE, active: true, ..Default::default() };
            self.conn().execute("INSERT INTO warehouses(name, data) VALUES (?1, ?2)", params![n, serde_json::to_string(&w)?])?;
        }
        Ok(())
    }

    pub fn warehouses(&self) -> Result<Vec<Warehouse>> {
        let mut st = self.conn().prepare("SELECT id, data FROM warehouses ORDER BY name COLLATE NOCASE")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            let (id, data) = r?;
            let mut w: Warehouse = serde_json::from_str(&data)?;
            w.id = id;
            out.push(w);
        }
        // il predefinito per primo
        out.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(out)
    }

    /// Nome del magazzino predefinito.
    pub fn default_warehouse(&self) -> Result<String> {
        Ok(self.warehouses()?.into_iter().find(|w| w.is_default).map(|w| w.name).unwrap_or_else(|| DEFAULT_WAREHOUSE.into()))
    }

    /// Nome effettivo: quello indicato se esiste, altrimenti il predefinito.
    pub fn resolve_warehouse(&self, name: &str) -> Result<String> {
        let n = name.trim();
        if n.is_empty() {
            return self.default_warehouse();
        }
        if self.warehouses()?.iter().any(|w| w.name == n) {
            Ok(n.to_string())
        } else {
            Err(StoreError::Invalid(format!("Il magazzino '{}' non esiste", n)))
        }
    }

    /// Inserisce o aggiorna un magazzino. La ridenominazione aggiorna movimenti e documenti.
    pub fn save_warehouse(&self, w: &Warehouse) -> Result<Warehouse> {
        let mut w = w.clone();
        w.name = w.name.trim().to_string();
        if w.name.is_empty() {
            return Err(StoreError::Invalid("Indicare il nome del magazzino".into()));
        }
        let clash: Option<i64> = self.conn().query_row("SELECT id FROM warehouses WHERE name = ?1 AND id <> ?2", params![w.name, w.id], |r| r.get(0)).optional()?;
        if clash.is_some() {
            return Err(StoreError::Invalid(format!("Esiste già un magazzino '{}'", w.name)));
        }
        let tx = self.begin()?;
        if w.is_default {
            // un solo predefinito
            for mut other in self.warehouses()?.into_iter().filter(|o| o.id != w.id && o.is_default) {
                other.is_default = false;
                self.conn().execute("UPDATE warehouses SET data = ?1 WHERE id = ?2", params![serde_json::to_string(&other)?, other.id])?;
            }
        }
        if w.id == 0 {
            w.active = true;
            self.conn().execute("INSERT INTO warehouses(name, data) VALUES (?1, ?2)", params![w.name, serde_json::to_string(&w)?])?;
            w.id = self.conn().last_insert_rowid();
        } else {
            let old: String = self.conn().query_row("SELECT name FROM warehouses WHERE id = ?1", [w.id], |r| r.get(0))?;
            if old != w.name {
                self.conn().execute("UPDATE stock_moves SET warehouse = ?1 WHERE warehouse = ?2", params![w.name, old])?;
                self.conn().execute(
                    "UPDATE documents SET data = json_set(data, '$.warehouse', ?1) WHERE json_extract(data, '$.warehouse') = ?2",
                    params![w.name, old],
                )?;
                self.conn().execute(
                    "UPDATE documents SET data = json_set(data, '$.to_warehouse', ?1) WHERE json_extract(data, '$.to_warehouse') = ?2",
                    params![w.name, old],
                )?;
            }
            self.conn().execute("UPDATE warehouses SET name = ?1, data = ?2 WHERE id = ?3", params![w.name, serde_json::to_string(&w)?, w.id])?;
        }
        if !self.warehouses()?.iter().any(|x| x.is_default) {
            return Err(StoreError::Invalid("Deve esserci un magazzino predefinito".into()));
        }
        self.audit("warehouse", w.id, "save", &w.name)?;
        tx.commit()?;
        Ok(w)
    }

    pub fn delete_warehouse(&self, id: i64) -> Result<()> {
        let w = self.warehouses()?.into_iter().find(|w| w.id == id).ok_or_else(|| StoreError::NotFound("magazzino".into()))?;
        if w.is_default {
            return Err(StoreError::Invalid("Il magazzino predefinito non si può eliminare".into()));
        }
        let used: i64 = self.conn().query_row("SELECT COUNT(*) FROM stock_moves WHERE warehouse = ?1", [&w.name], |r| r.get(0))?;
        if used > 0 {
            return Err(StoreError::Invalid(format!("Il magazzino ha {} movimenti: si può solo disattivare", used)));
        }
        self.conn().execute("DELETE FROM warehouses WHERE id = ?1", [id])?;
        self.audit("warehouse", id, "delete", &w.name)?;
        Ok(())
    }

    /// Giacenza di un articolo in ciascun magazzino.
    pub fn stock_by_warehouse(&self, product_id: i64) -> Result<Vec<(String, Decimal)>> {
        let mut st = self.conn().prepare("SELECT warehouse, qty FROM stock_moves WHERE product_id = ?1")?;
        let rows = st.query_map([product_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut m: BTreeMap<String, Decimal> = BTreeMap::new();
        for r in rows {
            let (w, q) = r?;
            *m.entry(w).or_default() += dec_from_sql(Some(q));
        }
        Ok(m.into_iter().collect())
    }

    /// Giacenza di un articolo in un magazzino.
    pub fn stock_in(&self, product_id: i64, warehouse: &str) -> Result<Decimal> {
        Ok(self.stock_by_warehouse(product_id)?.into_iter().find(|(w, _)| w == warehouse).map(|(_, q)| q).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use openfatt_core::dec;
    use openfatt_core::model::*;

    use super::*;

    fn d(dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, dd).unwrap()
    }

    fn setup(tracking: Tracking) -> (Store, Product, Party) {
        let s = Store::open_in_memory().unwrap();
        s.save_warehouse(&Warehouse { name: "Negozio".into(), ..Default::default() }).unwrap();
        let p = s.save_product(&Product { code: "P".into(), description: "Prodotto".into(), tracking, ..Default::default() }).unwrap();
        let c = s.save_party(&Party { name: "Cliente".into(), ..Default::default() }).unwrap();
        (s, p, c)
    }

    fn transfer(s: &Store, p: &Product, qty: Decimal, from: &str, to: &str, edit: impl FnOnce(&mut DocLine)) -> Result<Document> {
        let mut t = Document::new(DocKind::StockTransfer);
        t.date = d(2);
        t.warehouse = from.into();
        t.to_warehouse = to.into();
        let mut l = DocLine { product_id: Some(p.id), code: p.code.clone(), description: p.description.clone(), qty, ..Default::default() };
        edit(&mut l);
        t.lines.push(l);
        let t = s.save_document(&t)?;
        s.issue_document(t.id)
    }

    #[test]
    fn f06_transfer_between_warehouses() {
        // F-06: deposito A 10, B 0; trasferire 3 => A 7, B 3, totale 10
        let (s, p, _) = setup(Tracking::None);
        assert_eq!(s.default_warehouse().unwrap(), DEFAULT_WAREHOUSE);
        s.add_manual_stock_move(&StockMove { product_id: p.id, qty: dec!(10), date: d(1), ..Default::default() }).unwrap();
        let t = transfer(&s, &p, dec!(3), "", "Negozio", |_| {}).unwrap();
        assert_eq!(s.stock_in(p.id, DEFAULT_WAREHOUSE).unwrap(), dec!(7));
        assert_eq!(s.stock_in(p.id, "Negozio").unwrap(), dec!(3));
        assert_eq!(s.stock_of(p.id).unwrap(), dec!(10));
        // stessa destinazione dell'origine: rifiutato
        assert!(transfer(&s, &p, dec!(1), "Negozio", "Negozio", |_| {}).is_err());
        // quantità oltre la giacenza d'origine: rifiutato senza effetti parziali
        assert!(transfer(&s, &p, dec!(5), "Negozio", DEFAULT_WAREHOUSE, |_| {}).is_err());
        assert_eq!(s.stock_in(p.id, "Negozio").unwrap(), dec!(3));
        // annullando il trasferimento si torna alla situazione iniziale
        s.cancel_document(t.id).unwrap();
        assert_eq!(s.stock_in(p.id, DEFAULT_WAREHOUSE).unwrap(), dec!(10));
        assert_eq!(s.stock_in(p.id, "Negozio").unwrap(), dec!(0));
        // rapporto per magazzino
        let rep = s.stock_report("", Some("Negozio")).unwrap();
        assert_eq!(rep[0].stock, dec!(0));
    }

    #[test]
    fn documents_move_their_warehouse_and_lots_stay_local() {
        let (s, p, c) = setup(Tracking::Lot);
        let mut arr = Document::new(DocKind::GoodsReceipt);
        arr.date = d(1);
        arr.party_id = Some(c.id);
        arr.warehouse = "Negozio".into();
        arr.lines.push(DocLine { product_id: Some(p.id), qty: dec!(4), lot: "L-NEG".into(), lot_expiry: Some(d(30)), description: "x".into(), vat: VatRef { rate: dec!(22), ..Default::default() }, ..Default::default() });
        let arr = s.save_document(&arr).unwrap();
        s.issue_document(arr.id).unwrap();
        s.add_manual_stock_move(&StockMove { product_id: p.id, qty: dec!(5), lot: "L-MAG".into(), expiry: Some(d(10)), date: d(1), ..Default::default() }).unwrap();
        // vendita dal negozio: prende il lotto del negozio anche se nel principale ne scade uno prima
        let mut ddt = Document::new(DocKind::DeliveryNote);
        ddt.date = d(3);
        ddt.party_id = Some(c.id);
        ddt.warehouse = "Negozio".into();
        ddt.lines.push(DocLine { product_id: Some(p.id), qty: dec!(2), description: "x".into(), vat: VatRef { rate: dec!(22), ..Default::default() }, ..Default::default() });
        let ddt = s.save_document(&ddt).unwrap();
        s.issue_document(ddt.id).unwrap();
        let neg = s.lots_available(p.id, None, Some("Negozio")).unwrap();
        assert_eq!((neg[0].lot.as_str(), neg[0].qty), ("L-NEG", dec!(2)));
        // trasferimento del lotto dal principale al negozio: lotto e scadenza seguono la merce
        transfer(&s, &p, dec!(3), DEFAULT_WAREHOUSE, "Negozio", |_| {}).unwrap();
        let neg = s.lots_available(p.id, None, Some("Negozio")).unwrap();
        assert_eq!(neg[0].lot, "L-MAG");
        assert_eq!(neg[0].expiry, Some(d(10)));
        assert_eq!(neg[0].qty, dec!(3));
        // magazzino inesistente: rifiutato
        let mut bad = Document::new(DocKind::DeliveryNote);
        bad.party_id = Some(c.id);
        bad.warehouse = "Inesistente".into();
        bad.lines.push(DocLine { product_id: Some(p.id), qty: dec!(1), description: "x".into(), vat: VatRef { rate: dec!(22), ..Default::default() }, ..Default::default() });
        let bad = s.save_document(&bad).unwrap();
        assert!(s.issue_document(bad.id).is_err());
    }

    #[test]
    fn serial_transfer_and_rename() {
        let (s, p, _) = setup(Tracking::Serial);
        s.add_manual_stock_move(&StockMove { product_id: p.id, qty: dec!(2), serial: "A1, A2".into(), date: d(1), ..Default::default() }).unwrap();
        // il seriale deve essere nel magazzino d'origine
        assert!(transfer(&s, &p, dec!(1), "Negozio", DEFAULT_WAREHOUSE, |l| l.serials = vec!["A1".into()]).is_err());
        transfer(&s, &p, dec!(1), DEFAULT_WAREHOUSE, "Negozio", |l| l.serials = vec!["A1".into()]).unwrap();
        assert_eq!(s.serials_in_stock(p.id, Some("Negozio")).unwrap(), vec![("A1".to_string(), String::new())]);
        assert_eq!(s.serials_in_stock(p.id, Some(DEFAULT_WAREHOUSE)).unwrap().len(), 1);
        // ridenominazione: movimenti e documenti seguono il nuovo nome
        let mut w = s.warehouses().unwrap().into_iter().find(|w| w.name == "Negozio").unwrap();
        w.name = "Negozio centro".into();
        s.save_warehouse(&w).unwrap();
        assert_eq!(s.serials_in_stock(p.id, Some("Negozio centro")).unwrap().len(), 1);
        // un magazzino con movimenti non si elimina; il predefinito nemmeno
        assert!(s.delete_warehouse(w.id).is_err());
        let main = s.warehouses().unwrap().into_iter().find(|w| w.is_default).unwrap();
        assert!(s.delete_warehouse(main.id).is_err());
        // cambio del predefinito: uno solo resta tale
        w.is_default = true;
        s.save_warehouse(&w).unwrap();
        assert_eq!(s.default_warehouse().unwrap(), "Negozio centro");
        assert_eq!(s.warehouses().unwrap().iter().filter(|x| x.is_default).count(), 1);
    }
}
