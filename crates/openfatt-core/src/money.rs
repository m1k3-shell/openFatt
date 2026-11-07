//! Aritmetica decimale, arrotondamenti e formattazione.
//!
//! Tutti gli importi e le quantità usano `rust_decimal::Decimal`: mai `f64`.
//! L'arrotondamento fiscale è "half-up" (la terza cifra >= 5 arrotonda per eccesso),
//! come richiesto dalle regole di controllo del Sistema di Interscambio (codice 00421).

use rust_decimal::prelude::*;
use rust_decimal::RoundingStrategy;

pub use rust_decimal::Decimal;

/// Arrotonda a `dp` decimali con metà per eccesso (lontano da zero).
pub fn round_dp(d: Decimal, dp: u32) -> Decimal {
    d.round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero)
}

/// Arrotonda al centesimo.
pub fn round2(d: Decimal) -> Decimal {
    round_dp(d, 2)
}

pub fn hundred() -> Decimal {
    Decimal::ONE_HUNDRED
}

/// Interpreta un numero scritto all'italiana ("1.234,56") o all'inglese ("1234.56").
/// Restituisce `None` se la stringa non è un numero valido.
pub fn parse_dec(s: &str) -> Option<Decimal> {
    let t: String = s.trim().chars().filter(|c| !c.is_whitespace() && *c != '€').collect();
    if t.is_empty() {
        return None;
    }
    let normalized = if t.contains(',') {
        // formato italiano: il punto separa le migliaia, la virgola i decimali
        t.replace('.', "").replace(',', ".")
    } else {
        let dots = t.matches('.').count();
        if dots > 1 {
            t.replace('.', "")
        } else {
            t
        }
    };
    Decimal::from_str(&normalized).ok()
}

/// Come `parse_dec`, ma una stringa vuota vale zero.
pub fn parse_dec_or_zero(s: &str) -> Option<Decimal> {
    if s.trim().is_empty() {
        Some(Decimal::ZERO)
    } else {
        parse_dec(s)
    }
}

/// Formatta con separatore delle migliaia italiano e `dp` decimali: 1.234,56
pub fn fmt_it(d: Decimal, dp: u32) -> String {
    let r = round_dp(d, dp);
    let neg = r.is_sign_negative() && !r.is_zero();
    let abs = r.abs();
    let s = format!("{:.*}", dp as usize, abs);
    let (int_part, frac_part) = match s.split_once('.') {
        Some((a, b)) => (a.to_string(), Some(b.to_string())),
        None => (s.clone(), None),
    };
    let mut grouped = String::new();
    let bytes = int_part.as_bytes();
    for (i, ch) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            grouped.push('.');
        }
        grouped.push(*ch as char);
    }
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    out.push_str(&grouped);
    if let Some(f) = frac_part {
        out.push(',');
        out.push_str(&f);
    }
    out
}

/// Formatta un importo in euro: "1.234,56"
pub fn fmt_eur(d: Decimal) -> String {
    fmt_it(d, 2)
}

/// Formatta una quantità o un prezzo togliendo gli zeri superflui, con minimo `min_dp`
/// e massimo `max_dp` decimali, in formato italiano.
pub fn fmt_it_trim(d: Decimal, min_dp: u32, max_dp: u32) -> String {
    let r = round_dp(d, max_dp).normalize();
    let scale = r.scale().max(min_dp).min(max_dp);
    fmt_it(r, scale)
}

/// Formato XML FatturaPA: punto decimale, esattamente `dp` decimali, nessun separatore.
pub fn fmt_xml(d: Decimal, dp: u32) -> String {
    let r = round_dp(d, dp);
    let r = if r.is_zero() { Decimal::ZERO } else { r };
    format!("{:.*}", dp as usize, r)
}

/// Formato XML con decimali variabili fra `min_dp` e `max_dp` (es. PrezzoUnitario 2..8).
pub fn fmt_xml_var(d: Decimal, min_dp: u32, max_dp: u32) -> String {
    let r = round_dp(d, max_dp).normalize();
    let scale = r.scale().clamp(min_dp, max_dp);
    fmt_xml(r, scale)
}

/// Percentuale di `base`: base * pct / 100, non arrotondata.
pub fn pct_of(base: Decimal, pct: Decimal) -> Decimal {
    base * pct / Decimal::ONE_HUNDRED
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn rounding_half_up_as_sdi_examples() {
        // Esempi del documento "Elenco controlli" SdI, codice 00421
        assert_eq!(round2(dec!(1500.2448)), dec!(1500.24));
        assert_eq!(round2(dec!(1500.2458)), dec!(1500.25));
        assert_eq!(round2(dec!(0.005)), dec!(0.01));
        assert_eq!(round2(dec!(-0.005)), dec!(-0.01));
    }

    #[test]
    fn parse_italian_and_english() {
        assert_eq!(parse_dec("1.234,56"), Some(dec!(1234.56)));
        assert_eq!(parse_dec("1234.56"), Some(dec!(1234.56)));
        assert_eq!(parse_dec("1.234.567"), Some(dec!(1234567)));
        assert_eq!(parse_dec(" 22 "), Some(dec!(22)));
        assert_eq!(parse_dec("€ 10,5"), Some(dec!(10.5)));
        assert_eq!(parse_dec("abc"), None);
        assert_eq!(parse_dec(""), None);
    }

    #[test]
    fn format_italian() {
        assert_eq!(fmt_eur(dec!(1234567.891)), "1.234.567,89");
        assert_eq!(fmt_eur(dec!(-5)), "-5,00");
        assert_eq!(fmt_eur(dec!(0)), "0,00");
        assert_eq!(fmt_it_trim(dec!(2.5000), 0, 4), "2,5");
        assert_eq!(fmt_it_trim(dec!(3), 0, 4), "3");
    }

    #[test]
    fn format_xml() {
        assert_eq!(fmt_xml(dec!(22), 2), "22.00");
        assert_eq!(fmt_xml(dec!(-0.004), 2), "0.00");
        assert_eq!(fmt_xml_var(dec!(12.5), 2, 8), "12.50");
        assert_eq!(fmt_xml_var(dec!(1.123456789), 2, 8), "1.12345679");
    }
}
