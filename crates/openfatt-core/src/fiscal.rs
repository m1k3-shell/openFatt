//! Controlli formali su partita IVA, codice fiscale, IBAN e codice destinatario.
//! Sono controlli di forma e di carattere di controllo: l'esistenza in Anagrafe
//! Tributaria (errori SdI 00301/00305) si può verificare solo con i servizi dell'Agenzia.

/// Partita IVA italiana: 11 cifre con carattere di controllo (algoritmo di Luhn modificato).
pub fn partita_iva_valida(piva: &str) -> bool {
    let p = piva.trim();
    if p.len() != 11 || !p.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if p == "00000000000" {
        return false;
    }
    let d: Vec<u32> = p.bytes().map(|b| (b - b'0') as u32).collect();
    let mut sum = 0;
    for (i, v) in d.iter().take(10).enumerate() {
        if i % 2 == 0 {
            sum += v;
        } else {
            let x = v * 2;
            sum += if x > 9 { x - 9 } else { x };
        }
    }
    let check = (10 - sum % 10) % 10;
    check == d[10]
}

const ODD: [u32; 36] = [
    // valori per i caratteri in posizione dispari (1a, 3a, ...): 0-9 poi A-Z
    1, 0, 5, 7, 9, 13, 15, 17, 19, 21, // 0-9
    1, 0, 5, 7, 9, 13, 15, 17, 19, 21, 2, 4, 18, 20, 11, 3, 6, 8, 12, 14, 16, 10, 22, 25, 24, 23,
];

fn even_value(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32),
        b'A'..=b'Z' => Some((c - b'A') as u32),
        _ => None,
    }
}

fn odd_value(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some(ODD[(c - b'0') as usize]),
        b'A'..=b'Z' => Some(ODD[10 + (c - b'A') as usize]),
        _ => None,
    }
}

/// Codice fiscale di persona fisica (16 caratteri, con omocodia) oppure numerico a 11 cifre.
pub fn codice_fiscale_valido(cf: &str) -> bool {
    let c = cf.trim().to_ascii_uppercase();
    if c.len() == 11 {
        return partita_iva_valida(&c);
    }
    if c.len() != 16 {
        return false;
    }
    let b = c.as_bytes();
    // struttura: 6 lettere, 2 "cifre", lettera mese, 2 "cifre", lettera, 3 "cifre", controllo
    let omocodia = b"LMNPQRSTUV";
    let is_digitlike = |x: u8| x.is_ascii_digit() || omocodia.contains(&x);
    let pattern_ok = b[..6].iter().all(|x| x.is_ascii_uppercase())
        && is_digitlike(b[6])
        && is_digitlike(b[7])
        && b"ABCDEHLMPRST".contains(&b[8])
        && is_digitlike(b[9])
        && is_digitlike(b[10])
        && b[11].is_ascii_uppercase()
        && is_digitlike(b[12])
        && is_digitlike(b[13])
        && is_digitlike(b[14])
        && b[15].is_ascii_uppercase();
    if !pattern_ok {
        return false;
    }
    let mut sum = 0u32;
    for (i, &ch) in b.iter().take(15).enumerate() {
        let v = if i % 2 == 0 { odd_value(ch) } else { even_value(ch) };
        match v {
            Some(v) => sum += v,
            None => return false,
        }
    }
    let expected = b'A' + (sum % 26) as u8;
    expected == b[15]
}

/// IBAN: controllo modulo 97 (ISO 13616). Per l'Italia la lunghezza è 27.
pub fn iban_valido(iban: &str) -> bool {
    let s: String = iban.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase();
    if s.len() < 15 || s.len() > 34 || !s.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    if s.starts_with("IT") && s.len() != 27 {
        return false;
    }
    let rearranged = format!("{}{}", &s[4..], &s[..4]);
    let mut rem: u64 = 0;
    for ch in rearranged.chars() {
        let v = if ch.is_ascii_digit() {
            ch as u64 - '0' as u64
        } else {
            ch as u64 - 'A' as u64 + 10
        };
        if v >= 10 {
            rem = (rem * 100 + v) % 97;
        } else {
            rem = (rem * 10 + v) % 97;
        }
    }
    rem == 1
}

/// Codice destinatario: 7 caratteri per privati (FPR12), 6 per la PA (FPA12).
pub fn codice_destinatario_valido(code: &str, pa: bool) -> bool {
    let len = if pa { 6 } else { 7 };
    code.len() == len && code.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

pub fn cap_valido(cap: &str) -> bool {
    cap.len() == 5 && cap.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piva() {
        assert!(partita_iva_valida("12345678903"));
        assert!(!partita_iva_valida("12345678901"));
        assert!(!partita_iva_valida("1234567890"));
        assert!(!partita_iva_valida("00000000000"));
        // Partita IVA dell'Agenzia delle Entrate
        assert!(partita_iva_valida("06363391001"));
    }

    #[test]
    fn cf() {
        assert!(codice_fiscale_valido("RSSMRA85T10A562S"));
        assert!(!codice_fiscale_valido("RSSMRA85T10A562T"));
        assert!(codice_fiscale_valido("06363391001"));
        assert!(!codice_fiscale_valido("XYZ"));
    }

    #[test]
    fn iban() {
        assert!(iban_valido("IT60 X054 2811 1010 0000 0123 456"));
        assert!(!iban_valido("IT60X0542811101000000123457"));
        assert!(iban_valido("DE89370400440532013000"));
    }

    #[test]
    fn destinatario() {
        assert!(codice_destinatario_valido("0000000", false));
        assert!(codice_destinatario_valido("UFY9MH", true));
        assert!(!codice_destinatario_valido("UFY9MH", false));
    }
}
