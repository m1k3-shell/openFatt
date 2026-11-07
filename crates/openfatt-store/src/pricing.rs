//! Listini, prezzi specifici e condizioni particolari dei soggetti.

use std::str::FromStr;

use chrono::NaiveDate;
use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;

use openfatt_core::model::{Party, Product};
use openfatt_core::pricing::{self, ListData, PartyPrice, PriceBase, PriceList, PriceListItem, PriceQuote};

use crate::{dec_to_sql, Result, Store, StoreError};

impl Store {
    pub fn price_lists(&self) -> Result<Vec<PriceList>> {
        let mut st = self.conn().prepare("SELECT id, data FROM price_lists ORDER BY name COLLATE NOCASE")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            let (id, data) = r?;
            let mut l: PriceList = serde_json::from_str(&data)?;
            l.id = id;
            out.push(l);
        }
        Ok(out)
    }

    pub fn price_list(&self, id: i64) -> Result<Option<PriceList>> {
        Ok(self.price_lists()?.into_iter().find(|l| l.id == id))
    }

    pub fn save_price_list(&self, l: &PriceList) -> Result<PriceList> {
        let mut l = l.clone();
        if l.name.trim().is_empty() {
            return Err(StoreError::Invalid("Indicare il nome del listino".into()));
        }
        if let PriceBase::List(b) = l.base {
            if b == l.id && l.id != 0 {
                return Err(StoreError::Invalid("Un listino non può essere calcolato da se stesso".into()));
            }
            // evita catene circolari
            let mut seen = vec![l.id];
            let mut cur = b;
            for _ in 0..10 {
                if seen.contains(&cur) {
                    return Err(StoreError::Invalid("Catena di listini circolare".into()));
                }
                seen.push(cur);
                match self.price_list(cur)?.map(|x| x.base) {
                    Some(PriceBase::List(n)) => cur = n,
                    _ => break,
                }
            }
        }
        let data = serde_json::to_string(&l)?;
        if l.id == 0 {
            self.conn().execute("INSERT INTO price_lists(name, data) VALUES (?1, ?2)", params![l.name.trim(), data])?;
            l.id = self.conn().last_insert_rowid();
        } else {
            self.conn().execute("UPDATE price_lists SET name = ?1, data = ?2 WHERE id = ?3", params![l.name.trim(), data, l.id])?;
        }
        self.audit("price_list", l.id, "save", &l.name)?;
        Ok(l)
    }

    pub fn delete_price_list(&self, id: i64) -> Result<()> {
        let used = self.parties(&crate::PartyFilter { include_inactive: true, ..Default::default() })?.iter().filter(|p| p.price_list_id == Some(id)).count();
        if used > 0 {
            return Err(StoreError::Invalid(format!("Il listino è assegnato a {} soggetti", used)));
        }
        if self.price_lists()?.iter().any(|l| l.base == PriceBase::List(id)) {
            return Err(StoreError::Invalid("Altri listini sono calcolati da questo".into()));
        }
        self.conn().execute("DELETE FROM price_lists WHERE id = ?1", [id])?;
        self.audit("price_list", id, "delete", "")?;
        Ok(())
    }

    pub fn price_list_items(&self, list_id: i64) -> Result<Vec<PriceListItem>> {
        let mut st = self.conn().prepare("SELECT product_id, min_qty, price FROM price_list_items WHERE list_id = ?1 ORDER BY product_id, CAST(min_qty AS REAL)")?;
        let rows = st.query_map([list_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
        let mut out = Vec::new();
        for r in rows {
            let (pid, q, p) = r?;
            out.push(PriceListItem {
                list_id,
                product_id: pid,
                min_qty: Decimal::from_str(&q).unwrap_or_default(),
                price: Decimal::from_str(&p).unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// Imposta (o rimuove con `None`) il prezzo specifico di un articolo in un listino.
    pub fn set_price_list_item(&self, list_id: i64, product_id: i64, min_qty: Decimal, price: Option<Decimal>) -> Result<()> {
        let q = dec_to_sql(min_qty.max(Decimal::ZERO));
        match price {
            Some(p) => {
                if p.is_sign_negative() {
                    return Err(StoreError::Invalid("Prezzo negativo".into()));
                }
                self.conn().execute(
                    "INSERT INTO price_list_items(list_id, product_id, min_qty, price) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(list_id, product_id, min_qty) DO UPDATE SET price = excluded.price",
                    params![list_id, product_id, q, dec_to_sql(p)],
                )?;
            }
            None => {
                self.conn().execute("DELETE FROM price_list_items WHERE list_id = ?1 AND product_id = ?2 AND min_qty = ?3", params![list_id, product_id, q])?;
            }
        }
        Ok(())
    }

    pub fn party_prices(&self, party_id: i64) -> Result<Vec<PartyPrice>> {
        let mut st = self.conn().prepare("SELECT id, data FROM party_prices WHERE party_id = ?1 ORDER BY id")?;
        let rows = st.query_map([party_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            let (id, data) = r?;
            let mut x: PartyPrice = serde_json::from_str(&data)?;
            x.id = id;
            x.party_id = party_id;
            out.push(x);
        }
        Ok(out)
    }

    pub fn save_party_price(&self, x: &PartyPrice) -> Result<PartyPrice> {
        let mut x = x.clone();
        if x.party_id == 0 {
            return Err(StoreError::Invalid("Salvare prima il soggetto".into()));
        }
        if x.product_id.is_none() && x.category.trim().is_empty() {
            return Err(StoreError::Invalid("Indicare l'articolo o la categoria".into()));
        }
        if x.price.is_none() && x.discount.trim().is_empty() {
            return Err(StoreError::Invalid("Indicare un prezzo o uno sconto".into()));
        }
        openfatt_core::calc::parse_discounts(&x.discount).map_err(|e| StoreError::Invalid(e.0))?;
        let data = serde_json::to_string(&x)?;
        if x.id == 0 {
            self.conn().execute("INSERT INTO party_prices(party_id, product_id, category, data) VALUES (?1, ?2, ?3, ?4)", params![x.party_id, x.product_id, x.category, data])?;
            x.id = self.conn().last_insert_rowid();
        } else {
            self.conn().execute("UPDATE party_prices SET product_id = ?1, category = ?2, data = ?3 WHERE id = ?4", params![x.product_id, x.category, data, x.id])?;
        }
        self.audit("party_price", x.id, "save", &format!("soggetto {}", x.party_id))?;
        Ok(x)
    }

    pub fn delete_party_price(&self, id: i64) -> Result<()> {
        self.conn().execute("DELETE FROM party_prices WHERE id = ?1", [id])?;
        Ok(())
    }

    fn list_with_items(&self, id: i64) -> Option<(PriceList, Vec<PriceListItem>)> {
        let l = self.price_list(id).ok().flatten()?;
        let items = self.price_list_items(id).unwrap_or_default();
        Some((l, items))
    }

    fn vat_rate_of(&self, product: &Product) -> Decimal {
        product.vat_id.and_then(|id| self.vat_code(id).ok().flatten()).map(|v| v.rate).unwrap_or(Decimal::from(22))
    }

    /// Prezzo di un articolo per un soggetto e una quantità, alla data indicata.
    pub fn quote(&self, party: Option<&Party>, product: &Product, qty: Decimal, date: NaiveDate, purchase: bool) -> Result<PriceQuote> {
        let rules = match party {
            Some(p) if p.id != 0 => self.party_prices(p.id)?,
            _ => vec![],
        };
        let list = party.and_then(|p| p.price_list_id).and_then(|id| self.list_with_items(id));
        let lookup = |id: i64| self.list_with_items(id);
        let rate = self.vat_rate_of(product);
        Ok(pricing::quote(product, qty, date, rate, purchase, party, list.as_ref().map(|(l, i)| ListData { list: l, items: i }), &rules, &lookup))
    }

    /// Prezzo netto di un articolo in un listino (senza condizioni del soggetto).
    pub fn list_price_of(&self, list_id: i64, product: &Product, qty: Decimal) -> Result<Option<(Decimal, String)>> {
        let Some((l, items)) = self.list_with_items(list_id) else { return Ok(None) };
        let lookup = |id: i64| self.list_with_items(id);
        Ok(pricing::list_price(&ListData { list: &l, items: &items }, product, qty, self.vat_rate_of(product), &lookup, 0))
    }

    /// Categorie di articoli in uso (per le condizioni per categoria).
    pub fn product_categories(&self) -> Result<Vec<String>> {
        let mut st = self.conn().prepare("SELECT DISTINCT category FROM products WHERE category IS NOT NULL AND category <> '' ORDER BY category COLLATE NOCASE")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Quanti soggetti usano un listino.
    pub fn price_list_usage(&self, list_id: i64) -> Result<usize> {
        let n: Option<i64> = self
            .conn()
            .query_row("SELECT COUNT(*) FROM parties WHERE json_extract(data, '$.price_list_id') = ?1", [list_id], |r| r.get(0))
            .optional()?;
        Ok(n.unwrap_or(0) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;
    use openfatt_core::pricing::PriceListKind;

    #[test]
    fn store_quote_end_to_end() {
        let s = Store::open_in_memory().unwrap();
        let v = s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap();
        let p = s.save_product(&Product { code: "A".into(), description: "Vernice".into(), price: dec!(100), cost: dec!(55), category: "Vernici".into(), vat_id: Some(v.id), ..Default::default() }).unwrap();
        let riv = s.save_price_list(&PriceList { name: "Rivenditori".into(), percent: dec!(-15), ..Default::default() }).unwrap();
        let ingr = s.save_price_list(&PriceList { name: "Grandi clienti".into(), base: PriceBase::List(riv.id), percent: dec!(-5), round_to: dec!(0.5), ..Default::default() }).unwrap();
        // catena circolare rifiutata
        let mut bad = riv.clone();
        bad.base = PriceBase::List(ingr.id);
        assert!(s.save_price_list(&bad).is_err());
        s.set_price_list_item(riv.id, p.id, dec!(10), Some(dec!(80))).unwrap();
        let mut c = s.save_party(&Party { name: "Colorificio".into(), price_list_id: Some(ingr.id), ..Default::default() }).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
        // 100 -15% = 85 -5% = 80,75 -> arrotondato 81,00
        let q = s.quote(Some(&c), &p, dec!(1), today, false).unwrap();
        assert_eq!(q.price, dec!(81.0));
        // da 10 pezzi il listino base ha prezzo 80 -> 76 -> 76,00
        let q = s.quote(Some(&c), &p, dec!(10), today, false).unwrap();
        assert_eq!(q.price, dec!(76.0));
        // condizione particolare per categoria
        s.save_party_price(&PartyPrice { party_id: c.id, category: "Vernici".into(), discount: "3".into(), ..Default::default() }).unwrap();
        let q = s.quote(Some(&c), &p, dec!(1), today, false).unwrap();
        assert_eq!(q.discounts, "3");
        assert!(q.source.contains("categoria"));
        // listino eliminabile solo se non usato
        assert!(s.delete_price_list(ingr.id).is_err());
        assert_eq!(s.price_list_usage(ingr.id).unwrap(), 1);
        c.price_list_id = None;
        s.save_party(&c).unwrap();
        s.delete_price_list(ingr.id).unwrap();
        // acquisti: costo
        let q = s.quote(None, &p, dec!(1), today, true).unwrap();
        assert_eq!(q.price, dec!(55));
        // listino manuale senza prezzo per l'articolo: prezzo base
        let man = s.save_price_list(&PriceList { name: "Manuale".into(), kind: PriceListKind::Manual, ..Default::default() }).unwrap();
        assert!(s.list_price_of(man.id, &p, dec!(1)).unwrap().is_none());
    }
}
