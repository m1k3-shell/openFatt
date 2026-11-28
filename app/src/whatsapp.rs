//! Invio di un documento con WhatsApp: si apre la chat del cliente con il collegamento
//! pubblico "wa.me" (WhatsApp Desktop o il sito, scelto da Windows) e il messaggio già
//! scritto; il PDF si salva in una cartella aperta accanto, da trascinare nella chat.
//! Nessuna API e nessun account aziendale: l'invio lo fa l'utente.

use openfatt_core::model::{Document, Party};

/// Numero nel formato internazionale senza "+" né spazi (es. 393331234567). I numeri
/// italiani senza prefisso prendono il 39; quelli che iniziano con 00 o + si lasciano.
pub fn normalize_phone(raw: &str) -> Option<String> {
    let t = raw.trim();
    let intl = t.starts_with('+') || t.starts_with("00");
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = if t.starts_with("00") { digits.trim_start_matches("00").to_string() } else { digits };
    if digits.len() < 6 {
        return None;
    }
    let n = if intl || (digits.starts_with("39") && digits.len() >= 11) { digits } else { format!("39{}", digits) };
    (n.len() <= 15).then_some(n)
}

/// Codifica per l'indirizzo (RFC 3986: lettere, cifre e -._~ restano, il resto in %XX).
pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Numero del cliente da usare: cellulare, altrimenti telefono.
pub fn phone_of(p: &Party) -> Option<String> {
    normalize_phone(&p.mobile).or_else(|| normalize_phone(&p.phone))
}

/// Collegamento alla chat con il messaggio.
pub fn link(phone: &str, text: &str) -> String {
    format!("https://wa.me/{}?text={}", phone, url_encode(text))
}

/// Messaggio proposto per un documento.
pub fn message(company: &str, doc: &Document, total: &str) -> String {
    let who = doc.party.display_name();
    format!(
        "Buongiorno{}, le inviamo {} n. {} del {}{}. In allegato il PDF.\n{}",
        if who.is_empty() { String::new() } else { format!(" {}", who) },
        doc.kind.label().to_lowercase(),
        doc.number_label(),
        doc.date.format("%d/%m/%Y"),
        if total.is_empty() { String::new() } else { format!(", importo € {}", total) },
        company
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeri_e_collegamento() {
        assert_eq!(normalize_phone("333 123 4567").as_deref(), Some("393331234567"));
        assert_eq!(normalize_phone("+39 333-1234567").as_deref(), Some("393331234567"));
        assert_eq!(normalize_phone("0039 3331234567").as_deref(), Some("393331234567"));
        assert_eq!(normalize_phone("+41 79 123 45 67").as_deref(), Some("41791234567"));
        assert_eq!(normalize_phone("0461 123456").as_deref(), Some("390461123456"));
        assert_eq!(normalize_phone("12"), None);
        assert_eq!(normalize_phone(""), None);
        let l = link("393331234567", "Fattura n. 5/A del 28/11 – € 1.220,00 & grazie");
        assert!(l.starts_with("https://wa.me/393331234567?text=Fattura%20n.%205%2FA"));
        assert!(l.contains("%E2%80%93") && l.contains("%26") && !l.contains(' '));
    }
}
