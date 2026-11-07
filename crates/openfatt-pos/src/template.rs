//! Driver "a modelli": i comandi per il registratore si scrivono come testo con segnaposto
//! e si inviano scrivendo un file (per i programmi di collegamento che leggono una cartella,
//! come quelli forniti dai rivenditori Ditron, RCH, Micrelec e altri) oppure con una
//! richiesta HTTP POST (registratori con server web interno).
//!
//! Il gestionale non conosce i protocolli proprietari: i modelli vanno compilati secondo il
//! manuale tecnico del registratore o del suo programma di collegamento.
//!
//! Segnaposto delle righe: {desc} {qty} {qty_milli} {price} {price_cents} {total}
//! {total_cents} {dept} {vat} {disc} {disc_cents} {refund} {n};
//! dei pagamenti: {tender} {pay_desc} {amount} {amount_cents} {n};
//! generali: {total} {total_cents} {change} {cf} {lottery} {message} {operator}.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{amount, printable, PayKind, Receipt, Result, RtError, RtOutcome};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Transport {
    /// Scrive il file dei comandi; se `response` non è vuoto attende quel file di risposta.
    File { path: String, response: String, timeout_ms: u32 },
    /// Invia i comandi come corpo di una richiesta POST all'indirizzo indicato.
    Http { host: String, port: u16, path: String, content_type: String, timeout_ms: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Templates {
    pub begin: String,
    pub line: String,
    pub discount: String,
    pub payment: String,
    pub end: String,
    pub closure: String,
    /// Separatore decimale degli importi ("," o ".")
    pub decimal_sep: String,
    /// Separatore tra i comandi
    pub newline: String,
    /// Testo che nella risposta indica un errore (vuoto = nessun controllo)
    pub error_marker: String,
    /// Codici di pagamento: contanti, elettronico, buoni, non riscosso
    pub tenders: [String; 4],
    pub max_desc: usize,
}

impl Default for Templates {
    fn default() -> Self {
        Templates {
            begin: String::new(),
            line: "VENDITA REP={dept} DES='{desc}' QTA={qty} PREZZO={price}".into(),
            discount: "SCONTO DES='{desc}' IMPORTO={disc}".into(),
            payment: "PAGAMENTO TIPO={tender} IMPORTO={amount}".into(),
            end: "CHIUSURA".into(),
            closure: "CHIUSURA_GIORNALIERA".into(),
            decimal_sep: ",".into(),
            newline: "\r\n".into(),
            error_marker: "ERR".into(),
            tenders: ["1".into(), "2".into(), "3".into(), "4".into()],
            max_desc: 32,
        }
    }
}

fn cents(d: Decimal) -> String {
    (openfatt_core::money::round2(d) * Decimal::ONE_HUNDRED).trunc().abs().to_string()
}

fn tender(t: &Templates, k: PayKind) -> String {
    let i = match k {
        PayKind::Cash => 0,
        PayKind::Electronic => 1,
        PayKind::Ticket => 2,
        PayKind::NotCollected => 3,
    };
    t.tenders[i].clone()
}

fn fill(tpl: &str, pairs: &[(&str, String)]) -> String {
    let mut s = tpl.to_string();
    for (k, v) in pairs {
        s = s.replace(&format!("{{{}}}", k), v);
    }
    s
}

/// Testo dei comandi per un documento commerciale.
pub fn render(t: &Templates, r: &Receipt) -> String {
    let sep = t.decimal_sep.chars().next().unwrap_or(',');
    let nl = if t.newline.is_empty() { "\r\n" } else { t.newline.as_str() };
    let general = |extra: &[(&str, String)]| -> Vec<(&'static str, String)> {
        let mut v: Vec<(&'static str, String)> = vec![
            ("total", amount(r.total, sep)),
            ("total_cents", cents(r.total)),
            ("change", amount(r.change, sep)),
            ("cf", r.customer_tax_code.trim().to_uppercase()),
            ("lottery", r.lottery_code.trim().to_uppercase()),
            ("message", printable(&r.courtesy_message.replace('\n', " "), 46)),
            ("operator", r.operator.to_string()),
        ];
        for (k, val) in extra {
            if let Some(x) = v.iter_mut().find(|(a, _)| a == k) {
                x.1 = val.clone();
            }
        }
        v
    };
    let mut out: Vec<String> = Vec::new();
    if !t.begin.trim().is_empty() {
        out.push(fill(&t.begin, &general(&[])));
    }
    for (i, l) in r.lines.iter().enumerate() {
        let desc = printable(&l.description, t.max_desc.max(1)).replace('\'', " ");
        let line_total = openfatt_core::money::round2(l.unit_price * l.qty);
        let mut pairs = general(&[]);
        pairs.extend([
            ("desc", desc.clone()),
            ("qty", openfatt_core::money::fmt_xml_var(l.qty, 0, 3).replace('.', &sep.to_string())),
            ("qty_milli", (l.qty * Decimal::from(1000)).trunc().to_string()),
            ("price", amount(l.unit_price, sep)),
            ("price_cents", cents(l.unit_price)),
            ("dept", l.department.to_string()),
            ("vat", l.vat_rate.normalize().to_string()),
            ("disc", amount(l.discount, sep)),
            ("disc_cents", cents(l.discount)),
            ("refund", if l.refund { "1".into() } else { "0".into() }),
            ("n", (i + 1).to_string()),
        ]);
        if let Some(x) = pairs.iter_mut().find(|(k, _)| *k == "total") {
            x.1 = amount(line_total, sep);
        }
        if let Some(x) = pairs.iter_mut().find(|(k, _)| *k == "total_cents") {
            x.1 = cents(line_total);
        }
        out.push(fill(&t.line, &pairs));
        if !l.discount.is_zero() && !t.discount.trim().is_empty() {
            let mut p = pairs.clone();
            if let Some(x) = p.iter_mut().find(|(k, _)| *k == "desc") {
                x.1 = printable(if l.discount_label.is_empty() { "Sconto" } else { &l.discount_label }, t.max_desc.max(1));
            }
            out.push(fill(&t.discount, &p));
        }
    }
    for (i, p) in r.payments.iter().enumerate() {
        let mut pairs = general(&[]);
        pairs.extend([("tender", tender(t, p.kind)), ("pay_desc", printable(&p.description, 30)), ("amount", amount(p.amount, sep)), ("amount_cents", cents(p.amount)), ("n", (i + 1).to_string())]);
        out.push(fill(&t.payment, &pairs));
    }
    if !t.end.trim().is_empty() {
        out.push(fill(&t.end, &general(&[])));
    }
    let mut s = out.join(nl);
    s.push_str(nl);
    s
}

fn send(transport: &Transport, body: &str, error_marker: &str) -> Result<String> {
    let resp = match transport {
        Transport::File { path, response, timeout_ms } => {
            let p = std::path::Path::new(path);
            if let Some(dir) = p.parent() {
                if !dir.as_os_str().is_empty() && !dir.exists() {
                    return Err(RtError(format!("Cartella del collegamento non trovata: {}", dir.display())));
                }
            }
            if !response.is_empty() {
                let _ = std::fs::remove_file(response);
            }
            let bytes: Vec<u8> = body.chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }).collect();
            std::fs::write(p, bytes).map_err(|e| RtError(format!("Scrittura di {}: {}", p.display(), e)))?;
            if response.is_empty() {
                return Ok("Comandi scritti per il programma di collegamento".into());
            }
            let start = Instant::now();
            loop {
                if let Ok(r) = std::fs::read(response) {
                    break r.iter().map(|&b| b as char).collect::<String>();
                }
                if start.elapsed() > Duration::from_millis(*timeout_ms as u64) {
                    return Err(RtError(format!("Nessuna risposta dal registratore entro {} secondi ({})", timeout_ms / 1000, response)));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        Transport::Http { host, port, path, content_type, timeout_ms } => {
            let addr = (host.as_str(), *port).to_socket_addrs().map_err(|e| RtError(format!("Indirizzo {}: {}", host, e)))?.next().ok_or_else(|| RtError("Indirizzo non valido".into()))?;
            let to = Duration::from_millis(*timeout_ms as u64);
            let mut s = TcpStream::connect_timeout(&addr, to).map_err(|e| RtError(format!("Collegamento a {}:{}: {}", host, port, e)))?;
            s.set_read_timeout(Some(to)).ok();
            let ct = if content_type.is_empty() { "text/plain" } else { content_type.as_str() };
            let req = format!("POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", if path.is_empty() { "/" } else { path }, host, ct, body.len(), body);
            s.write_all(req.as_bytes()).map_err(|e| RtError(e.to_string()))?;
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            let text = String::from_utf8_lossy(&buf).to_string();
            if !text.starts_with("HTTP/1.1 2") && !text.starts_with("HTTP/1.0 2") {
                return Err(RtError(format!("Risposta del registratore: {}", text.lines().next().unwrap_or("vuota"))));
            }
            text.split("\r\n\r\n").nth(1).unwrap_or("").to_string()
        }
    };
    if !error_marker.is_empty() && resp.contains(error_marker) {
        return Err(RtError(format!("Il registratore ha segnalato un errore: {}", resp.trim())));
    }
    Ok(resp)
}

pub fn emit(transport: &Transport, t: &Templates, r: &Receipt) -> Result<RtOutcome> {
    let resp = send(transport, &render(t, r), &t.error_marker)?;
    Ok(RtOutcome { message: format!("Inviato al registratore. {}", resp.trim()), ..Default::default() })
}

pub fn closure(transport: &Transport, t: &Templates) -> Result<String> {
    if t.closure.trim().is_empty() {
        return Err(RtError("Modello della chiusura giornaliera non indicato".into()));
    }
    let nl = if t.newline.is_empty() { "\r\n" } else { t.newline.as_str() };
    send(transport, &format!("{}{}", t.closure, nl), &t.error_marker).map(|r| format!("Chiusura giornaliera inviata. {}", r.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReceiptLine, ReceiptPayment};
    use std::str::FromStr;

    #[test]
    fn renders_and_writes_commands() {
        let r = Receipt {
            lines: vec![ReceiptLine { description: "Caffe' macinato".into(), qty: Decimal::from(2), unit_price: Decimal::from_str("3.50").unwrap(), discount: Decimal::from_str("0.50").unwrap(), discount_label: "Promo".into(), vat_rate: Decimal::from(10), natura: None, department: 2, refund: false }],
            payments: vec![ReceiptPayment { kind: PayKind::Cash, description: "Contanti".into(), amount: Decimal::from_str("10").unwrap() }],
            total: Decimal::from_str("6.50").unwrap(),
            change: Decimal::from_str("3.50").unwrap(),
            customer_tax_code: String::new(),
            lottery_code: String::new(),
            courtesy_message: "Grazie".into(),
            refund_of: None,
            operator: 1,
        };
        let t = Templates { line: "R{dept}/{price_cents}/{qty}/({desc})".into(), discount: "S/{disc_cents}".into(), payment: "T{tender}/{amount_cents}".into(), begin: "INIZIO".into(), ..Default::default() };
        let s = render(&t, &r);
        assert_eq!(s, "INIZIO\r\nR2/350/2/(Caffe  macinato)\r\nS/50\r\nT1/1000\r\nCHIUSURA\r\n");
        let dir = std::env::temp_dir().join(format!("ga-tpl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scontrino.txt");
        let tr = Transport::File { path: path.display().to_string(), response: String::new(), timeout_ms: 1000 };
        emit(&tr, &t, &r).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("R2/350"));
        // con file di risposta atteso e non prodotto: errore entro il tempo
        let tr2 = Transport::File { path: path.display().to_string(), response: dir.join("risposta.txt").display().to_string(), timeout_ms: 400 };
        assert!(emit(&tr2, &t, &r).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
