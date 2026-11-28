//! Riordino automatico degli articoli sotto scorta: proposta di ordini a fornitore.
//!
//! Per ogni articolo a magazzino con scorta minima, la disponibilità prevista è giacenza
//! meno impegnato dai clienti più merce già in arrivo dai fornitori. Sotto la scorta minima
//! si propone di riordinare quanto serve a tornarci, arrotondato al multiplo d'ordine
//! dell'articolo. Le proposte si raggruppano per fornitore, col codice e il prezzo di quel
//! fornitore, e diventano ordini a fornitore in bozza.

use std::collections::BTreeMap;

use rust_decimal::Decimal;

use openfatt_core::model::*;

use crate::{Result, Store};

/// Articolo da riordinare.
#[derive(Debug, Clone, PartialEq)]
pub struct ReorderLine {
    pub product_id: i64,
    pub code: String,
    pub description: String,
    pub unit: String,
    pub stock: Decimal,
    pub committed: Decimal,
    pub incoming: Decimal,
    pub min_stock: Decimal,
    /// Disponibile previsto (giacenza - impegnato + in arrivo)
    pub expected: Decimal,
    /// Quantità proposta (modificabile prima di creare gli ordini)
    pub qty: Decimal,
    pub supplier_id: Option<i64>,
    pub supplier_name: String,
    pub supplier_code: String,
    pub price: Decimal,
    pub vat_id: Option<i64>,
}

/// Quantità da ordinare per tornare alla scorta minima, al multiplo d'ordine.
pub fn reorder_qty(expected: Decimal, min_stock: Decimal, multiple: Decimal) -> Decimal {
    let need = min_stock - expected;
    if need <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    if multiple > Decimal::ZERO {
        (need / multiple).ceil() * multiple
    } else {
        need.ceil()
    }
}

impl Store {
    /// Articoli sotto scorta con la quantità da riordinare.
    pub fn reorder_proposals(&self) -> Result<Vec<ReorderLine>> {
        let mut names: BTreeMap<i64, String> = BTreeMap::new();
        let mut out = Vec::new();
        for r in self.stock_report("", None)? {
            let p = &r.product;
            if !p.active || !p.manage_stock || p.min_stock <= Decimal::ZERO || p.kind != ProductKind::default() {
                continue;
            }
            let expected = r.stock - r.ordered_by_customers + r.incoming;
            let qty = reorder_qty(expected, p.min_stock, p.order_multiple);
            if qty.is_zero() {
                continue;
            }
            let supplier_name = match p.supplier_id {
                Some(id) => names.entry(id).or_insert_with(|| self.party(id).ok().flatten().map(|s| s.display_name()).unwrap_or_default()).clone(),
                None => String::new(),
            };
            let price = if p.supplier_price > Decimal::ZERO { p.supplier_price } else { p.cost };
            out.push(ReorderLine {
                product_id: p.id,
                code: p.code.clone(),
                description: p.description.clone(),
                unit: p.unit.clone(),
                stock: r.stock,
                committed: r.ordered_by_customers,
                incoming: r.incoming,
                min_stock: p.min_stock,
                expected,
                qty,
                supplier_id: p.supplier_id,
                supplier_name,
                supplier_code: p.supplier_code.clone(),
                price,
                vat_id: p.vat_id,
            });
        }
        out.sort_by(|a, b| a.supplier_name.to_lowercase().cmp(&b.supplier_name.to_lowercase()).then(a.code.cmp(&b.code)));
        Ok(out)
    }

    /// Cambia il fornitore di una proposta: codice e prezzo di quel fornitore, se l'articolo
    /// ne ha uno fra gli "altri fornitori".
    pub fn reorder_with_supplier(&self, line: &ReorderLine, supplier_id: i64) -> Result<ReorderLine> {
        let mut l = line.clone();
        let p = self.product(line.product_id)?.ok_or_else(|| crate::StoreError::NotFound("articolo".into()))?;
        l.supplier_id = Some(supplier_id);
        l.supplier_name = self.party(supplier_id)?.map(|s| s.display_name()).unwrap_or_default();
        if p.supplier_id == Some(supplier_id) {
            l.supplier_code = p.supplier_code.clone();
            l.price = if p.supplier_price > Decimal::ZERO { p.supplier_price } else { p.cost };
        } else if let Some(o) = p.other_suppliers.iter().find(|o| o.supplier_id == supplier_id) {
            l.supplier_code = o.code.clone();
            l.price = if o.price > Decimal::ZERO { o.price } else { p.cost };
        } else {
            l.supplier_code = String::new();
            l.price = p.cost;
        }
        Ok(l)
    }

    /// Crea un ordine a fornitore in bozza per ogni fornitore delle righe con quantità.
    /// Le righe senza fornitore si saltano (restituite a parte).
    pub fn create_reorder_orders(&self, lines: &[ReorderLine]) -> Result<(Vec<Document>, Vec<ReorderLine>)> {
        let vats = self.vat_codes()?;
        let mut by_supplier: BTreeMap<i64, Vec<&ReorderLine>> = BTreeMap::new();
        let mut skipped = Vec::new();
        for l in lines.iter().filter(|l| l.qty > Decimal::ZERO) {
            match l.supplier_id {
                Some(s) => by_supplier.entry(s).or_default().push(l),
                None => skipped.push(l.clone()),
            }
        }
        let tx = self.begin()?;
        let mut docs = Vec::new();
        for (supplier, rows) in by_supplier {
            let Some(party) = self.party(supplier)? else { continue };
            let mut d = Document::new(DocKind::SupplierOrder);
            d.party_id = Some(supplier);
            d.payment_id = party.default_payment_id;
            d.party = party;
            d.internal_notes = "Riordino degli articoli sotto scorta".into();
            for l in rows {
                let vat = l.vat_id.and_then(|id| vats.iter().find(|v| v.id == id)).or_else(|| vats.iter().find(|v| v.code == "22")).map(VatRef::from_code).unwrap_or_default();
                d.lines.push(DocLine {
                    product_id: Some(l.product_id),
                    code: l.code.clone(),
                    description: l.description.clone(),
                    qty: l.qty,
                    unit: l.unit.clone(),
                    unit_price: l.price,
                    unit_cost: l.price,
                    supplier_code: l.supplier_code.clone(),
                    vat,
                    ..Default::default()
                });
            }
            docs.push(self.save_document(&d)?);
        }
        tx.commit()?;
        Ok((docs, skipped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;

    #[test]
    fn quantita_al_multiplo_d_ordine() {
        assert_eq!(reorder_qty(dec!(3), dec!(10), dec!(0)), dec!(7));
        assert_eq!(reorder_qty(dec!(3), dec!(10), dec!(6)), dec!(12));
        assert_eq!(reorder_qty(dec!(-2), dec!(10), dec!(5)), dec!(15));
        assert_eq!(reorder_qty(dec!(10), dec!(10), dec!(5)), dec!(0));
        assert_eq!(reorder_qty(dec!(2.5), dec!(4), dec!(0)), dec!(2));
    }

    #[test]
    fn ordini_per_fornitore_con_impegnato_e_in_arrivo() {
        let s = Store::open_in_memory().unwrap();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap();
        let f1 = s.save_party(&Party { role: PartyRole::Supplier, name: "Fornitore Uno".into(), ..Default::default() }).unwrap();
        let f2 = s.save_party(&Party { role: PartyRole::Supplier, name: "Fornitore Due".into(), ..Default::default() }).unwrap();
        let c = s.save_party(&Party { name: "Cliente".into(), ..Default::default() }).unwrap();
        let mk = |code: &str, min: Decimal, mult: Decimal, sup: Option<i64>, stock: Decimal| {
            let p = s
                .save_product(&Product {
                    code: code.into(),
                    description: format!("Articolo {}", code),
                    manage_stock: true,
                    min_stock: min,
                    order_multiple: mult,
                    supplier_id: sup,
                    supplier_code: format!("F-{}", code),
                    supplier_price: dec!(4),
                    cost: dec!(5),
                    vat_id: Some(v.id),
                    other_suppliers: vec![ProductSupplier { supplier_id: f2.id, code: format!("D-{}", code), price: dec!(3.5), notes: String::new() }],
                    ..Default::default()
                })
                .unwrap();
            if !stock.is_zero() {
                s.add_manual_stock_move(&StockMove { product_id: p.id, qty: stock, ..Default::default() }).unwrap();
            }
            p
        };
        let a = mk("A", dec!(10), dec!(6), Some(f1.id), dec!(8));
        let b = mk("B", dec!(5), dec!(0), Some(f1.id), dec!(20));
        let _c = mk("C", dec!(4), dec!(0), None, dec!(1));
        let ok = mk("D", dec!(3), dec!(0), Some(f1.id), dec!(5));
        // impegno del cliente su B: 18 pezzi, ne restano 2 disponibili
        let mut o = Document::new(DocKind::CustomerOrder);
        o.party_id = Some(c.id);
        o.lines.push(DocLine { product_id: Some(b.id), code: "B".into(), description: "B".into(), qty: dec!(18), unit_price: dec!(10), vat: VatRef::from_code(&v), ..Default::default() });
        s.issue_document(s.save_document(&o).unwrap().id).unwrap();
        let props = s.reorder_proposals().unwrap();
        let get = |id: i64| props.iter().find(|l| l.product_id == id).cloned();
        // A: 8 disponibili, minimo 10, multiplo 6 -> 6
        assert_eq!(get(a.id).unwrap().qty, dec!(6));
        // B: 20 - 18 = 2, minimo 5 -> 3
        let lb = get(b.id).unwrap();
        assert_eq!(lb.expected, dec!(2));
        assert_eq!(lb.qty, dec!(3));
        assert!(get(ok.id).is_none());
        // ordine a fornitore già emesso per A: la merce in arrivo copre la scorta
        let mut po = Document::new(DocKind::SupplierOrder);
        po.party_id = Some(f1.id);
        po.lines.push(DocLine { product_id: Some(a.id), code: "A".into(), description: "A".into(), qty: dec!(6), unit_price: dec!(4), vat: VatRef::from_code(&v), ..Default::default() });
        let po = s.issue_document(s.save_document(&po).unwrap().id).unwrap();
        let props = s.reorder_proposals().unwrap();
        assert!(props.iter().all(|l| l.product_id != a.id), "{:?}", props);
        s.delete_document(po.id).unwrap_or(());
        // B dal secondo fornitore: suo codice e suo prezzo
        let lb2 = s.reorder_with_supplier(&lb, f2.id).unwrap();
        assert_eq!(lb2.supplier_code, "D-B");
        assert_eq!(lb2.price, dec!(3.5));
        let props = s.reorder_proposals().unwrap();
        let mut lines: Vec<ReorderLine> = props.into_iter().map(|l| if l.product_id == b.id { lb2.clone() } else { l }).collect();
        lines.iter_mut().for_each(|l| if l.code == "A" { l.qty = dec!(12) });
        let (docs, skipped) = s.create_reorder_orders(&lines).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].code, "C");
        let d1 = docs.iter().find(|d| d.party_id == Some(f1.id)).unwrap();
        assert_eq!(d1.kind, DocKind::SupplierOrder);
        assert_eq!(d1.status, DocStatus::Draft);
        assert_eq!(d1.lines.len(), 1);
        assert_eq!(d1.lines[0].qty, dec!(12));
        assert_eq!(d1.lines[0].supplier_code, "F-A");
        let d2 = docs.iter().find(|d| d.party_id == Some(f2.id)).unwrap();
        assert_eq!(d2.lines[0].unit_price, dec!(3.5));
    }
}
