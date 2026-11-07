//! Listini e condizioni particolari per cliente/fornitore.
//!
//! Priorità nel determinare prezzo e sconto di una riga:
//! 1. prezzo o sconto speciale del soggetto per l'articolo (con eventuale quantità minima);
//! 2. sconto speciale del soggetto per la categoria dell'articolo;
//! 3. listino assegnato al soggetto (prezzo specifico dell'articolo con scaglioni di quantità,
//!    altrimenti la formula del listino);
//! 4. prezzo base dell'articolo (prezzo di vendita, o costo per gli acquisti).
//!
//! Lo sconto abituale del soggetto si applica quando nessuna regola indica uno sconto.
//! I prezzi restituiti sono sempre al netto dell'IVA.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::model::{Id, Party, Product};
use crate::money::{pct_of, round_dp};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum PriceListKind {
    /// Prezzi inseriti articolo per articolo
    #[default]
    Manual,
    /// Prezzi calcolati con una formula (con eventuali prezzi specifici che la sostituiscono)
    Derived,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum PriceBase {
    /// Prezzo di vendita dell'articolo
    #[default]
    SalePrice,
    /// Costo d'acquisto dell'articolo
    Cost,
    /// Prezzo di un altro listino
    List(Id),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PriceList {
    pub id: Id,
    pub name: String,
    pub kind: PriceListKind,
    pub base: PriceBase,
    /// Variazione percentuale sulla base: positiva = ricarico, negativa = sconto.
    pub percent: Decimal,
    /// Arrotondamento del prezzo calcolato (0 = nessuno; es. 0,05 o 1).
    pub round_to: Decimal,
    /// I prezzi del listino si intendono IVA inclusa.
    pub vat_included: bool,
    pub valid_from: Option<NaiveDate>,
    pub valid_to: Option<NaiveDate>,
    pub active: bool,
    pub notes: String,
}

impl Default for PriceList {
    fn default() -> Self {
        PriceList {
            id: 0,
            name: String::new(),
            kind: PriceListKind::Derived,
            base: PriceBase::SalePrice,
            percent: Decimal::ZERO,
            round_to: Decimal::ZERO,
            vat_included: false,
            valid_from: None,
            valid_to: None,
            active: true,
            notes: String::new(),
        }
    }
}

impl PriceList {
    pub fn is_valid_on(&self, d: NaiveDate) -> bool {
        self.active && self.valid_from.map(|f| d >= f).unwrap_or(true) && self.valid_to.map(|t| d <= t).unwrap_or(true)
    }
    pub fn describe(&self, base_name: &str) -> String {
        match self.kind {
            PriceListKind::Manual => "prezzi inseriti a mano".into(),
            PriceListKind::Derived => {
                let b = match self.base {
                    PriceBase::SalePrice => "prezzo di vendita".to_string(),
                    PriceBase::Cost => "costo".to_string(),
                    PriceBase::List(_) => format!("listino {}", base_name),
                };
                if self.percent.is_zero() {
                    b
                } else {
                    format!("{} {}{}%", b, if self.percent.is_sign_positive() { "+" } else { "" }, self.percent.normalize())
                }
            }
        }
    }
}

/// Prezzo specifico di un articolo in un listino (con quantità minima per gli scaglioni).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PriceListItem {
    pub list_id: Id,
    pub product_id: Id,
    pub min_qty: Decimal,
    /// Prezzo come inserito (IVA inclusa se il listino è a prezzi ivati).
    pub price: Decimal,
}

/// Condizione particolare di un soggetto: per articolo o per categoria.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PartyPrice {
    pub id: Id,
    pub party_id: Id,
    pub product_id: Option<Id>,
    /// Categoria di articoli (usata se `product_id` è vuoto).
    pub category: String,
    pub min_qty: Decimal,
    /// Prezzo netto speciale; se assente vale il prezzo da listino/base.
    pub price: Option<Decimal>,
    /// Sconti in cascata, es. "10+5".
    pub discount: String,
    pub valid_from: Option<NaiveDate>,
    pub valid_to: Option<NaiveDate>,
    pub note: String,
}

impl PartyPrice {
    pub fn is_valid_on(&self, d: NaiveDate) -> bool {
        self.valid_from.map(|f| d >= f).unwrap_or(true) && self.valid_to.map(|t| d <= t).unwrap_or(true)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PriceQuote {
    /// Prezzo unitario netto IVA
    pub price: Decimal,
    pub discounts: String,
    /// Spiegazione della provenienza (es. "Listino Rivenditori, da 10 pz")
    pub source: String,
}

/// Dati di un listino con i suoi prezzi specifici.
pub struct ListData<'a> {
    pub list: &'a PriceList,
    pub items: &'a [PriceListItem],
}

/// Arrotonda al multiplo più vicino di `step` (step 0 = nessun arrotondamento).
pub fn round_to_step(v: Decimal, step: Decimal) -> Decimal {
    if step <= Decimal::ZERO {
        return v;
    }
    round_dp(v / step, 0) * step
}

fn net_from(price: Decimal, vat_included: bool, vat_rate: Decimal) -> Decimal {
    if vat_included {
        round_dp(price / (Decimal::ONE + vat_rate / Decimal::ONE_HUNDRED), 6)
    } else {
        price
    }
}

/// Prezzo netto di un articolo in un listino, per una quantità. `lookup` fornisce i listini base.
pub fn list_price(
    data: &ListData,
    product: &Product,
    qty: Decimal,
    vat_rate: Decimal,
    lookup: &dyn Fn(Id) -> Option<(PriceList, Vec<PriceListItem>)>,
    depth: u32,
) -> Option<(Decimal, String)> {
    if depth > 4 {
        return None;
    }
    // prezzo specifico: lo scaglione con la quantità minima più alta raggiunta
    let item = data
        .items
        .iter()
        .filter(|i| i.product_id == product.id && i.min_qty <= qty.abs().max(Decimal::ONE))
        .max_by(|a, b| a.min_qty.cmp(&b.min_qty));
    if let Some(i) = item {
        let note = if i.min_qty > Decimal::ONE { format!("Listino {}, da {} pz", data.list.name, i.min_qty.normalize()) } else { format!("Listino {}", data.list.name) };
        return Some((net_from(i.price, data.list.vat_included, vat_rate), note));
    }
    match data.list.kind {
        PriceListKind::Manual => None,
        PriceListKind::Derived => {
            let base = match data.list.base {
                PriceBase::SalePrice => product.price,
                PriceBase::Cost => product.cost,
                PriceBase::List(id) => {
                    let (l, items) = lookup(id)?;
                    list_price(&ListData { list: &l, items: &items }, product, qty, vat_rate, lookup, depth + 1).map(|x| x.0).unwrap_or(product.price)
                }
            };
            if base.is_zero() {
                return None;
            }
            let mut p = base + pct_of(base, data.list.percent);
            if !data.list.round_to.is_zero() {
                if data.list.vat_included {
                    // arrotondamento sul prezzo al pubblico IVA inclusa
                    let gross = round_to_step(p * (Decimal::ONE + vat_rate / Decimal::ONE_HUNDRED), data.list.round_to);
                    p = round_dp(gross / (Decimal::ONE + vat_rate / Decimal::ONE_HUNDRED), 6);
                } else {
                    p = round_to_step(p, data.list.round_to);
                }
            } else {
                p = round_dp(p, 6);
            }
            Some((p, format!("Listino {}", data.list.name)))
        }
    }
}

/// Determina prezzo e sconti di un articolo per un soggetto.
#[allow(clippy::too_many_arguments)]
pub fn quote(
    product: &Product,
    qty: Decimal,
    date: NaiveDate,
    vat_rate: Decimal,
    purchase: bool,
    party: Option<&Party>,
    list: Option<ListData>,
    party_rules: &[PartyPrice],
    lookup: &dyn Fn(Id) -> Option<(PriceList, Vec<PriceListItem>)>,
) -> PriceQuote {
    let q = qty.abs().max(Decimal::ONE);
    let base = if purchase { product.cost } else { product.price };
    let mut out = PriceQuote { price: base, discounts: String::new(), source: if purchase { "Costo d'acquisto".into() } else { "Prezzo base".into() } };

    // 3. listino del soggetto
    if let Some(l) = list.as_ref() {
        if l.list.is_valid_on(date) {
            if let Some((p, note)) = list_price(l, product, q, vat_rate, lookup, 0) {
                out.price = p;
                out.source = note;
            }
        }
    }

    let valid: Vec<&PartyPrice> = party_rules.iter().filter(|r| r.is_valid_on(date) && r.min_qty <= q).collect();
    let mut discount_set = false;
    // 2. sconto per categoria
    if !product.category.trim().is_empty() {
        if let Some(r) = valid
            .iter()
            .filter(|r| r.product_id.is_none() && r.category.trim().eq_ignore_ascii_case(product.category.trim()))
            .max_by(|a, b| a.min_qty.cmp(&b.min_qty))
        {
            if let Some(p) = r.price {
                out.price = p;
            }
            if !r.discount.trim().is_empty() {
                out.discounts = r.discount.trim().to_string();
                discount_set = true;
            }
            out.source = format!("Condizione per la categoria {}", product.category.trim());
        }
    }
    // 1. condizione per l'articolo
    if let Some(r) = valid.iter().filter(|r| r.product_id == Some(product.id)).max_by(|a, b| a.min_qty.cmp(&b.min_qty)) {
        if let Some(p) = r.price {
            out.price = p;
        }
        if !r.discount.trim().is_empty() || r.price.is_some() {
            out.discounts = r.discount.trim().to_string();
            discount_set = true;
        }
        out.source = if r.min_qty > Decimal::ONE { format!("Condizione particolare, da {} pz", r.min_qty.normalize()) } else { "Condizione particolare".into() };
    }
    // sconto abituale del soggetto
    if !discount_set {
        if let Some(p) = party {
            if !p.default_discount.trim().is_empty() {
                out.discounts = p.default_discount.trim().to_string();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dec;

    fn d(m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, dd).unwrap()
    }

    fn product() -> Product {
        Product { id: 7, code: "A".into(), description: "Articolo".into(), price: dec!(100), cost: dec!(60), category: "Vernici".into(), ..Default::default() }
    }

    fn none(_: Id) -> Option<(PriceList, Vec<PriceListItem>)> {
        None
    }

    #[test]
    fn base_price_and_default_discount() {
        let p = product();
        let party = Party { default_discount: "5".into(), ..Default::default() };
        let q = quote(&p, dec!(1), d(1, 1), dec!(22), false, Some(&party), None, &[], &none);
        assert_eq!(q.price, dec!(100));
        assert_eq!(q.discounts, "5");
        let q = quote(&p, dec!(1), d(1, 1), dec!(22), true, None, None, &[], &none);
        assert_eq!(q.price, dec!(60));
    }

    #[test]
    fn derived_list_with_rounding_and_breaks() {
        let p = product();
        let list = PriceList { id: 1, name: "Rivenditori".into(), kind: PriceListKind::Derived, base: PriceBase::SalePrice, percent: dec!(-12.5), round_to: dec!(0.50), ..Default::default() };
        let items = vec![PriceListItem { list_id: 1, product_id: 7, min_qty: dec!(10), price: dec!(80) }];
        let q = quote(&p, dec!(1), d(1, 1), dec!(22), false, None, Some(ListData { list: &list, items: &items }), &[], &none);
        assert_eq!(q.price, dec!(87.50));
        let q = quote(&p, dec!(12), d(1, 1), dec!(22), false, None, Some(ListData { list: &list, items: &items }), &[], &none);
        assert_eq!(q.price, dec!(80));
        assert!(q.source.contains("da 10"));
        // listino scaduto: prezzo base
        let expired = PriceList { valid_to: Some(d(1, 1)), ..list.clone() };
        let q = quote(&p, dec!(1), d(2, 1), dec!(22), false, None, Some(ListData { list: &expired, items: &items }), &[], &none);
        assert_eq!(q.price, dec!(100));
    }

    #[test]
    fn markup_on_cost_and_gross_rounding() {
        let p = product();
        // ingrosso: costo + 30% = 78
        let l = PriceList { id: 2, name: "Ingrosso".into(), base: PriceBase::Cost, percent: dec!(30), ..Default::default() };
        let q = quote(&p, dec!(1), d(1, 1), dec!(22), false, None, Some(ListData { list: &l, items: &[] }), &[], &none);
        assert_eq!(q.price, dec!(78));
        // pubblico IVA inclusa arrotondato a 1 euro: 100 * 1,22 = 122 -> -3% = 118,34 -> 118 ivato
        let pub_l = PriceList { id: 3, name: "Pubblico".into(), percent: dec!(-3), round_to: dec!(1), vat_included: true, ..Default::default() };
        let q = quote(&p, dec!(1), d(1, 1), dec!(22), false, None, Some(ListData { list: &pub_l, items: &[] }), &[], &none);
        assert_eq!(crate::money::round2(q.price * dec!(1.22)), dec!(118.00));
        // listino manuale a prezzi ivati: 61 ivato = 50 netto
        let man = PriceList { id: 4, name: "Manuale".into(), kind: PriceListKind::Manual, vat_included: true, ..Default::default() };
        let items = vec![PriceListItem { list_id: 4, product_id: 7, min_qty: Decimal::ZERO, price: dec!(61) }];
        let q = quote(&p, dec!(1), d(1, 1), dec!(22), false, None, Some(ListData { list: &man, items: &items }), &[], &none);
        assert_eq!(q.price, dec!(50));
    }

    #[test]
    fn list_derived_from_list() {
        let p = product();
        let base_list = PriceList { id: 10, name: "Base".into(), percent: dec!(-10), ..Default::default() };
        let child = PriceList { id: 11, name: "Figlio".into(), base: PriceBase::List(10), percent: dec!(-10), ..Default::default() };
        let lookup = |id: Id| if id == 10 { Some((base_list.clone(), vec![])) } else { None };
        let q = quote(&p, dec!(1), d(1, 1), dec!(22), false, None, Some(ListData { list: &child, items: &[] }), &[], &lookup);
        assert_eq!(q.price, dec!(81));
    }

    #[test]
    fn party_conditions_have_priority() {
        let p = product();
        let party = Party { default_discount: "5".into(), ..Default::default() };
        let list = PriceList { id: 1, name: "Rivenditori".into(), percent: dec!(-10), ..Default::default() };
        let rules = vec![
            PartyPrice { category: "vernici".into(), discount: "20".into(), ..Default::default() },
            PartyPrice { product_id: Some(7), min_qty: dec!(50), price: Some(dec!(70)), ..Default::default() },
            PartyPrice { product_id: Some(99), price: Some(dec!(1)), ..Default::default() },
        ];
        // categoria: prezzo di listino 90 con sconto 20 (lo sconto abituale non si somma)
        let q = quote(&p, dec!(5), d(1, 1), dec!(22), false, Some(&party), Some(ListData { list: &list, items: &[] }), &rules, &none);
        assert_eq!((q.price, q.discounts.as_str()), (dec!(90), "20"));
        // da 50 pezzi: prezzo netto speciale senza sconti
        let q = quote(&p, dec!(60), d(1, 1), dec!(22), false, Some(&party), Some(ListData { list: &list, items: &[] }), &rules, &none);
        assert_eq!((q.price, q.discounts.as_str()), (dec!(70), ""));
        // condizione scaduta: ignorata
        let old = vec![PartyPrice { product_id: Some(7), price: Some(dec!(1)), valid_to: Some(d(1, 1)), ..Default::default() }];
        let q = quote(&p, dec!(1), d(3, 1), dec!(22), false, Some(&party), None, &old, &none);
        assert_eq!((q.price, q.discounts.as_str()), (dec!(100), "5"));
    }

    #[test]
    fn steps() {
        assert_eq!(round_to_step(dec!(12.37), dec!(0.05)), dec!(12.35));
        assert_eq!(round_to_step(dec!(12.38), dec!(0.05)), dec!(12.40));
        assert_eq!(round_to_step(dec!(12.5), dec!(1)), dec!(13));
        assert_eq!(round_to_step(dec!(12.37), dec!(0)), dec!(12.37));
    }
}
