//! Ordini elettronici della Pubblica Amministrazione ricevuti tramite il Nodo di Smistamento
//! degli Ordini (NSO). Il formato è UBL 2.1 Order secondo le regole PEPPOL BIS Order /
//! NSO (specifiche pubbliche del MEF). In fattura la "tripletta" dell'ordine va in
//! DatiOrdineAcquisto: IdDocumento = ID dell'ordine, Data = IssueDate,
//! CodiceCommessaConvenzione = "#" + EndpointID del punto ordinante + "#".

use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NsoParty {
    pub endpoint: String,
    pub endpoint_scheme: String,
    pub name: String,
    pub vat_number: String,
    pub tax_code: String,
    pub address: String,
    pub city: String,
    pub zip: String,
    pub province: String,
    pub country: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NsoLine {
    pub id: String,
    pub code: String,
    pub description: String,
    pub qty: Decimal,
    pub unit: String,
    pub price: Decimal,
    pub vat_rate: Option<Decimal>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NsoOrder {
    pub id: String,
    pub issue_date: Option<NaiveDate>,
    /// Tipo: 220 ordine, 227 ordine di convalida ("pre-concordato")
    pub type_code: String,
    pub currency: String,
    pub note: String,
    pub buyer: NsoParty,
    /// Ufficio di fatturazione (codice univoco per la fattura elettronica)
    pub accounting: NsoParty,
    pub seller_endpoint: String,
    pub delivery_date: Option<NaiveDate>,
    pub delivery_address: String,
    pub lines: Vec<NsoLine>,
    pub cig: String,
}

impl NsoOrder {
    /// Riferimento da riportare in fattura: (IdDocumento, Data, CodiceCommessaConvenzione).
    pub fn triplet(&self) -> (String, Option<NaiveDate>, String) {
        (self.id.clone(), self.issue_date, format!("#{}#", self.buyer.endpoint.trim()))
    }
}

fn child<'a>(n: roxmltree::Node<'a, 'a>, name: &str) -> Option<roxmltree::Node<'a, 'a>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn path<'a>(n: roxmltree::Node<'a, 'a>, names: &[&str]) -> Option<roxmltree::Node<'a, 'a>> {
    let mut cur = n;
    for p in names {
        cur = child(cur, p)?;
    }
    Some(cur)
}

fn text(n: Option<roxmltree::Node>) -> String {
    n.and_then(|x| x.text()).map(|t| t.trim().to_string()).unwrap_or_default()
}

fn date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s.trim()).unwrap_or_default()
}

fn party(n: Option<roxmltree::Node>) -> NsoParty {
    let Some(n) = n else { return NsoParty::default() };
    let p = child(n, "Party").unwrap_or(n);
    let ep = child(p, "EndpointID");
    let addr = child(p, "PostalAddress");
    let tax = child(p, "PartyTaxScheme");
    let legal = child(p, "PartyLegalEntity");
    let company_id = text(tax.and_then(|t| child(t, "CompanyID")));
    let (vat, cf) = if company_id.to_uppercase().starts_with("IT") { (company_id[2..].to_string(), String::new()) } else { (String::new(), company_id) };
    let legal_id = text(legal.and_then(|l| child(l, "CompanyID")));
    NsoParty {
        endpoint: text(ep),
        endpoint_scheme: ep.and_then(|e| e.attribute("schemeID")).unwrap_or("").to_string(),
        name: {
            let a = text(path(p, &["PartyName", "Name"]));
            if a.is_empty() {
                text(legal.and_then(|l| child(l, "RegistrationName")))
            } else {
                a
            }
        },
        vat_number: vat,
        tax_code: if cf.is_empty() { legal_id } else { cf },
        address: text(addr.and_then(|a| child(a, "StreetName"))),
        city: text(addr.and_then(|a| child(a, "CityName"))),
        zip: text(addr.and_then(|a| child(a, "PostalZone"))),
        province: text(addr.and_then(|a| child(a, "CountrySubentity"))),
        country: text(addr.and_then(|a| path(a, &["Country", "IdentificationCode"]))),
    }
}

/// Legge un ordine UBL (Order o OrderResponse non sono distinti: si accetta solo Order).
pub fn parse_order(xml: &str) -> Result<NsoOrder, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("XML non valido: {}", e))?;
    let root = doc.root_element();
    if root.tag_name().name() != "Order" {
        return Err(format!("Il file non è un ordine UBL (elemento radice {})", root.tag_name().name()));
    }
    let mut o = NsoOrder {
        id: text(child(root, "ID")),
        issue_date: date(&text(child(root, "IssueDate"))),
        type_code: text(child(root, "OrderTypeCode")),
        currency: text(child(root, "DocumentCurrencyCode")),
        note: root.children().filter(|c| c.is_element() && c.tag_name().name() == "Note").filter_map(|c| c.text()).collect::<Vec<_>>().join(" "),
        buyer: party(child(root, "BuyerCustomerParty")),
        accounting: party(child(root, "AccountingCustomerParty")),
        seller_endpoint: text(path(root, &["SellerSupplierParty", "Party", "EndpointID"])),
        ..Default::default()
    };
    if let Some(d) = child(root, "Delivery") {
        o.delivery_date = date(&text(path(d, &["RequestedDeliveryPeriod", "StartDate"])));
        let a = path(d, &["DeliveryLocation", "Address"]);
        let parts: Vec<String> = ["StreetName", "PostalZone", "CityName"].iter().map(|n| text(a.and_then(|a| child(a, n)))).filter(|s| !s.is_empty()).collect();
        o.delivery_address = parts.join(" ");
    }
    // CIG: in NSO è indicato in OriginatorDocumentReference o in Contract
    for name in ["OriginatorDocumentReference", "Contract"] {
        if let Some(r) = child(root, name) {
            let id = text(child(r, "ID"));
            if !id.is_empty() && o.cig.is_empty() {
                o.cig = id;
            }
        }
    }
    for ol in root.children().filter(|c| c.is_element() && c.tag_name().name() == "OrderLine") {
        let Some(li) = child(ol, "LineItem") else { continue };
        let q = child(li, "Quantity");
        let item = child(li, "Item");
        o.lines.push(NsoLine {
            id: text(child(li, "ID")),
            code: {
                let s = text(item.and_then(|i| path(i, &["SellersItemIdentification", "ID"])));
                if s.is_empty() {
                    text(item.and_then(|i| path(i, &["StandardItemIdentification", "ID"])))
                } else {
                    s
                }
            },
            description: {
                let n = text(item.and_then(|i| child(i, "Name")));
                if n.is_empty() {
                    text(item.and_then(|i| child(i, "Description")))
                } else {
                    n
                }
            },
            qty: dec(&text(q)),
            unit: q.and_then(|q| q.attribute("unitCode")).unwrap_or("").to_string(),
            price: {
                let amount = dec(&text(path(li, &["Price", "PriceAmount"])));
                let base = dec(&text(path(li, &["Price", "BaseQuantity"])));
                if base > Decimal::ZERO {
                    amount / base
                } else {
                    amount
                }
            },
            vat_rate: item.and_then(|i| path(i, &["ClassifiedTaxCategory", "Percent"])).and_then(|p| p.text()).map(dec),
            note: text(child(ol, "Note")),
        });
    }
    if o.id.is_empty() {
        return Err("Ordine senza identificativo (cbc:ID)".into());
    }
    if o.lines.is_empty() {
        return Err("Ordine senza righe".into());
    }
    Ok(o)
}

/// Ordine di esempio (dati fittizi), usato dai test e dall'archivio dimostrativo.
pub const SAMPLE_ORDER: &str = include_str!("samples/nso_ordine.xml");

#[cfg(test)]
mod tests {
    use super::*;



    #[test]
    fn parses_nso_order_and_triplet() {
        let o = parse_order(SAMPLE_ORDER).unwrap();
        assert_eq!(o.id, "110");
        assert_eq!(o.buyer.endpoint, "B9BGAQ");
        assert_eq!(o.buyer.vat_number, "04114370168");
        assert_eq!(o.accounting.endpoint, "UFE1A2");
        assert_eq!(o.cig, "Z1A2B3C4D5");
        assert_eq!(o.lines.len(), 2);
        assert_eq!(o.lines[1].price, Decimal::from_str("0.35").unwrap());
        assert_eq!(o.lines[0].code, "GUA100");
        let (id, d, c) = o.triplet();
        assert_eq!((id.as_str(), d, c.as_str()), ("110", NaiveDate::from_ymd_opt(2026, 3, 30), "#B9BGAQ#"));
        assert!(parse_order("<Invoice/>").is_err());
    }
}

