//! Importazioni da formati esterni nell'archivio: ordini NSO della Pubblica Amministrazione,
//! listini Metel dei fornitori, ordini dei negozi online.

use rust_decimal::Decimal;

use openfatt_core::model::{DocKind, DocLine, DocStatus, Document, Party, PartyRole, Product, RelatedDoc, VatRef};
use openfatt_interop::metel::MetelList;
use openfatt_interop::nso::NsoOrder;
use openfatt_interop::shop::{ShopKind, ShopOrder};

use crate::{PartyFilter, Result, Store, StoreError};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetelImport {
    pub created: usize,
    pub updated: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShopImport {
    pub created: Vec<i64>,
    pub already: usize,
    pub new_customers: usize,
    pub unknown_skus: Vec<String>,
}

impl Store {
    fn vat_for_rate(&self, rate: Option<Decimal>) -> Result<VatRef> {
        let vats = self.vat_codes()?;
        let r = rate.unwrap_or(Decimal::from(22));
        Ok(vats.iter().find(|v| v.rate == r && v.natura.is_none()).or_else(|| vats.iter().find(|v| v.code == "22")).map(VatRef::from_code).unwrap_or(VatRef { vat_id: None, rate: r, natura: None, legal_ref: String::new(), split: false }))
    }

    /// Crea un ordine cliente in bozza da un ordine NSO, con la tripletta dell'ordine nei
    /// riferimenti (passa in fattura come DatiOrdineAcquisto). L'ente si cerca per partita
    /// IVA o codice fiscale; se manca si crea come Pubblica Amministrazione.
    pub fn import_nso_order(&self, o: &NsoOrder) -> Result<Document> {
        let (id, date, commessa) = o.triplet();
        let dup: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM documents WHERE kind = 'ORC' AND json_extract(data, '$.related') LIKE ?1",
            [format!("%\"number\":\"{}\"%{}%", id.replace('"', ""), commessa.replace('"', ""))],
            |r| r.get(0),
        )?;
        if dup > 0 {
            return Err(StoreError::Invalid(format!("L'ordine NSO {} è già stato importato", id)));
        }
        let tx = self.begin()?;
        let party = match self.find_party_by_tax_id(&o.buyer.vat_number, &o.buyer.tax_code)? {
            Some(p) => p,
            None => self.save_party(&Party {
                role: PartyRole::Customer,
                name: o.buyer.name.clone(),
                vat_number: o.buyer.vat_number.clone(),
                tax_code: if o.buyer.tax_code.is_empty() { o.buyer.vat_number.clone() } else { o.buyer.tax_code.clone() },
                address: o.buyer.address.clone(),
                city: o.buyer.city.clone(),
                zip: o.buyer.zip.clone(),
                province: o.buyer.province.clone(),
                country: if o.buyer.country.is_empty() { "IT".into() } else { o.buyer.country.clone() },
                is_pa: true,
                // codice univoco dell'ufficio di fatturazione, se presente nell'ordine
                sdi_code: if o.accounting.endpoint.len() == 6 { o.accounting.endpoint.to_uppercase() } else { String::new() },
                ..Default::default()
            })?,
        };
        let mut d = Document::new(DocKind::CustomerOrder);
        d.date = date.unwrap_or_else(|| chrono::Local::now().date_naive());
        d.party_id = Some(party.id);
        d.party = party.clone();
        d.vat_liability = "S".into();
        d.notes = o.note.clone();
        if !o.delivery_address.is_empty() {
            d.destination = o.delivery_address.clone();
        }
        d.related.push(RelatedDoc { kind: "ORDINE".into(), number: id.clone(), date, cig: o.cig.clone(), commessa, ..Default::default() });
        for l in &o.lines {
            let product = if l.code.is_empty() { None } else { self.product_by_code(&l.code)? };
            d.lines.push(DocLine {
                product_id: product.as_ref().map(|p| p.id),
                code: l.code.clone(),
                description: if l.description.is_empty() { product.as_ref().map(|p| p.description.clone()).unwrap_or_default() } else { l.description.clone() },
                qty: l.qty,
                unit: product.as_ref().map(|p| p.unit.clone()).unwrap_or_else(|| if l.unit == "C62" || l.unit == "H87" { "pz".into() } else { l.unit.clone() }),
                unit_price: openfatt_core::money::round_dp(l.price, 6),
                vat: self.vat_for_rate(l.vat_rate)?,
                unit_cost: product.as_ref().map(|p| p.cost).unwrap_or_default(),
                ..Default::default()
            });
        }
        let saved = self.save_document(&d)?;
        self.audit("document", saved.id, "import_nso", &id)?;
        tx.commit()?;
        Ok(saved)
    }

    /// Carica o aggiorna gli articoli da un listino Metel. Codice = sigla marchio + codice
    /// prodotto; prezzo di vendita = prezzo al pubblico, costo = prezzo al rivenditore
    /// meno lo sconto indicato. Gli articoli annullati nel listino vengono disattivati.
    pub fn import_metel(&self, list: &MetelList, supplier_id: Option<i64>, dealer_discount: Decimal, update_prices: bool) -> Result<MetelImport> {
        let vat = self.vat_for_rate(None)?;
        let mut r = MetelImport::default();
        let tx = self.begin()?;
        for it in &list.items {
            let code: String = it.full_code().chars().take(30).collect();
            let cost = openfatt_core::money::round_dp(it.dealer_price * (Decimal::ONE_HUNDRED - dealer_discount) / Decimal::ONE_HUNDRED, 4);
            let unit = match it.unit.as_str() {
                "PCE" | "NR" | "" => "pz".to_string(),
                "MTR" => "m".to_string(),
                "KGM" => "kg".to_string(),
                other => other.to_lowercase(),
            };
            match self.product_by_code(&code)? {
                Some(mut p) => {
                    if !update_prices && !it.is_cancelled() {
                        r.skipped += 1;
                        continue;
                    }
                    if update_prices {
                        p.price = it.public_price;
                        p.cost = cost;
                    }
                    if it.is_cancelled() {
                        p.active = false;
                    }
                    if p.barcode.is_empty() && !it.ean.is_empty() {
                        p.barcode = it.ean.clone();
                    }
                    p.supplier_id = p.supplier_id.or(supplier_id);
                    self.save_product(&p)?;
                    r.updated += 1;
                }
                None if it.is_cancelled() => r.skipped += 1,
                None => {
                    self.save_product(&Product {
                        code,
                        description: it.description.clone(),
                        unit,
                        category: if it.stat_family.is_empty() { it.brand.clone() } else { format!("{} > {}", it.brand, it.stat_family) },
                        price: it.public_price,
                        cost,
                        vat_id: vat.vat_id,
                        barcode: it.ean.clone(),
                        supplier_id,
                        supplier_code: it.code.clone(),
                        pack_size: if it.box_qty > Decimal::ONE { it.box_qty } else { Decimal::ZERO },
                        pack_unit: if it.box_qty > Decimal::ONE { "CT".into() } else { String::new() },
                        ..Default::default()
                    })?;
                    r.created += 1;
                }
            }
        }
        self.audit("products", 0, "import_metel", &format!("{} {}: {} nuovi, {} aggiornati", list.header.brand, list.header.list_number, r.created, r.updated))?;
        tx.commit()?;
        Ok(r)
    }

    /// Importa gli ordini del negozio online come ordini cliente in bozza (prezzi IVA
    /// inclusa). I clienti si riconoscono dall'email; gli ordini già importati si saltano.
    pub fn import_shop_orders(&self, kind: ShopKind, orders: &[ShopOrder]) -> Result<ShopImport> {
        let shop = kind.label();
        let mut res = ShopImport::default();
        for o in orders {
            let known: i64 = self.conn().query_row("SELECT COUNT(*) FROM shop_links WHERE shop = ?1 AND entity = 'order' AND remote_id = ?2", rusqlite::params![shop, o.id], |r| r.get(0))?;
            if known > 0 {
                res.already += 1;
                continue;
            }
            let tx = self.begin()?;
            let email = o.customer.email.trim().to_lowercase();
            let existing = if email.is_empty() { None } else { self.parties(&PartyFilter { role: Some(PartyRole::Customer), search: email.clone(), include_inactive: true })?.into_iter().find(|p| p.email.trim().to_lowercase() == email) };
            let party = match existing {
                Some(p) => p,
                None => {
                    res.new_customers += 1;
                    let company = !o.customer.company.trim().is_empty();
                    self.save_party(&Party {
                        role: PartyRole::Customer,
                        name: if company { o.customer.company.clone() } else { String::new() },
                        first_name: if company { String::new() } else { o.customer.first_name.clone() },
                        last_name: if company { String::new() } else { o.customer.last_name.clone() },
                        vat_number: o.customer.vat_number.clone(),
                        tax_code: o.customer.tax_code.to_uppercase(),
                        address: o.customer.address.clone(),
                        city: o.customer.city.clone(),
                        zip: o.customer.zip.clone(),
                        province: o.customer.province.clone(),
                        country: if o.customer.country.is_empty() { "IT".into() } else { o.customer.country.clone() },
                        email: o.customer.email.clone(),
                        phone: o.customer.phone.clone(),
                        notes: format!("Cliente del negozio online ({})", shop),
                        ..Default::default()
                    })?
                }
            };
            let mut d = Document::new(DocKind::CustomerOrder);
            d.date = o.date.unwrap_or_else(|| chrono::Local::now().date_naive());
            d.party_id = Some(party.id);
            d.party = party;
            d.prices_include_vat = true;
            d.shop_order = format!("{} {}", shop, o.number);
            d.destination = o.shipping_address.clone();
            d.notes = [o.note.clone(), if o.payment_method.is_empty() { String::new() } else { format!("Pagamento: {}{}", o.payment_method, if o.paid { " (pagato)" } else { "" }) }].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join("\n");
            if !o.currency.is_empty() && o.currency != "EUR" {
                d.currency = o.currency.clone();
            }
            d.related.push(RelatedDoc { kind: "ORDINE".into(), number: o.number.clone(), date: o.date, ..Default::default() });
            for l in &o.lines {
                let product = if l.sku.is_empty() { None } else { self.product_by_code(&l.sku)? };
                if product.is_none() && !l.sku.is_empty() && !res.unknown_skus.contains(&l.sku) {
                    res.unknown_skus.push(l.sku.clone());
                }
                d.lines.push(DocLine {
                    product_id: product.as_ref().map(|p| p.id),
                    code: l.sku.clone(),
                    description: l.name.clone(),
                    qty: l.qty,
                    unit: product.as_ref().map(|p| p.unit.clone()).unwrap_or_else(|| "pz".into()),
                    unit_price: l.unit_gross,
                    vat: self.vat_for_rate(l.vat_rate)?,
                    unit_cost: product.as_ref().map(|p| p.cost).unwrap_or_default(),
                    ..Default::default()
                });
            }
            if !o.shipping_gross.is_zero() {
                d.lines.push(DocLine { description: "Spese di spedizione".into(), qty: Decimal::ONE, unit_price: o.shipping_gross, vat: self.vat_for_rate(None)?, ..Default::default() });
            }
            d.status = DocStatus::Draft;
            let saved = self.save_document(&d)?;
            self.conn().execute("INSERT INTO shop_links(shop, entity, remote_id, local_id) VALUES (?1, 'order', ?2, ?3)", rusqlite::params![shop, o.id, saved.id])?;
            tx.commit()?;
            res.created.push(saved.id);
        }
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;

    #[test]
    fn nso_order_becomes_customer_order_with_triplet() {
        let s = Store::open_in_memory().unwrap();
        let o = openfatt_interop::nso::parse_order(openfatt_interop::nso::SAMPLE_ORDER).unwrap();
        let d = s.import_nso_order(&o).unwrap();
        assert_eq!(d.kind, DocKind::CustomerOrder);
        assert!(d.party.is_pa);
        assert_eq!(d.party.sdi_code, "UFE1A2");
        assert_eq!(d.related[0].commessa, "#B9BGAQ#");
        assert_eq!(d.lines.len(), 2);
        assert!(s.import_nso_order(&o).is_err(), "doppione");
        // l'ordine evaso in fattura porta la tripletta
        let d = s.issue_document(d.id).unwrap();
        let inv = s.transform(&[d.id], DocKind::Invoice).unwrap();
        assert!(inv.related.iter().any(|r| r.number == "110" && r.commessa == "#B9BGAQ#"), "{:?}", inv.related);
    }

    #[test]
    fn metel_import_creates_and_updates() {
        let s = Store::open_in_memory().unwrap();
        let header = format!("{:<20}{:<3}{:<11}{:<6}{}{}{:<30}{:<39}{}", "LISTINO METEL", "ABC", "01234567890", "000001", "20260101", "20251215", "Listino", "", "020");
        let it = openfatt_interop::metel::MetelItem { brand: "ABC".into(), code: "INT10A".into(), ean: "8012345678901".into(), description: "Interruttore".into(), box_qty: dec!(10), min_qty: dec!(1), dealer_price: dec!(2.40), public_price: dec!(4.10), currency: "EUR".into(), unit: "PCE".into(), status: "1".into(), ..Default::default() };
        let text = format!("{}\n{}\n", header, openfatt_interop::metel::format_item(&it, 1));
        let list = openfatt_interop::metel::parse(&text).unwrap();
        let r = s.import_metel(&list, None, dec!(40), true).unwrap();
        assert_eq!(r.created, 1);
        let p = s.product_by_code("ABCINT10A").unwrap().unwrap();
        assert_eq!((p.price, p.cost, p.pack_size), (dec!(4.10), dec!(1.44), dec!(10)));
        let it2 = openfatt_interop::metel::MetelItem { status: "9".into(), ..it };
        let list = openfatt_interop::metel::parse(&format!("{}\n{}\n", header, openfatt_interop::metel::format_item(&it2, 1))).unwrap();
        assert_eq!(s.import_metel(&list, None, dec!(40), true).unwrap().updated, 1);
        assert!(!s.product_by_code("ABCINT10A").unwrap().unwrap().active, "articolo annullato nel listino");
    }

    #[test]
    fn shop_orders_import_once() {
        let s = Store::open_in_memory().unwrap();
        s.save_product(&Product { code: "LAM01".into(), description: "Lampada".into(), price: dec!(39), ..Default::default() }).unwrap();
        let json = r#"[{"id":727,"number":"727","currency":"EUR","date_created":"2026-10-30T14:25:05","total":"57.50","shipping_total":"8.20","shipping_tax":"1.80",
          "billing":{"first_name":"Luca","last_name":"Verdi","address_1":"Via Po 4","city":"Torino","postcode":"10121","country":"IT","email":"Luca@Example.com"},
          "line_items":[{"name":"Lampada da tavolo","sku":"LAM01","quantity":1,"total":"39.00","total_tax":"8.58"},{"name":"Altro","sku":"ZZZ","quantity":2,"total":"10.00","total_tax":"2.20"}]}]"#;
        let orders = openfatt_interop::shop::parse_woocommerce(json).unwrap();
        let r = s.import_shop_orders(ShopKind::WooCommerce, &orders).unwrap();
        assert_eq!((r.created.len(), r.new_customers, r.unknown_skus.clone()), (1, 1, vec!["ZZZ".to_string()]));
        let d = s.document(r.created[0]).unwrap();
        assert!(d.prices_include_vat);
        assert_eq!(d.lines.len(), 3);
        let t = openfatt_core::calc::compute(&d).unwrap();
        assert_eq!(t.total, dec!(69.78));
        let again = s.import_shop_orders(ShopKind::WooCommerce, &orders).unwrap();
        assert_eq!((again.created.len(), again.already), (0, 1));
    }
}
