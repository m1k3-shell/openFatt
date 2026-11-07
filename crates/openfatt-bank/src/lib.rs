//! File da caricare nell'home banking:
//! - Ri.Ba.: tracciato CBI a record fissi di 120 caratteri (`riba`);
//! - addebiti diretti SEPA (SDD Core e B2B): XML CBI `CBISDDReqLogMsg.00.01.01` (`sepa`);
//! - bonifici SEPA (SCT): XML CBI `CBIPaymentRequest.00.04.01` (`sepa`).
//!
//! Fonti: standard CBI pubblici (schemi XSD in `specs/cbi`) e regolamento SEPA dell'EPC.
//! Le funzioni sono pure: ricevono i dati già raccolti e restituiscono il testo del file,
//! oppure l'elenco dei problemi da correggere (nessun file parziale).

pub mod riba;
pub mod sepa;

use chrono::NaiveDate;
use rust_decimal::Decimal;

/// Dati dell'azienda che presenta la distinta.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Company {
    pub name: String,
    pub address: String,
    pub zip: String,
    pub city: String,
    pub province: String,
    /// Partita IVA o codice fiscale.
    pub tax_id: String,
    pub iban: String,
    pub bic: String,
    /// Codice SIA (Ri.Ba.).
    pub sia: String,
    /// Codice CUC (bonifici e SDD in formato CBI).
    pub cuc: String,
    /// Identificativo del creditore SEPA (Creditor Identifier, SDD).
    pub creditor_id: String,
}

/// Elenco dei problemi che impediscono di creare il file.
pub type Problems = Vec<String>;

pub fn normalize_iban(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase()
}

/// ABI, CAB e numero di conto di un IBAN italiano valido.
pub fn it_bank_codes(iban: &str) -> Option<(String, String, String)> {
    let s = normalize_iban(iban);
    if !s.starts_with("IT") || !openfatt_core::fiscal::iban_valido(&s) {
        return None;
    }
    Some((s[5..10].to_string(), s[10..15].to_string(), s[15..27].to_string()))
}

/// Controllo dell'identificativo del creditore SEPA (es. IT66ZZZA1B2C3D4E5F6G7H8):
/// paese, cifre di controllo (modulo 97 sull'identificativo nazionale, escluso il codice
/// attività di tre caratteri), identificativo nazionale.
pub fn creditor_id_valid(id: &str) -> bool {
    let s = normalize_iban(id);
    if s.len() < 8 || s.len() > 35 || !s.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let (cc, kk, national) = (&s[0..2], &s[2..4], &s[7..]);
    if !cc.chars().all(|c| c.is_ascii_uppercase()) || !kk.chars().all(|c| c.is_ascii_digit()) || national.is_empty() {
        return false;
    }
    let digits: String = format!("{}{}00", national, cc).chars().map(|c| if c.is_ascii_digit() { c.to_string() } else { (c as u32 - 'A' as u32 + 10).to_string() }).collect();
    let mut rem: u32 = 0;
    for ch in digits.chars() {
        rem = (rem * 10 + ch.to_digit(10).unwrap()) % 97;
    }
    format!("{:02}", 98 - rem) == kk
}

/// Testo ammesso nei messaggi SEPA (insieme di caratteri latini di base), accenti tolti,
/// lunghezza massima `max`.
pub fn sepa_text(s: &str, max: usize) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let mapped = deaccent(c);
        for m in mapped.chars() {
            let ok = m.is_ascii_alphanumeric() || "/-?:().,'+ ".contains(m);
            out.push(if ok { m } else if m == '&' { '+' } else { ' ' });
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(max).collect::<String>().trim_end().to_string()
}

/// Testo per i record RiBa: maiuscolo, senza accenti, solo caratteri stampabili ASCII.
pub fn riba_text(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        for m in deaccent(c).chars() {
            out.push(if m.is_ascii_graphic() || m == ' ' { m.to_ascii_uppercase() } else { ' ' });
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn deaccent(c: char) -> String {
    let m = match c {
        'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => "a",
        'À' | 'Á' | 'Â' | 'Ä' | 'Ã' | 'Å' => "A",
        'è' | 'é' | 'ê' | 'ë' => "e",
        'È' | 'É' | 'Ê' | 'Ë' => "E",
        'ì' | 'í' | 'î' | 'ï' => "i",
        'Ì' | 'Í' | 'Î' | 'Ï' => "I",
        'ò' | 'ó' | 'ô' | 'ö' | 'õ' => "o",
        'Ò' | 'Ó' | 'Ô' | 'Ö' | 'Õ' => "O",
        'ù' | 'ú' | 'û' | 'ü' => "u",
        'Ù' | 'Ú' | 'Û' | 'Ü' => "U",
        'ç' => "c",
        'Ç' => "C",
        'ñ' => "n",
        'Ñ' => "N",
        'ß' => "ss",
        '’' | '‘' | '`' | '´' => "'",
        '“' | '”' | '"' => "",
        '–' | '—' => "-",
        '€' => "EUR",
        _ => return c.to_string(),
    };
    m.to_string()
}

/// Importo in centesimi (arrotondato a 2 decimali).
pub fn cents(d: Decimal) -> i64 {
    use rust_decimal::prelude::ToPrimitive;
    (d.round_dp(2) * Decimal::ONE_HUNDRED).to_i64().unwrap_or(0)
}

pub fn fmt_ddmmyy(d: NaiveDate) -> String {
    d.format("%d%m%y").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codici_banca_e_identificativo_creditore() {
        assert_eq!(it_bank_codes("IT60 X054 2811 1010 0000 0123 456"), Some(("05428".into(), "11101".into(), "000000123456".into())));
        assert_eq!(it_bank_codes("IT60X0542811101000000123457"), None);
        assert_eq!(it_bank_codes("DE89370400440532013000"), None);
        // esempio pubblicato dall'EPC per l'identificativo del creditore
        assert!(creditor_id_valid("DE98ZZZ09999999999"));
        assert!(!creditor_id_valid("DE97ZZZ09999999999"));
        assert!(creditor_id_valid(&format!("IT{}ZZZ{}", check("12345678901", "IT"), "12345678901")));
    }

    fn check(national: &str, cc: &str) -> String {
        for k in 0..100 {
            let id = format!("{}{:02}ZZZ{}", cc, k, national);
            if creditor_id_valid(&id) {
                return format!("{:02}", k);
            }
        }
        panic!()
    }

    #[test]
    fn testi() {
        assert_eq!(sepa_text("Città & Società «Più» S.r.l.", 70), "Citta + Societa Piu S.r.l.");
        assert_eq!(sepa_text("abcdefghij", 4), "abcd");
        assert_eq!(riba_text("Caffè Ñandù s.n.c."), "CAFFE NANDU S.N.C.");
        assert_eq!(cents(rust_decimal_macros::dec!(1220.005)), 122000);
        assert_eq!(cents(rust_decimal_macros::dec!(0.1)), 10);
    }
}
