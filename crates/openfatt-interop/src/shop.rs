//! Collegamento con i negozi online tramite le API pubbliche documentate:
//! - WooCommerce REST API v3 (`/wp-json/wc/v3/orders`, `/products`);
//! - Shopify Admin REST API (`/admin/api/<versione>/orders.json`, `/products.json`).
//!
//! Qui ci sono solo la lettura degli ordini e la preparazione dei dati degli articoli:
//! le chiamate HTTP sono fatte dall'applicazione.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShopKind {
    #[default]
    WooCommerce,
    Shopify,
}

impl ShopKind {
    pub fn label(self) -> &'static str {
        match self {
            ShopKind::WooCommerce => "WooCommerce",
            ShopKind::Shopify => "Shopify",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShopCustomer {
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub company: String,
    pub address: String,
    pub city: String,
    pub zip: String,
    pub province: String,
    pub country: String,
    pub phone: String,
    pub vat_number: String,
    pub tax_code: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShopLine {
    pub sku: String,
    pub name: String,
    pub qty: Decimal,
    /// Prezzo unitario IVA inclusa, dopo gli sconti
    pub unit_gross: Decimal,
    pub vat_rate: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShopOrder {
    pub id: String,
    pub number: String,
    pub date: Option<chrono::NaiveDate>,
    pub status: String,
    pub currency: String,
    pub customer: ShopCustomer,
    pub shipping_address: String,
    pub lines: Vec<ShopLine>,
    pub shipping_gross: Decimal,
    pub total: Decimal,
    pub paid: bool,
    pub payment_method: String,
    pub note: String,
}

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(x)) => x.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn d(v: &Value, k: &str) -> Decimal {
    match v.get(k) {
        Some(Value::String(x)) => Decimal::from_str(x.trim()).unwrap_or_default(),
        Some(Value::Number(n)) => Decimal::from_str(&n.to_string()).unwrap_or_default(),
        _ => Decimal::ZERO,
    }
}

fn date_of(text: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(text.get(..10)?, "%Y-%m-%d").ok()
}

fn meta(order: &Value, keys: &[&str]) -> String {
    let Some(Value::Array(m)) = order.get("meta_data") else { return String::new() };
    for item in m {
        let k = s(item, "key").to_lowercase();
        if keys.iter().any(|x| k.contains(x)) {
            return s(item, "value");
        }
    }
    String::new()
}

/// Ordini WooCommerce (risposta di GET /wp-json/wc/v3/orders).
pub fn parse_woocommerce(json: &str) -> Result<Vec<ShopOrder>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("Risposta non valida: {}", e))?;
    let arr = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(_) => vec![v.clone()],
        _ => return Err("Risposta inattesa".into()),
    };
    let mut out = Vec::new();
    for o in arr {
        let b = o.get("billing").cloned().unwrap_or(Value::Null);
        let sh = o.get("shipping").cloned().unwrap_or(Value::Null);
        let mut lines = Vec::new();
        if let Some(Value::Array(items)) = o.get("line_items") {
            for it in items {
                let qty = d(it, "quantity");
                let net = d(it, "total");
                let tax = d(it, "total_tax");
                let unit = if qty.is_zero() { Decimal::ZERO } else { openfatt_core::money::round_dp((net + tax) / qty, 4) };
                let rate = if net.is_zero() { None } else { Some(openfatt_core::money::round_dp(tax / net * Decimal::ONE_HUNDRED, 0)) };
                lines.push(ShopLine { sku: s(it, "sku"), name: s(it, "name"), qty, unit_gross: unit, vat_rate: rate });
            }
        }
        let ship_addr: Vec<String> = ["address_1", "postcode", "city", "state"].iter().map(|k| s(&sh, k)).filter(|x| !x.is_empty()).collect();
        out.push(ShopOrder {
            id: s(&o, "id"),
            number: { let n = s(&o, "number"); if n.is_empty() { s(&o, "id") } else { n } },
            date: date_of(&s(&o, "date_created")),
            status: s(&o, "status"),
            currency: s(&o, "currency"),
            customer: ShopCustomer {
                email: s(&b, "email"),
                first_name: s(&b, "first_name"),
                last_name: s(&b, "last_name"),
                company: s(&b, "company"),
                address: [s(&b, "address_1"), s(&b, "address_2")].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" "),
                city: s(&b, "city"),
                zip: s(&b, "postcode"),
                province: s(&b, "state"),
                country: s(&b, "country"),
                phone: s(&b, "phone"),
                vat_number: meta(&o, &["vat", "partita_iva", "piva"]),
                tax_code: meta(&o, &["codice_fiscale", "fiscal", "cf"]),
            },
            shipping_address: ship_addr.join(" "),
            lines,
            shipping_gross: d(&o, "shipping_total") + d(&o, "shipping_tax"),
            total: d(&o, "total"),
            paid: !s(&o, "date_paid").is_empty() || o.get("set_paid").and_then(|x| x.as_bool()).unwrap_or(false),
            payment_method: s(&o, "payment_method_title"),
            note: s(&o, "customer_note"),
        });
    }
    Ok(out)
}

/// Ordini Shopify (risposta di GET /admin/api/<v>/orders.json).
pub fn parse_shopify(json: &str) -> Result<Vec<ShopOrder>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("Risposta non valida: {}", e))?;
    let arr = match v.get("orders") {
        Some(Value::Array(a)) => a.clone(),
        _ => match v.get("order") {
            Some(o) => vec![o.clone()],
            None => return Err("Risposta inattesa: manca \"orders\"".into()),
        },
    };
    let mut out = Vec::new();
    for o in arr {
        let b = o.get("billing_address").cloned().unwrap_or(Value::Null);
        let sh = o.get("shipping_address").cloned().unwrap_or(Value::Null);
        let mut lines = Vec::new();
        if let Some(Value::Array(items)) = o.get("line_items") {
            for it in items {
                let qty = d(it, "quantity");
                let price = d(it, "price");
                let disc = d(it, "total_discount");
                let rate = it.get("tax_lines").and_then(|t| t.as_array()).and_then(|t| t.first()).map(|t| openfatt_core::money::round_dp(d(t, "rate") * Decimal::ONE_HUNDRED, 2));
                // con taxes_included i prezzi sono IVA inclusa
                let included = o.get("taxes_included").and_then(|x| x.as_bool()).unwrap_or(true);
                let mut unit = if qty.is_zero() { Decimal::ZERO } else { price - disc / qty };
                if !included {
                    if let Some(r) = rate {
                        unit = unit * (Decimal::ONE + r / Decimal::ONE_HUNDRED);
                    }
                }
                lines.push(ShopLine { sku: s(it, "sku"), name: s(it, "name"), qty, unit_gross: openfatt_core::money::round_dp(unit, 4), vat_rate: rate });
            }
        }
        let shipping: Decimal = o.get("shipping_lines").and_then(|x| x.as_array()).map(|a| a.iter().map(|l| d(l, "price")).sum()).unwrap_or_default();
        let ship_addr: Vec<String> = ["address1", "zip", "city", "province_code"].iter().map(|k| s(&sh, k)).filter(|x| !x.is_empty()).collect();
        let cust = o.get("customer").cloned().unwrap_or(Value::Null);
        out.push(ShopOrder {
            id: s(&o, "id"),
            number: { let n = s(&o, "name"); if n.is_empty() { s(&o, "order_number") } else { n.trim_start_matches('#').to_string() } },
            date: date_of(&s(&o, "created_at")),
            status: s(&o, "fulfillment_status"),
            currency: s(&o, "currency"),
            customer: ShopCustomer {
                email: { let e = s(&o, "email"); if e.is_empty() { s(&cust, "email") } else { e } },
                first_name: s(&b, "first_name"),
                last_name: s(&b, "last_name"),
                company: s(&b, "company"),
                address: [s(&b, "address1"), s(&b, "address2")].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" "),
                city: s(&b, "city"),
                zip: s(&b, "zip"),
                province: s(&b, "province_code"),
                country: s(&b, "country_code"),
                phone: s(&b, "phone"),
                vat_number: String::new(),
                tax_code: String::new(),
            },
            shipping_address: ship_addr.join(" "),
            lines,
            shipping_gross: shipping,
            total: d(&o, "total_price"),
            paid: s(&o, "financial_status") == "paid",
            payment_method: o.get("payment_gateway_names").and_then(|x| x.as_array()).and_then(|a| a.first()).and_then(|x| x.as_str()).unwrap_or("").to_string(),
            note: s(&o, "note"),
        });
    }
    Ok(out)
}

/// Dati di un articolo da pubblicare o aggiornare sul negozio.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShopProduct {
    pub sku: String,
    pub name: String,
    pub gross_price: Decimal,
    pub stock: Option<Decimal>,
    pub description: String,
}

/// Corpo JSON per WooCommerce (POST /products o PUT /products/<id>).
pub fn woocommerce_product_json(p: &ShopProduct) -> String {
    let mut v = serde_json::json!({
        "sku": p.sku,
        "name": p.name,
        "regular_price": openfatt_core::money::fmt_xml(p.gross_price, 2),
        "description": p.description,
    });
    if let Some(q) = p.stock {
        v["manage_stock"] = Value::Bool(true);
        v["stock_quantity"] = Value::from(q.trunc().to_string().parse::<i64>().unwrap_or(0));
    }
    v.to_string()
}

/// Corpo JSON per Shopify (POST /products.json): prezzo e SKU sulla variante.
pub fn shopify_product_json(p: &ShopProduct) -> String {
    serde_json::json!({
        "product": {
            "title": p.name,
            "body_html": p.description,
            "variants": [{ "sku": p.sku, "price": openfatt_core::money::fmt_xml(p.gross_price, 2), "inventory_management": if p.stock.is_some() { "shopify" } else { "" } }]
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WOO: &str = r#"[{"id":727,"number":"727","status":"processing","currency":"EUR","date_created":"2026-10-30T14:25:05","date_paid":"2026-10-30T14:26:00",
      "total":"57.50","shipping_total":"8.20","shipping_tax":"1.80","payment_method_title":"Carta di credito","customer_note":"Citofonare Rossi",
      "billing":{"first_name":"Luca","last_name":"Verdi","company":"","address_1":"Via Po 4","address_2":"","city":"Torino","state":"TO","postcode":"10121","country":"IT","email":"luca@example.com","phone":"333"},
      "shipping":{"address_1":"Via Po 4","city":"Torino","state":"TO","postcode":"10121"},
      "meta_data":[{"id":1,"key":"_billing_codice_fiscale","value":"VRDLCU80A01L219X"}],
      "line_items":[{"id":1,"name":"Lampada da tavolo","sku":"LAM01","quantity":1,"total":"39.00","total_tax":"8.58"}]}]"#;

    const SHOPIFY: &str = r##"{"orders":[{"id":450789469,"name":"#1001","created_at":"2026-11-02T10:00:00-05:00","currency":"EUR","email":"anna@example.com","financial_status":"paid","taxes_included":true,
      "total_price":"159.00","fulfillment_status":null,"payment_gateway_names":["shopify_payments"],
      "billing_address":{"first_name":"Anna","last_name":"Neri","address1":"Corso Italia 9","city":"Milano","zip":"20122","province_code":"MI","country_code":"IT"},
      "shipping_address":{"address1":"Corso Italia 9","city":"Milano","zip":"20122","province_code":"MI"},
      "shipping_lines":[{"price":"10.00"}],
      "line_items":[{"sku":"SED01","name":"Sedia ergonomica","quantity":1,"price":"149.00","total_discount":"0.00","tax_lines":[{"rate":0.22,"price":"26.87"}]}]}]}"##;

    #[test]
    fn woocommerce_orders() {
        let o = parse_woocommerce(WOO).unwrap();
        assert_eq!(o.len(), 1);
        let o = &o[0];
        assert_eq!(o.number, "727");
        assert!(o.paid);
        assert_eq!(o.customer.tax_code, "VRDLCU80A01L219X");
        assert_eq!(o.lines[0].unit_gross, Decimal::from_str("47.58").unwrap());
        assert_eq!(o.lines[0].vat_rate, Some(Decimal::from(22)));
        assert_eq!(o.shipping_gross, Decimal::from(10));
    }

    #[test]
    fn shopify_orders_and_product_payloads() {
        let o = parse_shopify(SHOPIFY).unwrap();
        assert_eq!(o[0].number, "1001");
        assert_eq!(o[0].lines[0].unit_gross, Decimal::from(149));
        assert_eq!(o[0].lines[0].vat_rate, Some(Decimal::from(22)));
        assert_eq!(o[0].customer.city, "Milano");
        let p = ShopProduct { sku: "SED01".into(), name: "Sedia".into(), gross_price: Decimal::from_str("149.9").unwrap(), stock: Some(Decimal::from(7)), description: String::new() };
        assert!(woocommerce_product_json(&p).contains("\"regular_price\":\"149.90\""));
        assert!(woocommerce_product_json(&p).contains("\"stock_quantity\":7"));
        assert!(shopify_product_json(&p).contains("\"sku\":\"SED01\""));
    }
}
