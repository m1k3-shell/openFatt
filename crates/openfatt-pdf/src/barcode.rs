//! Codici a barre per le etichette: EAN-13, EAN-8 (GS1) e Code 128 (ISO/IEC 15417),
//! disegnati come rettangoli vettoriali nel PDF.

use crate::writer::{Align, Pdf};

#[derive(Debug, Clone, PartialEq)]
pub struct Barcode {
    /// Moduli: true = barra, false = spazio.
    pub modules: Vec<bool>,
    /// Testo leggibile sotto le barre.
    pub text: String,
    /// Moduli di zona di rispetto da lasciare a sinistra e a destra.
    pub quiet: usize,
    /// EAN: barre di guardia più lunghe (posizioni dei moduli).
    pub guards: Vec<usize>,
}

const L: [&str; 10] = ["0001101", "0011001", "0010011", "0111101", "0100011", "0110001", "0101111", "0111011", "0110111", "0001011"];
const G: [&str; 10] = ["0100111", "0110011", "0011011", "0100001", "0011101", "0111001", "0000101", "0010001", "0001001", "0010111"];
const R: [&str; 10] = ["1110010", "1100110", "1101100", "1000010", "1011100", "1001110", "1010000", "1000100", "1001000", "1110100"];
const PARITY: [&str; 10] = ["LLLLLL", "LLGLGG", "LLGGLG", "LLGGGL", "LGLLGG", "LGGLLG", "LGGGLL", "LGLGLG", "LGLGGL", "LGGLGL"];

/// Cifra di controllo GS1 (modulo 10, pesi 3 e 1 da destra).
pub fn gs1_check_digit(digits: &str) -> Option<u32> {
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let sum: u32 = digits.chars().rev().enumerate().map(|(i, c)| c.to_digit(10).unwrap() * if i % 2 == 0 { 3 } else { 1 }).sum();
    Some((10 - sum % 10) % 10)
}

/// EAN-13 o EAN-8 valido (con cifra di controllo corretta)?
pub fn ean_valid(code: &str) -> bool {
    let c = code.trim();
    (c.len() == 13 || c.len() == 8) && c.chars().all(|ch| ch.is_ascii_digit()) && gs1_check_digit(&c[..c.len() - 1]) == c[c.len() - 1..].parse().ok()
}

fn push(m: &mut Vec<bool>, pat: &str) {
    m.extend(pat.chars().map(|c| c == '1'));
}

pub fn ean13(code: &str) -> Option<Barcode> {
    let c = code.trim();
    if c.len() != 13 || !ean_valid(c) {
        return None;
    }
    let d: Vec<usize> = c.chars().map(|ch| ch.to_digit(10).unwrap() as usize).collect();
    let mut m = Vec::new();
    push(&mut m, "101");
    for (i, p) in PARITY[d[0]].chars().enumerate() {
        push(&mut m, if p == 'L' { L[d[i + 1]] } else { G[d[i + 1]] });
    }
    push(&mut m, "01010");
    for x in &d[7..13] {
        push(&mut m, R[*x]);
    }
    push(&mut m, "101");
    let guards = [0, 1, 2, 45, 46, 47, 48, 49, 92, 93, 94].to_vec();
    Some(Barcode { modules: m, text: format!("{} {} {}", &c[..1], &c[1..7], &c[7..]), quiet: 11, guards })
}

pub fn ean8(code: &str) -> Option<Barcode> {
    let c = code.trim();
    if c.len() != 8 || !ean_valid(c) {
        return None;
    }
    let d: Vec<usize> = c.chars().map(|ch| ch.to_digit(10).unwrap() as usize).collect();
    let mut m = Vec::new();
    push(&mut m, "101");
    for x in &d[..4] {
        push(&mut m, L[*x]);
    }
    push(&mut m, "01010");
    for x in &d[4..] {
        push(&mut m, R[*x]);
    }
    push(&mut m, "101");
    Some(Barcode { modules: m, text: format!("{} {}", &c[..4], &c[4..]), quiet: 7, guards: [0, 1, 2, 31, 32, 33, 34, 35, 64, 65, 66].to_vec() })
}

/// Larghezze di barre e spazi dei simboli Code 128 (valori 0-105).
const C128: [&str; 106] = [
    "212222", "222122", "222221", "121223", "121322", "131222", "122213", "122312", "132212", "221213", "221312", "231212", "112232", "122132", "122231", "113222", "123122", "123221", "223211", "221132", "221231", "213212", "223112", "312131", "311222", "321122", "321221", "312212", "322112", "322211", "212123", "212321", "232121", "111323", "131123", "131321", "112313", "132113", "132311", "211313", "231113", "231311", "112133", "112331", "132131", "113123", "113321", "133121", "313121", "211331", "231131", "213113", "213311", "213131", "311123", "311321", "331121", "312113", "312311", "332111", "314111", "221411", "431111", "111224", "111422", "121124", "121421", "141122", "141221", "112214", "112412", "122114", "122411", "142112", "142211", "241211", "221114", "413111", "241112", "134111", "111242", "121142", "121241", "114212", "124112", "124211", "411212", "421112", "421211", "212141", "214121", "412121", "111143", "111341", "131141", "114113", "114311", "411113", "411311", "113141", "114131", "311141", "411131", "211412", "211214", "211232",
];
const C128_STOP: &str = "2331112";

/// Valori dei simboli Code 128 (con avvio e controllo, senza stop): subset C per i numeri
/// di lunghezza pari, altrimenti subset B (caratteri ASCII stampabili).
pub fn code128_values(text: &str) -> Option<Vec<usize>> {
    if text.is_empty() {
        return None;
    }
    let mut v = Vec::new();
    if text.len() % 2 == 0 && text.chars().all(|c| c.is_ascii_digit()) {
        v.push(105);
        for i in (0..text.len()).step_by(2) {
            v.push(text[i..i + 2].parse::<usize>().unwrap());
        }
    } else {
        if !text.chars().all(|c| (' '..='~').contains(&c)) {
            return None;
        }
        v.push(104);
        v.extend(text.chars().map(|c| c as usize - 32));
    }
    let check = (v[0] + v.iter().enumerate().skip(1).map(|(i, x)| i * x).sum::<usize>()) % 103;
    v.push(check);
    Some(v)
}

pub fn code128(text: &str) -> Option<Barcode> {
    let values = code128_values(text)?;
    let mut m = Vec::new();
    let mut add = |widths: &str| {
        for (i, w) in widths.chars().enumerate() {
            for _ in 0..w.to_digit(10).unwrap() {
                m.push(i % 2 == 0);
            }
        }
    };
    for v in &values {
        add(C128[*v]);
    }
    add(C128_STOP);
    Some(Barcode { modules: m, text: text.to_string(), quiet: 10, guards: Vec::new() })
}

/// Il codice più adatto: EAN se il codice è un EAN valido, altrimenti Code 128.
pub fn auto(code: &str) -> Option<Barcode> {
    let c = code.trim();
    ean13(c).or_else(|| ean8(c)).or_else(|| code128(c))
}

/// Disegna il codice nel riquadro (x, y in alto a sinistra), con il testo sotto.
pub fn draw(pdf: &mut Pdf, b: &Barcode, x: f32, y: f32, w: f32, h: f32, font: f32) {
    let total = b.modules.len() + 2 * b.quiet;
    let mw = w / total as f32;
    let text_h = if font > 0.0 { font + 1.5 } else { 0.0 };
    let bar_h = (h - text_h).max(4.0);
    let mut i = 0;
    while i < b.modules.len() {
        if b.modules[i] {
            let start = i;
            while i < b.modules.len() && b.modules[i] {
                i += 1;
            }
            let hh = bar_h;
            pdf.rect(x + (b.quiet + start) as f32 * mw, y, (i - start) as f32 * mw, hh, Some(0.0), false);
        } else {
            i += 1;
        }
    }
    if font > 0.0 {
        pdf.text(x + w / 2.0, y + bar_h + font, font, false, &b.text, Align::Center);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ean() {
        assert_eq!(gs1_check_digit("400638133393"), Some(1));
        assert!(ean_valid("4006381333931"));
        assert!(!ean_valid("4006381333932"));
        assert!(ean_valid("96385074"));
        let b = ean13("4006381333931").unwrap();
        assert_eq!(b.modules.len(), 95);
        // inizio, centro e fine con le barre di guardia
        assert_eq!(&b.modules[..3], &[true, false, true]);
        assert_eq!(&b.modules[45..50], &[false, true, false, true, false]);
        // prima cifra 4 = parità LGLLGG: la seconda cifra (0) in L
        let s: String = b.modules[3..10].iter().map(|x| if *x { '1' } else { '0' }).collect();
        assert_eq!(s, L[0]);
        assert_eq!(ean8("96385074").unwrap().modules.len(), 67);
        assert!(ean13("123").is_none());
        // codici alfanumerici della stessa lunghezza non sono EAN
        assert!(!ean_valid("SED01-XL") && ean8("SED01-XL").is_none());
        assert!(!ean_valid("ABCDEFGHIJKLM"));
        assert_eq!(auto("SED01-XL").unwrap().text, "SED01-XL");
    }

    #[test]
    fn code128_tabella_e_controllo() {
        assert!(C128.iter().all(|p| p.chars().map(|c| c.to_digit(10).unwrap()).sum::<u32>() == 11));
        assert_eq!(C128_STOP.chars().map(|c| c.to_digit(10).unwrap()).sum::<u32>(), 13);
        // esempio di calcolo: "PJJ123C" in subset B
        let v = code128_values("PJJ123C").unwrap();
        assert_eq!(v[0], 104);
        let check = (104 + 48 + 2 * 42 + 3 * 42 + 4 * 17 + 5 * 18 + 6 * 19 + 7 * 35) % 103;
        assert_eq!(*v.last().unwrap(), check);
        // numerico pari: subset C, due cifre per simbolo
        assert_eq!(code128_values("123456").unwrap(), vec![105, 12, 34, 56, (105 + 12 + 2 * 34 + 3 * 56) % 103]);
        let b = code128("SED01").unwrap();
        assert_eq!(b.modules.len(), 11 * 7 + 13);
        assert!(code128("caffè").is_none());
        assert_eq!(auto("4006381333931").unwrap().modules.len(), 95);
    }
}
