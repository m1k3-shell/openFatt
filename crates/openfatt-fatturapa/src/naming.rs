//! Nomi dei file secondo le specifiche SdI:
//! `<IdPaese><IdCodice>_<progressivo univoco>.xml` per le fatture,
//! `<nome file fattura>_<tipo messaggio>_<progressivo>.xml` per le notifiche.

const ALPHABET: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Progressivo alfanumerico di 5 caratteri (base 36) da un contatore numerico.
/// Copre 36^5 = 60.466.176 file per identificativo trasmittente.
pub fn progressive(counter: u64) -> String {
    let mut n = counter % 60_466_176;
    let mut out = [b'0'; 5];
    for i in (0..5).rev() {
        out[i] = ALPHABET[(n % 36) as usize];
        n /= 36;
    }
    String::from_utf8(out.to_vec()).unwrap()
}

/// Nome del file fattura: IT01234567890_0001A.xml
pub fn invoice_file_name(country: &str, code: &str, progressive: &str) -> String {
    format!("{}{}_{}.xml", country.to_ascii_uppercase(), code.to_ascii_uppercase(), progressive)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedName {
    pub country: String,
    pub code: String,
    pub progressive: String,
    /// Tipo messaggio (RC, NS, MC, NE, MT, EC, SE, DT, AT) per le notifiche.
    pub message_type: Option<String>,
    pub signed: bool,
    pub extension: String,
}

/// Analizza il nome di un file ricevuto dallo SdI.
pub fn parse_file_name(name: &str) -> Option<ParsedName> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let lower = base.to_ascii_lowercase();
    let signed = lower.ends_with(".p7m");
    let stem = if signed { &base[..base.len() - 4] } else { base };
    let (stem, ext) = match stem.rfind('.') {
        Some(i) => (&stem[..i], stem[i + 1..].to_ascii_lowercase()),
        None => (stem, String::new()),
    };
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() < 2 || parts[0].len() < 3 {
        return None;
    }
    let id = parts[0];
    let (country, code) = id.split_at(2);
    if !country.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let message_type = if parts.len() >= 4 {
        Some(parts[2].to_ascii_uppercase())
    } else if parts.len() == 3 && parts[2].len() == 2 && parts[2].chars().all(|c| c.is_ascii_alphabetic()) {
        Some(parts[2].to_ascii_uppercase())
    } else {
        None
    };
    Some(ParsedName {
        country: country.to_ascii_uppercase(),
        code: code.to_string(),
        progressive: parts[1].to_string(),
        message_type,
        signed,
        extension: ext,
    })
}

/// Vero se il nome corrisponde a una notifica SdI (RC, NS, MC, ...).
pub fn is_notification_name(name: &str) -> bool {
    parse_file_name(name)
        .and_then(|p| p.message_type)
        .map(|t| matches!(t.as_str(), "RC" | "NS" | "MC" | "NE" | "MT" | "EC" | "SE" | "DT" | "AT"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prog() {
        assert_eq!(progressive(1), "00001");
        assert_eq!(progressive(35), "0000Z");
        assert_eq!(progressive(36), "00010");
        assert_eq!(invoice_file_name("it", "01234567890", "00001"), "IT01234567890_00001.xml");
    }
    #[test]
    fn names() {
        let p = parse_file_name("IT01234567890_11111_RC_001.xml").unwrap();
        assert_eq!(p.message_type.as_deref(), Some("RC"));
        assert_eq!(p.progressive, "11111");
        let f = parse_file_name("IT01234567890_11111.xml.p7m").unwrap();
        assert!(f.signed);
        assert!(f.message_type.is_none());
        assert_eq!(f.extension, "xml");
        assert!(is_notification_name("IT01234567890_11111_MT_001.xml"));
        assert!(!is_notification_name("IT01234567890_11111.xml"));
    }
}
