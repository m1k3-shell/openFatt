//! Importazione CSV di anagrafiche e articoli. Le intestazioni sono riconosciute in modo
//! tollerante (maiuscole/minuscole, accenti, sinonimi comuni).

use openfatt_core::model::{Party, PartyRole, Product};
use openfatt_core::money::parse_dec;

/// Legge un CSV (separatore ; , o tab rilevato dalla prima riga) gestendo le virgolette.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let text = text.trim_start_matches('\u{feff}');
    let first = text.lines().next().unwrap_or("");
    let sep = [';', '\t', ','].into_iter().max_by_key(|c| first.matches(*c).count()).unwrap_or(';');
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == sep {
            row.push(std::mem::take(&mut field));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut field));
            if row.iter().any(|f| !f.trim().is_empty()) {
                rows.push(std::mem::take(&mut row));
            } else {
                row.clear();
            }
        } else {
            field.push(c);
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        if row.iter().any(|f| !f.trim().is_empty()) {
            rows.push(row);
        }
    }
    rows
}

fn norm(h: &str) -> String {
    h.trim()
        .to_lowercase()
        .replace(['à', 'á'], "a")
        .replace(['è', 'é'], "e")
        .replace('ì', "i")
        .replace('ò', "o")
        .replace('ù', "u")
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn find(headers: &[String], names: &[&str]) -> Option<usize> {
    headers.iter().position(|h| names.iter().any(|n| norm(h) == norm(n)))
}

pub struct CsvResult<T> {
    pub items: Vec<T>,
    pub warnings: Vec<String>,
}

pub fn parties_from_csv(text: &str, role: PartyRole) -> CsvResult<Party> {
    let rows = parse_csv(text);
    let mut warnings = Vec::new();
    let mut items = Vec::new();
    let Some((head, data)) = rows.split_first() else { return CsvResult { items, warnings } };
    let col = |names: &[&str]| find(head, names);
    let c_code = col(&["codice", "cod", "codice cliente", "codice fornitore", "code"]);
    let c_name = col(&["denominazione", "ragione sociale", "nome completo", "cliente", "fornitore", "name"]);
    let c_first = col(&["nome", "first name"]);
    let c_last = col(&["cognome", "last name"]);
    let c_vat = col(&["partita iva", "p iva", "piva", "p.iva", "vat", "vat number"]);
    let c_cf = col(&["codice fiscale", "cf", "cod fiscale", "fiscal code"]);
    let c_addr = col(&["indirizzo", "via", "address"]);
    let c_zip = col(&["cap", "postcode", "zip"]);
    let c_city = col(&["citta", "comune", "localita", "city"]);
    let c_prov = col(&["provincia", "prov", "province"]);
    let c_country = col(&["nazione", "paese", "country"]);
    let c_email = col(&["email", "e-mail", "mail"]);
    let c_pec = col(&["pec", "indirizzo pec"]);
    let c_tel = col(&["telefono", "tel", "phone"]);
    let c_sdi = col(&["codice destinatario", "codice sdi", "sdi", "codice univoco"]);
    if c_name.is_none() && c_last.is_none() {
        warnings.push("Colonna 'Denominazione' (o 'Cognome') non trovata".into());
        return CsvResult { items, warnings };
    }
    let get = |r: &Vec<String>, c: Option<usize>| c.and_then(|i| r.get(i)).map(|s| s.trim().to_string()).unwrap_or_default();
    for (i, r) in data.iter().enumerate() {
        let mut p = Party {
            role,
            code: get(r, c_code),
            name: get(r, c_name),
            first_name: get(r, c_first),
            last_name: get(r, c_last),
            vat_number: get(r, c_vat).replace(' ', ""),
            tax_code: get(r, c_cf).replace(' ', "").to_uppercase(),
            address: get(r, c_addr),
            zip: get(r, c_zip),
            city: get(r, c_city),
            province: get(r, c_prov).to_uppercase(),
            email: get(r, c_email),
            pec: get(r, c_pec),
            phone: get(r, c_tel),
            sdi_code: get(r, c_sdi).to_uppercase(),
            ..Default::default()
        };
        let country = get(r, c_country);
        if !country.is_empty() && !country.eq_ignore_ascii_case("italia") {
            p.country = if country.len() == 2 { country.to_uppercase() } else { country };
        }
        if let Some(v) = p.vat_number.strip_prefix("IT") {
            p.vat_number = v.to_string();
        }
        if p.display_name().is_empty() {
            warnings.push(format!("Riga {}: senza nome, ignorata", i + 2));
            continue;
        }
        if p.country == "IT" && !p.vat_number.is_empty() && !openfatt_core::fiscal::partita_iva_valida(&p.vat_number) {
            warnings.push(format!("Riga {}: partita IVA '{}' non valida", i + 2, p.vat_number));
        }
        items.push(p);
    }
    CsvResult { items, warnings }
}

pub fn products_from_csv(text: &str) -> CsvResult<(Product, String)> {
    let rows = parse_csv(text);
    let mut warnings = Vec::new();
    let mut items = Vec::new();
    let Some((head, data)) = rows.split_first() else { return CsvResult { items, warnings } };
    let col = |names: &[&str]| find(head, names);
    let c_code = col(&["codice", "codice articolo", "cod", "code", "sku"]);
    let c_desc = col(&["descrizione", "description", "nome", "articolo"]);
    let c_price = col(&["prezzo", "prezzo vendita", "listino", "price", "prezzo netto"]);
    let c_vat = col(&["iva", "aliquota", "aliquota iva", "cod iva", "vat"]);
    let c_unit = col(&["um", "u.m.", "unita di misura", "unita", "unit"]);
    let c_cat = col(&["categoria", "category"]);
    let c_cost = col(&["costo", "prezzo acquisto", "cost"]);
    let c_bar = col(&["barcode", "codice a barre", "ean"]);
    if c_desc.is_none() {
        warnings.push("Colonna 'Descrizione' non trovata".into());
        return CsvResult { items, warnings };
    }
    let get = |r: &Vec<String>, c: Option<usize>| c.and_then(|i| r.get(i)).map(|s| s.trim().to_string()).unwrap_or_default();
    for (i, r) in data.iter().enumerate() {
        let desc = get(r, c_desc);
        if desc.is_empty() {
            continue;
        }
        let price_s = get(r, c_price);
        let price = if price_s.is_empty() { Some(Default::default()) } else { parse_dec(&price_s) };
        let Some(price) = price else {
            warnings.push(format!("Riga {}: prezzo '{}' non valido", i + 2, price_s));
            continue;
        };
        let unit = get(r, c_unit);
        let p = Product {
            code: get(r, c_code),
            description: desc,
            price,
            unit: if unit.is_empty() { "pz".into() } else { unit },
            category: get(r, c_cat),
            cost: parse_dec(&get(r, c_cost)).unwrap_or_default(),
            barcode: get(r, c_bar),
            ..Default::default()
        };
        items.push((p, get(r, c_vat)));
    }
    CsvResult { items, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_parties() {
        let t = "Codice;Ragione sociale;Partita IVA;Città;Note\n\"C1\";\"Rossi; Bianchi srl\";IT12345678903;Trento;x\n;;;;\n";
        let r = parties_from_csv(t, PartyRole::Customer);
        assert_eq!(r.items.len(), 1);
        assert_eq!(r.items[0].name, "Rossi; Bianchi srl");
        assert_eq!(r.items[0].vat_number, "12345678903");
        assert_eq!(r.items[0].city, "Trento");
        assert!(r.warnings.is_empty());
    }
    #[test]
    fn csv_products() {
        let t = "codice,descrizione,prezzo,iva\nA1,Vite,\"1,50\",22\nA2,Dado,xx,22\n";
        let r = products_from_csv(t);
        assert_eq!(r.items.len(), 1);
        assert_eq!(r.items[0].0.price, rust_decimal::Decimal::new(150, 2));
        assert_eq!(r.warnings.len(), 1);
    }
}
