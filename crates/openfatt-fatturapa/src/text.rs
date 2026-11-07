//! Normalizzazione dei testi secondo i tipi dello schema:
//! - "Latin": caratteri Basic Latin + Latin-1 Supplement (U+0020..U+00FF);
//! - "BasicLatin": solo ASCII stampabile.
//! Il simbolo dell'euro non appartiene al Latin-1 e viene sostituito da "EUR".

fn map_special(c: char) -> Option<&'static str> {
    Some(match c {
        '€' => "EUR",
        '‘' | '’' | '‚' | '′' | '`' => "'",
        '“' | '”' | '„' | '″' => "\"",
        '–' | '—' | '‐' | '−' => "-",
        '…' => "...",
        '•' | '·' => "-",
        '™' => "TM",
        '\u{00A0}' => " ",
        'Œ' => "OE",
        'œ' => "oe",
        'Š' => "S",
        'š' => "s",
        'Ž' => "Z",
        'ž' => "z",
        'Ÿ' => "Y",
        _ => return None,
    })
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        let c = if c == '\r' || c == '\n' || c == '\t' { ' ' } else { c };
        if c == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Testo per i campi "Latin" (Latin-1). I caratteri non rappresentabili sono sostituiti.
pub fn latin(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if let Some(rep) = map_special(c) {
            out.push_str(rep);
        } else if (c as u32) >= 0x20 && (c as u32) <= 0x7E {
            out.push(c);
        } else if (c as u32) >= 0xA0 && (c as u32) <= 0xFF {
            out.push(c);
        } else if c == '\n' || c == '\r' || c == '\t' {
            out.push(' ');
        } else {
            out.push('?');
        }
    }
    collapse_ws(&out)
}

/// Testo per i campi "BasicLatin" (solo ASCII). Le lettere accentate diventano
/// lettera + apostrofo (à -> a'), come nell'uso comune.
pub fn basic_latin(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in latin(s).chars() {
        if c.is_ascii() {
            out.push(c);
            continue;
        }
        let rep = match c {
            'à' | 'á' => "a'",
            'è' | 'é' => "e'",
            'ì' | 'í' => "i'",
            'ò' | 'ó' => "o'",
            'ù' | 'ú' => "u'",
            'À' | 'Á' => "A'",
            'È' | 'É' => "E'",
            'Ì' | 'Í' => "I'",
            'Ò' | 'Ó' => "O'",
            'Ù' | 'Ú' => "U'",
            'â' | 'ä' | 'ã' | 'å' => "a",
            'ê' | 'ë' => "e",
            'î' | 'ï' => "i",
            'ô' | 'ö' | 'õ' | 'ø' => "o",
            'û' | 'ü' => "u",
            'Â' | 'Ä' | 'Ã' | 'Å' => "A",
            'Ê' | 'Ë' => "E",
            'Î' | 'Ï' => "I",
            'Ô' | 'Ö' | 'Õ' | 'Ø' => "O",
            'Û' | 'Ü' => "U",
            'ç' => "c",
            'Ç' => "C",
            'ñ' => "n",
            'Ñ' => "N",
            'ß' => "ss",
            'ý' | 'ÿ' => "y",
            'Ý' => "Y",
            'æ' => "ae",
            'Æ' => "AE",
            '°' => "o",
            '«' | '»' => "\"",
            _ => "",
        };
        out.push_str(rep);
    }
    out
}

/// Tronca a `max` caratteri (non byte).
pub fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Divide un testo lungo in blocchi di al massimo `max` caratteri (es. Causale da 200).
pub fn chunks(s: &str, max: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    chars.chunks(max).map(|c| c.iter().collect::<String>().trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Valore opzionale normalizzato Latin e troncato; None se vuoto.
pub fn opt_latin(s: &str, max: usize) -> Option<String> {
    let t = truncate(&latin(s), max);
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

pub fn opt_basic(s: &str, max: usize) -> Option<String> {
    let t = truncate(&basic_latin(s), max);
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalize() {
        assert_eq!(latin("Prezzo 10 € – “nuovo”"), "Prezzo 10 EUR - \"nuovo\"");
        assert_eq!(latin("riga1\nriga2"), "riga1 riga2");
        assert_eq!(basic_latin("Città più"), "Citta' piu'");
        assert_eq!(chunks(&"a".repeat(450), 200).len(), 3);
    }
}
