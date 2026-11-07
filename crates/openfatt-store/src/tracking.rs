//! Varianti, lotti con scadenza e numeri di serie: registrazione dei movimenti con i controlli
//! di coerenza, disponibilità e tracciabilità.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rusqlite::params;
use rust_decimal::Decimal;

use openfatt_core::model::{Product, StockMove, Tracking};

use crate::{date_from_sql, dec_from_sql, Result, Store, StoreError};

/// Lotto non indicato nei carichi.
pub const LOT_UNKNOWN: &str = "N.D.";

#[derive(Debug, Clone, PartialEq)]
pub struct LotRow {
    pub variant: String,
    pub lot: String,
    pub expiry: Option<NaiveDate>,
    pub qty: Decimal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraceRow {
    pub date: NaiveDate,
    pub product_code: String,
    pub product_description: String,
    pub variant: String,
    pub lot: String,
    pub serial: String,
    pub expiry: Option<NaiveDate>,
    pub qty: Decimal,
    pub reason: String,
    pub document_id: Option<i64>,
    pub party_name: String,
}

fn line_label(pos: usize) -> String {
    if pos == 0 {
        "Movimento".into()
    } else {
        format!("Riga {}", pos)
    }
}

impl Store {
    /// Registra il movimento di una riga applicando le regole di variante, lotto e seriale.
    /// `pos` è il numero di riga per i messaggi (0 = movimento manuale).
    pub(crate) fn post_tracked_move(&self, product: &Product, base: StockMove, serials: &[String], pos: usize) -> Result<Vec<StockMove>> {
        let label = line_label(pos);
        let mut base = base;
        base.variant = base.variant.trim().to_string();
        if !product.variants.is_empty() {
            if base.variant.is_empty() {
                return Err(StoreError::Invalid(format!("{}: indicare la variante di {} ({})", label, product.code, product.variants.combinations().join(", "))));
            }
            if !product.variants.contains(&base.variant) {
                return Err(StoreError::Invalid(format!("{}: la variante '{}' non esiste per {}", label, base.variant, product.code)));
            }
        } else {
            base.variant.clear();
        }
        if base.warehouse.trim().is_empty() {
            base.warehouse = self.default_warehouse()?;
        }
        let wh = base.warehouse.clone();
        let mut ids: Vec<StockMove> = Vec::new();
        let mut push = |s: &Store, m: StockMove| -> Result<()> {
            let id = s.insert_stock_move(&m)?;
            ids.push(StockMove { id, ..m });
            Ok(())
        };
        match product.tracking {
            Tracking::None => {
                push(self, base)?;
            }
            Tracking::Lot => {
                if base.qty > Decimal::ZERO {
                    if base.lot.is_empty() {
                        base.lot = LOT_UNKNOWN.into();
                    }
                    push(self, base)?;
                } else if base.qty < Decimal::ZERO {
                    let need = -base.qty;
                    let lots = self.lots_available(product.id, Some(&base.variant), Some(&wh))?;
                    if !base.lot.is_empty() {
                        let avail = lots.iter().filter(|l| l.lot == base.lot).map(|l| l.qty).sum::<Decimal>();
                        if avail < need {
                            return Err(StoreError::Invalid(format!("{}: del lotto {} di {} nel magazzino {} sono disponibili {} (richiesti {})", label, base.lot, product.code, wh, avail.normalize(), need.normalize())));
                        }
                        base.expiry = lots.iter().find(|l| l.lot == base.lot).and_then(|l| l.expiry);
                        push(self, base)?;
                    } else {
                        // assegnazione automatica: prima i lotti che scadono prima (FEFO)
                        let total: Decimal = lots.iter().map(|l| l.qty).sum();
                        if total < need {
                            return Err(StoreError::Invalid(format!("{}: di {} nel magazzino {} sono disponibili {} nei lotti (richiesti {})", label, product.code, wh, total.normalize(), need.normalize())));
                        }
                        let mut rest = need;
                        for l in lots {
                            if rest <= Decimal::ZERO {
                                break;
                            }
                            let take = rest.min(l.qty);
                            let mut m = base.clone();
                            m.qty = -take;
                            m.lot = l.lot.clone();
                            m.expiry = l.expiry;
                            push(self, m)?;
                            rest -= take;
                        }
                    }
                }
            }
            Tracking::Serial => {
                let qty = base.qty.abs();
                let list: Vec<String> = serials.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                if qty.fract() != Decimal::ZERO {
                    return Err(StoreError::Invalid(format!("{}: {} è gestito a numeri di serie, la quantità deve essere intera", label, product.code)));
                }
                if Decimal::from(list.len()) != qty {
                    return Err(StoreError::Invalid(format!("{}: indicare {} numeri di serie per {} (indicati {})", label, qty.normalize(), product.code, list.len())));
                }
                let mut seen = std::collections::HashSet::new();
                for s in &list {
                    if !seen.insert(s.clone()) {
                        return Err(StoreError::Invalid(format!("{}: il numero di serie {} è ripetuto", label, s)));
                    }
                }
                let incoming = base.qty > Decimal::ZERO;
                for s in &list {
                    if incoming && self.serial_balance(product.id, s, None)? >= Decimal::ONE {
                        return Err(StoreError::Invalid(format!("{}: il numero di serie {} di {} risulta già a magazzino", label, s, product.code)));
                    }
                    if !incoming && self.serial_balance(product.id, s, Some(&wh))? < Decimal::ONE {
                        return Err(StoreError::Invalid(format!("{}: il numero di serie {} di {} non è nel magazzino {}", label, s, product.code, wh)));
                    }
                    let mut m = base.clone();
                    m.qty = if incoming { Decimal::ONE } else { Decimal::NEGATIVE_ONE };
                    m.serial = s.clone();
                    push(self, m)?;
                }
            }
        }
        Ok(ids)
    }

    fn serial_balance(&self, product_id: i64, serial: &str, warehouse: Option<&str>) -> Result<Decimal> {
        let mut st = self.conn().prepare("SELECT qty FROM stock_moves WHERE product_id = ?1 AND serial = ?2 AND (?3 IS NULL OR warehouse = ?3)")?;
        let rows = st.query_map(params![product_id, serial, warehouse], |r| r.get::<_, String>(0))?;
        let mut t = Decimal::ZERO;
        for r in rows {
            t += dec_from_sql(Some(r?));
        }
        Ok(t)
    }

    /// Lotti con giacenza positiva, ordinati per scadenza (poi per lotto).
    pub fn lots_available(&self, product_id: i64, variant: Option<&str>, warehouse: Option<&str>) -> Result<Vec<LotRow>> {
        let mut st = self.conn().prepare(
            "SELECT variant, lot, expiry, qty FROM stock_moves WHERE product_id = ?1 AND lot <> '' AND (?2 IS NULL OR variant = ?2) AND (?3 IS NULL OR warehouse = ?3)",
        )?;
        let rows = st.query_map(params![product_id, variant, warehouse], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?))
        })?;
        let mut m: BTreeMap<(String, String), (Option<NaiveDate>, Decimal)> = BTreeMap::new();
        for r in rows {
            let (v, lot, exp, q) = r?;
            let e = m.entry((v, lot)).or_insert((None, Decimal::ZERO));
            if let Some(x) = exp.map(|s| date_from_sql(&s)) {
                e.0 = Some(e.0.map(|cur| cur.min(x)).unwrap_or(x));
            }
            e.1 += dec_from_sql(Some(q));
        }
        let mut out: Vec<LotRow> = m.into_iter().filter(|(_, (_, q))| *q > Decimal::ZERO).map(|((variant, lot), (expiry, qty))| LotRow { variant, lot, expiry, qty }).collect();
        out.sort_by(|a, b| match (a.expiry, b.expiry) {
            (Some(x), Some(y)) => x.cmp(&y).then(a.lot.cmp(&b.lot)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.lot.cmp(&b.lot),
        });
        Ok(out)
    }

    /// Numeri di serie a magazzino (con la variante).
    pub fn serials_in_stock(&self, product_id: i64, warehouse: Option<&str>) -> Result<Vec<(String, String)>> {
        let mut st = self.conn().prepare("SELECT serial, variant, qty FROM stock_moves WHERE product_id = ?1 AND serial <> '' AND (?2 IS NULL OR warehouse = ?2)")?;
        let rows = st.query_map(params![product_id, warehouse], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
        let mut m: BTreeMap<String, (String, Decimal)> = BTreeMap::new();
        for r in rows {
            let (s, v, q) = r?;
            let e = m.entry(s).or_insert((v.clone(), Decimal::ZERO));
            e.0 = v;
            e.1 += dec_from_sql(Some(q));
        }
        Ok(m.into_iter().filter(|(_, (_, q))| *q > Decimal::ZERO).map(|(s, (v, _))| (s, v)).collect())
    }

    /// Giacenza per variante.
    pub fn stock_by_variant(&self, product_id: i64) -> Result<Vec<(String, Decimal)>> {
        let mut st = self.conn().prepare("SELECT variant, qty FROM stock_moves WHERE product_id = ?1")?;
        let rows = st.query_map([product_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut m: BTreeMap<String, Decimal> = BTreeMap::new();
        for r in rows {
            let (v, q) = r?;
            *m.entry(v).or_default() += dec_from_sql(Some(q));
        }
        Ok(m.into_iter().collect())
    }

    /// Tracciabilità: tutti i movimenti di un lotto o numero di serie (ricerca esatta).
    pub fn trace(&self, code: &str) -> Result<Vec<TraceRow>> {
        let code = code.trim();
        if code.is_empty() {
            return Ok(vec![]);
        }
        let mut st = self.conn().prepare(
            "SELECT m.date, p.code, p.description, m.variant, m.lot, m.serial, m.expiry, m.qty, m.reason, m.document_id, d.party_name
             FROM stock_moves m JOIN products p ON p.id = m.product_id LEFT JOIN documents d ON d.id = m.document_id
             WHERE m.lot = ?1 OR m.serial = ?1 ORDER BY m.date, m.id",
        )?;
        let rows = st.query_map([code], |r| {
            Ok(TraceRow {
                date: date_from_sql(&r.get::<_, String>(0)?),
                product_code: r.get(1)?,
                product_description: r.get(2)?,
                variant: r.get(3)?,
                lot: r.get(4)?,
                serial: r.get(5)?,
                expiry: r.get::<_, Option<String>>(6)?.map(|s| date_from_sql(&s)),
                qty: dec_from_sql(r.get(7)?),
                reason: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
                document_id: r.get(9)?,
                party_name: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Lotti con giacenza che scadono entro la data indicata (compresi quelli già scaduti).
    pub fn expiring_lots(&self, until: NaiveDate) -> Result<Vec<(Product, LotRow)>> {
        let mut out = Vec::new();
        for p in self.products("", false)?.into_iter().filter(|p| p.tracking == Tracking::Lot) {
            for l in self.lots_available(p.id, None, None)? {
                if l.expiry.map(|e| e <= until).unwrap_or(false) {
                    out.push((p.clone(), l));
                }
            }
        }
        out.sort_by_key(|(_, l)| l.expiry);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;
    use openfatt_core::model::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    fn setup(tracking: Tracking, variants: bool) -> (Store, Party, Product, VatRef) {
        let s = Store::open_in_memory().unwrap();
        let c = s.save_party(&Party { name: "Cliente".into(), vat_number: "12345678903".into(), address: "Via 1".into(), zip: "38122".into(), city: "Trento".into(), ..Default::default() }).unwrap();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap();
        let mut p = Product { code: "X".into(), description: "Articolo".into(), price: dec!(10), vat_id: Some(v.id), tracking, ..Default::default() };
        if variants {
            p.variants = VariantSpec {
                axis1: "Taglia".into(),
                values1: vec!["S".into(), "M".into()],
                axis2: "Colore".into(),
                values2: vec!["Rosso".into(), "Blu".into()],
                barcodes: vec![VariantBarcode { variant: "M/Blu".into(), barcode: "2000000000015".into() }],
            };
        }
        let p = s.save_product(&p).unwrap();
        (s, c, p, VatRef::from_code(&v))
    }

    fn doc(s: &Store, kind: DocKind, party: &Party, date: NaiveDate, lines: Vec<DocLine>) -> ga_store_result::R {
        let mut x = Document::new(kind);
        x.date = date;
        x.party_id = Some(party.id);
        x.lines = lines;
        let saved = s.save_document(&x).unwrap();
        s.issue_document(saved.id)
    }

    mod ga_store_result {
        pub type R = crate::Result<openfatt_core::model::Document>;
    }

    fn line(p: &Product, v: &VatRef, qty: Decimal) -> DocLine {
        DocLine { product_id: Some(p.id), code: p.code.clone(), description: p.description.clone(), qty, unit_price: dec!(10), vat: v.clone(), ..Default::default() }
    }

    #[test]
    fn lots_fefo_and_traceability() {
        let (s, c, p, v) = setup(Tracking::Lot, false);
        let mut l1 = line(&p, &v, dec!(5));
        l1.lot = "L1".into();
        l1.lot_expiry = Some(d(2027, 3, 1));
        let mut l2 = line(&p, &v, dec!(5));
        l2.lot = "L2".into();
        l2.lot_expiry = Some(d(2027, 1, 15));
        doc(&s, DocKind::GoodsReceipt, &c, d(2026, 1, 1), vec![l1, l2]).unwrap();
        // vendita di 7 senza lotto: prima L2 (scade prima), poi 2 da L1
        doc(&s, DocKind::DeliveryNote, &c, d(2026, 1, 2), vec![line(&p, &v, dec!(7))]).unwrap();
        let lots = s.lots_available(p.id, None, None).unwrap();
        assert_eq!(lots.len(), 1);
        assert_eq!(lots[0].lot, "L1");
        assert_eq!(lots[0].qty, dec!(3));
        // lotto indicato ma insufficiente: rifiutato senza effetti parziali
        let mut too_many = line(&p, &v, dec!(4));
        too_many.lot = "L1".into();
        assert!(doc(&s, DocKind::DeliveryNote, &c, d(2026, 1, 3), vec![too_many]).is_err());
        assert_eq!(s.stock_of(p.id).unwrap(), dec!(3));
        // tracciabilità del lotto L2: carico e scarico verso il cliente
        let t = s.trace("L2").unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[1].qty, dec!(-5));
        assert_eq!(t[1].party_name, "Cliente");
        assert_eq!(s.expiring_lots(d(2027, 12, 31)).unwrap().len(), 1);
        // carico senza lotto: lotto "N.D."
        doc(&s, DocKind::GoodsReceipt, &c, d(2026, 1, 4), vec![line(&p, &v, dec!(1))]).unwrap();
        assert!(s.lots_available(p.id, None, None).unwrap().iter().any(|l| l.lot == LOT_UNKNOWN));
    }

    #[test]
    fn f08_serials_sale_and_return() {
        // F-08: vendita e reso del seriale S1 senza duplicare la disponibilità
        let (s, c, p, v) = setup(Tracking::Serial, false);
        let mut inl = line(&p, &v, dec!(2));
        inl.serials = vec!["S1".into(), "S2".into()];
        doc(&s, DocKind::GoodsReceipt, &c, d(2026, 2, 1), vec![inl.clone()]).unwrap();
        // stesso seriale caricato due volte: rifiutato
        let mut dup = line(&p, &v, dec!(1));
        dup.serials = vec!["S1".into()];
        assert!(doc(&s, DocKind::GoodsReceipt, &c, d(2026, 2, 2), vec![dup.clone()]).is_err());
        // numero di seriali diverso dalla quantità: rifiutato
        let mut wrong = line(&p, &v, dec!(2));
        wrong.serials = vec!["S1".into()];
        assert!(doc(&s, DocKind::DeliveryNote, &c, d(2026, 2, 3), vec![wrong]).is_err());
        // vendita di S1
        let mut out = line(&p, &v, dec!(1));
        out.serials = vec!["S1".into()];
        let inv = doc(&s, DocKind::Invoice, &c, d(2026, 2, 4), vec![out.clone()]).unwrap();
        assert_eq!(s.serials_in_stock(p.id, None).unwrap(), vec![("S2".to_string(), String::new())]);
        // S1 non si può vendere due volte
        assert!(doc(&s, DocKind::Invoice, &c, d(2026, 2, 5), vec![out]).is_err());
        // reso con nota di credito: S1 torna disponibile una sola volta
        let nc = s.transform(&[inv.id], DocKind::CreditNote).unwrap();
        assert_eq!(nc.lines[0].serials, vec!["S1".to_string()]);
        s.issue_document(nc.id).unwrap();
        assert_eq!(s.serials_in_stock(p.id, None).unwrap().len(), 2);
        assert_eq!(s.stock_of(p.id).unwrap(), dec!(2));
        assert!(doc(&s, DocKind::GoodsReceipt, &c, d(2026, 2, 6), vec![dup]).is_err());
        let t = s.trace("S1").unwrap();
        assert_eq!(t.iter().map(|r| r.qty).collect::<Vec<_>>(), vec![dec!(1), dec!(-1), dec!(1)]);
    }

    #[test]
    fn variants_required_and_barcodes() {
        let (s, c, p, v) = setup(Tracking::None, true);
        // variante mancante o inesistente: rifiutata
        assert!(doc(&s, DocKind::GoodsReceipt, &c, d(2026, 3, 1), vec![line(&p, &v, dec!(3))]).is_err());
        let mut bad = line(&p, &v, dec!(3));
        bad.variant = "XL/Verde".into();
        assert!(doc(&s, DocKind::GoodsReceipt, &c, d(2026, 3, 1), vec![bad]).is_err());
        let mut a = line(&p, &v, dec!(3));
        a.variant = "M/Blu".into();
        let mut b = line(&p, &v, dec!(2));
        b.variant = "S/Rosso".into();
        doc(&s, DocKind::GoodsReceipt, &c, d(2026, 3, 1), vec![a, b]).unwrap();
        let mut sold = line(&p, &v, dec!(1));
        sold.variant = "M/Blu".into();
        doc(&s, DocKind::DeliveryNote, &c, d(2026, 3, 2), vec![sold]).unwrap();
        let by = s.stock_by_variant(p.id).unwrap();
        assert_eq!(by, vec![("M/Blu".to_string(), dec!(2)), ("S/Rosso".to_string(), dec!(2))]);
        // il codice a barre della variante riconosce articolo e variante
        let (found, variant) = s.find_by_code("2000000000015").unwrap().unwrap();
        assert_eq!(found.id, p.id);
        assert_eq!(variant, "M/Blu");
        // codice a barre già usato da un altro articolo: rifiutato
        let mut other = Product { code: "Y".into(), description: "Altro".into(), ..Default::default() };
        other.variants = VariantSpec { values1: vec!["U".into()], barcodes: vec![VariantBarcode { variant: "U".into(), barcode: "2000000000015".into() }], ..Default::default() };
        assert!(s.save_product(&other).is_err());
    }
}
