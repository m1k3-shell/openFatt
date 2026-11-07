//! Formato Easyfatt-XML (https://www.danea.it/software/easyfatt/xml/).
//! Le regole pubbliche del formato distinguono campo omesso, vuoto e zero: un campo
//! omesso lascia il valore predefinito, per questo i campi sono letti come Option.

use std::str::FromStr;

use chrono::NaiveDate;
use openfatt_core::calc;
use openfatt_core::model::*;
use rust_decimal::Decimal;

#[derive(Debug, Clone)]
pub struct ImportedDocument {
    pub doc: Document,
    pub customer_code: String,
    /// Totale dichiarato nel file (se presente), da confrontare con il ricalcolo.
    pub declared_total: Option<Decimal>,
    pub recomputed_total: Option<Decimal>,
    pub warnings: Vec<String>,
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn text(n: roxmltree::Node, name: &str) -> Option<String> {
    child(n, name).map(|c| c.text().unwrap_or("").trim().to_string())
}

fn text_or(n: roxmltree::Node, name: &str) -> String {
    text(n, name).unwrap_or_default()
}

fn dec(n: roxmltree::Node, name: &str) -> Option<Decimal> {
    text(n, name).filter(|s| !s.is_empty()).and_then(|s| Decimal::from_str(&s.replace(',', ".")).ok())
}

fn boolean(n: roxmltree::Node, name: &str) -> Option<bool> {
    text(n, name).map(|s| s.eq_ignore_ascii_case("true") || s == "1")
}

pub fn kind_from_code(code: &str) -> (Option<DocKind>, &'static str) {
    match code {
        "A" => (Some(DocKind::Proforma), "Avviso di parcella importato come proforma"),
        "C" | "" => (Some(DocKind::CustomerOrder), ""),
        "D" => (Some(DocKind::DeliveryNote), ""),
        "E" => (Some(DocKind::SupplierOrder), ""),
        "F" => (Some(DocKind::AccompanyingInvoice), ""),
        "I" => (Some(DocKind::Invoice), ""),
        "J" => (Some(DocKind::DownPaymentInvoice), ""),
        "L" => (Some(DocKind::Proforma), ""),
        "M" => (Some(DocKind::SelfInvoice), ""),
        "N" => (Some(DocKind::CreditNote), ""),
        "O" => (Some(DocKind::DebitNote), ""),
        "P" => (Some(DocKind::Fee), ""),
        "Q" => (Some(DocKind::Quote), ""),
        "H" => (Some(DocKind::GoodsReceipt), ""),
        "B" => (Some(DocKind::RetailSale), "Vendita al banco importata senza estremi del documento commerciale"),
        "R" => (None, "Ricevuta fiscale: non importata"),
        "G" => (Some(DocKind::Quote), "Rapporto d'intervento importato come preventivo"),
        "S" => (Some(DocKind::SupplierOrder), "Preventivo fornitore importato come ordine fornitore"),
        _ => (None, "Tipo documento sconosciuto"),
    }
}

pub fn kind_to_code(k: DocKind) -> &'static str {
    match k {
        DocKind::Quote => "Q",
        DocKind::CustomerOrder => "C",
        DocKind::DeliveryNote => "D",
        DocKind::Invoice => "I",
        DocKind::AccompanyingInvoice => "F",
        DocKind::CreditNote => "N",
        DocKind::DebitNote => "O",
        DocKind::DownPaymentInvoice => "J",
        DocKind::Fee => "P",
        DocKind::Proforma => "L",
        DocKind::SelfInvoice => "M",
        DocKind::SupplierOrder => "E",
        DocKind::GoodsReceipt | DocKind::PurchaseInvoice | DocKind::PurchaseCreditNote => "H",
        DocKind::RetailSale => "B",
        DocKind::StockTransfer | DocKind::Production => "D",
    }
}

/// Ricava aliquota/natura dall'elemento VatCode (codice + attributi Perc, Class, Description).
fn vat_from(node: Option<roxmltree::Node>, vat_codes: &[VatCode], warnings: &mut Vec<String>, line_no: usize) -> VatRef {
    let Some(n) = node else { return VatRef::default() };
    let code = n.text().unwrap_or("").trim().to_string();
    let perc = n.attribute("Perc").and_then(|p| Decimal::from_str(p).ok());
    let class = n.attribute("Class").unwrap_or("").to_string();
    let descr = n.attribute("Description").unwrap_or("").to_string();
    // 1) corrispondenza esatta con un codice dell'archivio
    if let Some(v) = vat_codes.iter().find(|v| v.code.eq_ignore_ascii_case(&code)) {
        return VatRef::from_code(v);
    }
    let rate = perc.or_else(|| Decimal::from_str(&code.replace(',', ".")).ok());
    let natura_hint = [code.as_str(), class.as_str()]
        .iter()
        .find(|s| s.starts_with('N') && openfatt_core::codes::is_valid(openfatt_core::codes::NATURA, s))
        .map(|s| s.to_string());
    match rate {
        Some(r) if !r.is_zero() => {
            if let Some(v) = vat_codes.iter().find(|v| v.rate == r && v.natura.is_none()) {
                VatRef::from_code(v)
            } else {
                VatRef { vat_id: None, rate: r, natura: None, legal_ref: String::new() }
            }
        }
        _ => {
            if let Some(nat) = natura_hint {
                if let Some(v) = vat_codes.iter().find(|v| v.natura.as_deref() == Some(nat.as_str())) {
                    return VatRef::from_code(v);
                }
                return VatRef { vat_id: None, rate: Decimal::ZERO, natura: Some(nat), legal_ref: descr };
            }
            warnings.push(format!("Riga {}: codice IVA '{}' a zero senza natura: indicare la natura prima di emettere", line_no, code));
            VatRef { vat_id: None, rate: Decimal::ZERO, natura: None, legal_ref: descr }
        }
    }
}

fn country_code(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("italia") || t.eq_ignore_ascii_case("italy") {
        "IT".into()
    } else if t.len() == 2 {
        t.to_ascii_uppercase()
    } else {
        t.to_string()
    }
}

/// Legge un file Easyfatt-XML. `vat_codes` serve a riconoscere le aliquote dell'archivio.
pub fn parse_documents(xml: &str, vat_codes: &[VatCode]) -> Result<Vec<ImportedDocument>, String> {
    let opts = roxmltree::ParsingOptions { allow_dtd: false, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(xml.trim_start_matches('\u{feff}'), opts).map_err(|e| format!("XML non valido: {}", e))?;
    let root = doc.root_element();
    if root.tag_name().name() != "EasyfattDocuments" {
        return Err("Il file non è in formato Easyfatt-XML (manca EasyfattDocuments)".into());
    }
    let docs = child(root, "Documents").ok_or("Elemento Documents mancante")?;
    let mut out = Vec::new();
    for d in docs.children().filter(|c| c.is_element() && c.tag_name().name() == "Document") {
        let mut warnings = Vec::new();
        let code = text_or(d, "DocumentType");
        let (kind, note) = kind_from_code(&code);
        if !note.is_empty() {
            warnings.push(note.to_string());
        }
        let Some(kind) = kind else { continue };
        let mut doc = Document::new(kind);
        if let Some(date) = text(d, "Date").and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()) {
            doc.date = date;
        }
        doc.number = text(d, "Number").and_then(|s| s.trim().parse::<i64>().ok());
        doc.series = text_or(d, "Numbering").trim_start_matches('/').to_string();
        if kind.is_purchase() {
            doc.external_number = text_or(d, "Number");
            doc.number = None;
        }
        let name = text_or(d, "CustomerName");
        doc.party = Party {
            code: text_or(d, "CustomerCode"),
            role: if kind.is_purchase() { PartyRole::Supplier } else { PartyRole::Customer },
            name,
            address: text_or(d, "CustomerAddress"),
            zip: text_or(d, "CustomerPostcode"),
            city: text_or(d, "CustomerCity"),
            province: text_or(d, "CustomerProvince"),
            country: country_code(&text_or(d, "CustomerCountry")),
            vat_number: text_or(d, "CustomerVatCode"),
            tax_code: text_or(d, "CustomerFiscalCode"),
            phone: text_or(d, "CustomerTel"),
            mobile: text_or(d, "CustomerCellPhone"),
            email: text_or(d, "CustomerEmail"),
            pec: text_or(d, "CustomerPec"),
            ..Default::default()
        };
        let delivery: Vec<String> = ["DeliveryName", "DeliveryAddress", "DeliveryPostcode", "DeliveryCity", "DeliveryProvince"]
            .iter()
            .map(|k| text_or(d, k))
            .filter(|s| !s.is_empty())
            .collect();
        doc.destination = delivery.join("\n");
        doc.prices_include_vat = boolean(d, "PricesIncludeVat").unwrap_or(false);
        doc.warehouse = text_or(d, "Warehouse");
        doc.notes = text_or(d, "FootNotes");
        doc.internal_notes = text_or(d, "InternalComment");
        if let Some(r) = text(d, "CustomerReference").filter(|s| !s.is_empty()) {
            doc.related.push(RelatedDoc { kind: "ORDINE".into(), number: r, ..Default::default() });
        }
        if let Some(w) = dec(d, "WithholdingTaxPerc").filter(|w| !w.is_zero()) {
            doc.withholdings.push(Withholding { rate: w, ..Default::default() });
        }
        if kind.has_transport() || kind == DocKind::AccompanyingInvoice {
            let t = doc.transport.get_or_insert_with(Transport::default);
            t.carrier = text_or(d, "Carrier");
            t.reason = text_or(d, "TransportReason");
            t.appearance = text_or(d, "GoodsAppearance");
            t.port = text_or(d, "ShipmentTerms");
            t.packages = text(d, "NumOfPieces").and_then(|s| s.parse().ok());
            t.gross_weight = dec(d, "TransportedWeight");
        }
        let payment_name = text_or(d, "PaymentName");
        if !payment_name.is_empty() {
            doc.internal_notes = format!("{}\nPagamento originale: {}", doc.internal_notes, payment_name).trim().to_string();
        }

        if let Some(rows) = child(d, "Rows") {
            for (i, r) in rows.children().filter(|c| c.is_element() && c.tag_name().name() == "Row").enumerate() {
                let qty = dec(r, "Qty");
                let price = dec(r, "Price");
                let description = text_or(r, "Description");
                if qty.is_none() && price.is_none() {
                    doc.lines.push(DocLine { kind: LineKind::Text, description, ..Default::default() });
                    continue;
                }
                let vat = vat_from(child(r, "VatCode"), vat_codes, &mut warnings, i + 1);
                let mut line = DocLine {
                    code: text_or(r, "Code"),
                    description,
                    qty: qty.unwrap_or(Decimal::ONE),
                    unit: text_or(r, "Um"),
                    unit_price: price.unwrap_or_default(),
                    discounts: text_or(r, "Discounts").replace('%', ""),
                    vat,
                    withholding: boolean(r, "WithholdingTax").unwrap_or(false),
                    lot: text_or(r, "Lot"),
                    lot_expiry: text(r, "ExpiryDate").and_then(|s| NaiveDate::parse_from_str(s.get(..10).unwrap_or(&s), "%Y-%m-%d").ok()),
                    variant: [text_or(r, "Size"), text_or(r, "Color")].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join("/"),
                    serials: text_or(r, "Serial").split([',', ';']).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
                    ..Default::default()
                };
                if let (Some(total), true) = (dec(r, "Total"), line.discounts.is_empty()) {
                    // se il totale dichiarato non torna con prezzo x quantità si segnala
                    let expected = openfatt_core::money::round2(line.unit_price * line.qty);
                    if (expected - total).abs() > Decimal::new(1, 2) {
                        warnings.push(format!("Riga {}: totale dichiarato {} diverso da prezzo x quantità {}", i + 1, total, expected));
                    }
                }
                if line.vat.rate.is_zero() && line.vat.natura.is_none() && line.unit_price.is_zero() {
                    line.vat = VatRef { vat_id: None, rate: Decimal::from(22), natura: None, legal_ref: String::new() };
                }
                doc.lines.push(line);
            }
        }
        if let Some(pays) = child(d, "Payments") {
            for p in pays.children().filter(|c| c.is_element() && c.tag_name().name() == "Payment") {
                let date = text(p, "Date").and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()).unwrap_or(doc.date);
                let amount = dec(p, "Amount").unwrap_or_default();
                let paid = boolean(p, "Paid").unwrap_or(false);
                doc.due_dates.push(DueDate { date, amount, paid: if paid { amount } else { Decimal::ZERO }, paid_date: if paid { Some(date) } else { None }, ..Default::default() });
            }
        }
        let declared_total = dec(d, "Total");
        let recomputed_total = calc::compute(&doc).ok().map(|t| t.total);
        match (declared_total, recomputed_total) {
            (Some(a), Some(b)) if (a - b).abs() > Decimal::new(1, 2) => {
                warnings.push(format!("Totale dichiarato {} diverso dal ricalcolo {}", a, b));
            }
            (_, None) => warnings.push("Il documento non è calcolabile: verificare aliquote e righe".into()),
            _ => {}
        }
        out.push(ImportedDocument { customer_code: doc.party.code.clone(), doc, declared_total, recomputed_total, warnings });
    }
    Ok(out)
}

fn esc(s: &str) -> String {
    openfatt_fatturapa::write::escape(s)
}

/// Esporta documenti nel formato Easyfatt-XML (per altri programmi compatibili).
pub fn export_documents(company: &Company, docs: &[(Document, calc::Totals)]) -> String {
    let mut o = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    o.push_str("<EasyfattDocuments AppVersion=\"2\" Creator=\"openFatt\">\n  <Company>\n");
    for (k, v) in [
        ("Name", company.display_name()),
        ("Address", company.address.clone()),
        ("Postcode", company.zip.clone()),
        ("City", company.city.clone()),
        ("Province", company.province.clone()),
        ("FiscalCode", company.tax_code.clone()),
        ("VatCode", company.vat_number.clone()),
        ("Email", company.email.clone()),
    ] {
        o.push_str(&format!("    <{0}>{1}</{0}>\n", k, esc(&v)));
    }
    o.push_str("  </Company>\n  <Documents>\n");
    for (d, t) in docs {
        let p = &d.party;
        o.push_str("    <Document>\n");
        let fields = [
            ("DocumentType", kind_to_code(d.kind).to_string()),
            ("Date", d.date.format("%Y-%m-%d").to_string()),
            ("Number", d.number.map(|n| n.to_string()).unwrap_or_default()),
            ("Numbering", if d.series.is_empty() { String::new() } else { format!("/{}", d.series) }),
            ("CustomerCode", p.code.clone()),
            ("CustomerName", p.display_name()),
            ("CustomerAddress", p.full_address()),
            ("CustomerPostcode", p.zip.clone()),
            ("CustomerCity", p.city.clone()),
            ("CustomerProvince", p.province.clone()),
            ("CustomerCountry", if p.country == "IT" { "Italia".into() } else { p.country.clone() }),
            ("CustomerFiscalCode", p.tax_code.clone()),
            ("CustomerVatCode", p.vat_number.clone()),
            ("CustomerEmail", p.email.clone()),
            ("CustomerPec", p.pec.clone()),
            ("TotalWithoutTax", t.taxable.to_string()),
            ("VatAmount", t.tax.to_string()),
            ("WithholdingTaxAmount", t.withholding_total.to_string()),
            ("Total", t.total.to_string()),
            ("PricesIncludeVat", d.prices_include_vat.to_string()),
            ("PaymentName", d.payment.as_ref().map(|p| p.name.clone()).unwrap_or_default()),
            ("FootNotes", d.notes.clone()),
        ];
        for (k, v) in fields {
            o.push_str(&format!("      <{0}>{1}</{0}>\n", k, esc(&v)));
        }
        o.push_str("      <Rows>\n");
        for l in &d.lines {
            o.push_str("        <Row>\n");
            if l.kind == LineKind::Text {
                o.push_str(&format!("          <Description>{}</Description>\n", esc(&l.description)));
            } else {
                let lt = openfatt_core::money::round2(calc::apply_cascade(l.unit_price, &calc::parse_discounts(&l.discounts).unwrap_or_default()) * l.qty);
                let vat_code = match &l.vat.natura {
                    Some(n) => n.clone(),
                    None => l.vat.rate.normalize().to_string(),
                };
                o.push_str(&format!(
                    "          <Code>{}</Code>\n          <Description>{}</Description>\n          <Qty>{}</Qty>\n          <Um>{}</Um>\n          <Price>{}</Price>\n          <Discounts>{}</Discounts>\n          <VatCode Perc=\"{}\">{}</VatCode>\n          <Total>{}</Total>\n          <WithholdingTax>{}</WithholdingTax>\n",
                    esc(&l.code), esc(&l.description), l.qty.normalize(), esc(&l.unit), l.unit_price.normalize(), esc(&l.discounts), l.vat.rate.normalize(), esc(&vat_code), lt, l.withholding
                ));
            }
            o.push_str("        </Row>\n");
        }
        o.push_str("      </Rows>\n");
        if !d.due_dates.is_empty() {
            o.push_str("      <Payments>\n");
            for dd in &d.due_dates {
                o.push_str(&format!(
                    "        <Payment>\n          <Advance>false</Advance>\n          <Date>{}</Date>\n          <Amount>{}</Amount>\n          <Paid>{}</Paid>\n        </Payment>\n",
                    dd.date.format("%Y-%m-%d"),
                    dd.amount,
                    dd.is_paid()
                ));
            }
            o.push_str("      </Payments>\n");
        }
        o.push_str("    </Document>\n");
    }
    o.push_str("  </Documents>\n</EasyfattDocuments>\n");
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vats() -> Vec<VatCode> {
        vec![
            VatCode { id: 1, code: "22".into(), rate: Decimal::from(22), active: true, ..Default::default() },
            VatCode { id: 2, code: "N4".into(), rate: Decimal::ZERO, natura: Some("N4".into()), active: true, ..Default::default() },
        ]
    }

    #[test]
    fn official_example() {
        // file di esempio pubblicato con la specifica Easyfatt-XML (non incluso nella repository):
        // OPENFATT_EASYFATT_ESEMPIO=percorso\Documenti-esempio.xml cargo test
        let Some(p) = std::env::var_os("OPENFATT_EASYFATT_ESEMPIO") else { return };
        let Ok(xml) = std::fs::read_to_string(&p) else { return };
        let docs = parse_documents(&xml, &vats()).unwrap();
        assert_eq!(docs.len(), 5);
        let first = &docs[0];
        assert_eq!(first.doc.kind, DocKind::Invoice);
        assert_eq!(first.doc.number, Some(69));
        assert_eq!(first.doc.party.name, "Il Puffetto");
        assert_eq!(first.doc.due_dates.len(), 2);
        assert!(first.doc.lines.iter().any(|l| l.kind == LineKind::Text));
        // l'esempio usa l'aliquota storica del 20%: il ricalcolo deve coincidere con il dichiarato
        assert_eq!(first.declared_total, first.recomputed_total, "{:?}", first.warnings);
    }

    #[test]
    fn roundtrip_export_import() {
        let company = Company { name: "Prova".into(), ..Default::default() };
        let mut d = Document::new(DocKind::Invoice);
        d.number = Some(3);
        d.party = Party { code: "C1".into(), name: "Cliente & Figli".into(), ..Default::default() };
        d.lines.push(DocLine { code: "X".into(), description: "Riga".into(), qty: Decimal::from(2), unit_price: Decimal::new(1050, 2), vat: VatRef::from_code(&vats()[0]), ..Default::default() });
        let t = calc::compute(&d).unwrap();
        let xml = export_documents(&company, &[(d.clone(), t.clone())]);
        let back = parse_documents(&xml, &vats()).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].doc.party.name, "Cliente & Figli");
        assert_eq!(back[0].recomputed_total, Some(t.total));
    }
}
