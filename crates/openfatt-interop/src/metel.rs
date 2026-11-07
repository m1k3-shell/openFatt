//! Listini prezzi in formato Metel (materiale elettrico), versioni 020 e 021.
//!
//! Tracciato a lunghezza fissa pubblicato da Metel S.r.l. ("Listino Prezzi Metel - Barcode
//! Metel, Manuale Operativo delle Definizioni"): una testata che inizia con "LISTINO METEL"
//! e righe articolo. Posizioni (da 1) delle righe usate qui:
//! sigla marchio 1-3, codice prodotto 4-19, EAN 20-32, descrizione 33-75,
//! quantità cartone 76-80, multipla 81-85, minima 86-90, massima 91-96, lead time 97,
//! prezzo al rivenditore 98-108 e al pubblico 109-119 (due decimali impliciti),
//! moltiplicatore prezzo 120-125, valuta 126-128, unità di misura 129-131,
//! prodotto composto 132, stato 133, data ultima variazione 134-141,
//! famiglia di sconto 142-159, famiglia statistica 160-177.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MetelHeader {
    pub brand: String,
    pub vat_number: String,
    pub list_number: String,
    pub valid_from: Option<NaiveDate>,
    pub description: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MetelItem {
    pub brand: String,
    pub code: String,
    pub ean: String,
    pub description: String,
    pub box_qty: Decimal,
    pub min_qty: Decimal,
    /// Prezzo al rivenditore per unità (già diviso per il moltiplicatore)
    pub dealer_price: Decimal,
    /// Prezzo al pubblico per unità (già diviso per il moltiplicatore)
    pub public_price: Decimal,
    pub currency: String,
    pub unit: String,
    /// 1 nuovo, 2 variato, ... 9 annullato
    pub status: String,
    pub discount_family: String,
    pub stat_family: String,
}

impl MetelItem {
    /// Codice articolo come lo costruiscono i gestionali: sigla marchio + codice prodotto.
    pub fn full_code(&self) -> String {
        format!("{}{}", self.brand.trim(), self.code.trim())
    }

    pub fn is_cancelled(&self) -> bool {
        self.status.trim() == "9" || self.status.trim() == "5"
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MetelList {
    pub header: MetelHeader,
    pub items: Vec<MetelItem>,
    pub warnings: Vec<String>,
}

/// Sottostringa per posizioni Metel (da 1, estremi inclusi), su caratteri.
fn field(chars: &[char], from: usize, to: usize) -> String {
    if from == 0 || from > chars.len() {
        return String::new();
    }
    let end = to.min(chars.len());
    chars[from - 1..end].iter().collect::<String>().trim().to_string()
}

fn num(s: &str, decimals: u32) -> Option<Decimal> {
    let t = s.trim();
    if t.is_empty() {
        return Some(Decimal::ZERO);
    }
    if !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let v = Decimal::from_str(t).ok()?;
    Some(v / Decimal::from(10u64.pow(decimals)))
}

/// I file Metel sono testo a 8 bit (Windows-1252): si decodifica senza perdere le lettere accentate.
pub fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

pub fn parse(text: &str) -> Result<MetelList, String> {
    let mut out = MetelList::default();
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let first = lines.next().ok_or("File vuoto")?;
    let h: Vec<char> = first.chars().collect();
    if !field(&h, 1, 20).to_uppercase().starts_with("LISTINO METEL") {
        return Err("Non è un listino Metel: la prima riga deve iniziare con \"LISTINO METEL\"".into());
    }
    out.header = MetelHeader {
        brand: field(&h, 21, 23),
        vat_number: field(&h, 24, 34),
        list_number: field(&h, 35, 40),
        valid_from: NaiveDate::parse_from_str(&field(&h, 41, 48), "%Y%m%d").ok(),
        description: field(&h, 57, 86),
        version: field(&h, 126, 128),
    };
    for (i, line) in lines.enumerate() {
        let c: Vec<char> = line.chars().collect();
        let n = i + 2;
        if c.len() < 133 {
            out.warnings.push(format!("Riga {}: troppo corta ({} caratteri), saltata", n, c.len()));
            continue;
        }
        let mult = num(&field(&c, 120, 125), 0).filter(|m| !m.is_zero()).unwrap_or(Decimal::ONE);
        let (Some(dealer), Some(public)) = (num(&field(&c, 98, 108), 2), num(&field(&c, 109, 119), 2)) else {
            out.warnings.push(format!("Riga {}: prezzi non numerici, saltata", n));
            continue;
        };
        let code = field(&c, 4, 19);
        if code.is_empty() {
            out.warnings.push(format!("Riga {}: codice prodotto mancante", n));
            continue;
        }
        out.items.push(MetelItem {
            brand: field(&c, 1, 3),
            code,
            ean: field(&c, 20, 32).trim_start_matches('0').is_empty().then(String::new).unwrap_or_else(|| field(&c, 20, 32)),
            description: field(&c, 33, 75),
            box_qty: num(&field(&c, 76, 80), 0).unwrap_or_default(),
            min_qty: num(&field(&c, 86, 90), 0).unwrap_or_default(),
            dealer_price: openfatt_core::money::round_dp(dealer / mult, 6),
            public_price: openfatt_core::money::round_dp(public / mult, 6),
            currency: field(&c, 126, 128),
            unit: field(&c, 129, 131),
            status: field(&c, 133, 133),
            discount_family: field(&c, 142, 159),
            stat_family: field(&c, 160, 177),
        });
    }
    Ok(out)
}

/// Riga Metel per i test e per esportare: costruita campo per campo.
pub fn format_item(it: &MetelItem, multiplier: u32) -> String {
    let price = |p: Decimal| format!("{:011}", (p * Decimal::from(multiplier) * Decimal::ONE_HUNDRED).round().to_string().parse::<u64>().unwrap_or(0));
    format!(
        "{:<3}{:<16}{:<13}{:<43}{:05}{:05}{:05}{:06}{}{}{}{:06}{:<3}{:<3}{}{}{:<8}{:<18}{:<18}",
        it.brand,
        it.code,
        it.ean,
        it.description.chars().take(43).collect::<String>(),
        it.box_qty.round().to_string().parse::<u32>().unwrap_or(0),
        1,
        it.min_qty.round().to_string().parse::<u32>().unwrap_or(0),
        0,
        "1",
        price(it.dealer_price),
        price(it.public_price),
        multiplier,
        it.currency,
        it.unit,
        "0",
        if it.status.is_empty() { "2" } else { &it.status },
        "20260101",
        it.discount_family,
        it.stat_family
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> String {
        format!("{:<20}{:<3}{:<11}{:<6}{}{}{:<30}{:<39}{}", "LISTINO METEL", "ABC", "01234567890", "000001", "20260101", "20251215", "Listino generale 2026", "", "020")
    }

    #[test]
    fn parses_header_and_items_with_multiplier() {
        let a = MetelItem { brand: "ABC".into(), code: "INT10A".into(), ean: "8012345678901".into(), description: "Interruttore 10A bianco".into(), box_qty: Decimal::from(10), min_qty: Decimal::ONE, dealer_price: Decimal::from_str("2.35").unwrap(), public_price: Decimal::from_str("4.10").unwrap(), currency: "EUR".into(), unit: "PCE".into(), status: "1".into(), discount_family: "FAM01".into(), stat_family: "S1".into() };
        // cavo: prezzo riferito a 100 metri (moltiplicatore 100)
        let b = MetelItem { code: "CAVO15".into(), description: "Cavo FS17 1,5 mmq".into(), unit: "MTR".into(), dealer_price: Decimal::from_str("0.42").unwrap(), public_price: Decimal::from_str("0.80").unwrap(), ..a.clone() };
        let text = format!("{}\r\n{}\r\n{}\r\nRIGA CORTA\r\n", header(), format_item(&a, 1), format_item(&b, 100));
        let l = parse(&text).unwrap();
        assert_eq!(l.header.brand, "ABC");
        assert_eq!(l.header.valid_from, NaiveDate::from_ymd_opt(2026, 1, 1));
        assert_eq!(l.header.version, "020");
        assert_eq!(l.items.len(), 2);
        assert_eq!(l.items[0].full_code(), "ABCINT10A");
        assert_eq!(l.items[0].dealer_price, Decimal::from_str("2.35").unwrap());
        assert_eq!(l.items[0].ean, "8012345678901");
        assert_eq!(l.items[1].public_price, Decimal::from_str("0.80").unwrap());
        assert_eq!(l.items[1].unit, "MTR");
        assert_eq!(l.warnings.len(), 1);
        assert!(parse("ALTRO FILE").is_err());
    }
}
