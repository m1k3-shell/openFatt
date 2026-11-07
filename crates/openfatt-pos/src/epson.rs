//! Registratori Epson con interfaccia XML su HTTP ("Fiscal ePOS-Print", cgi-bin/fpmate.cgi).
//!
//! Il documento commerciale si apre con beginFiscalReceipt, contiene una printRecItem per
//! riga (importi IVA inclusa, separatore decimale virgola), i pagamenti con printRecTotal e si
//! chiude con endFiscalReceipt. Gli sconti sono applicati al prezzo della riga, così il totale
//! emesso coincide sempre con quello del gestionale. Da collaudare sul modello in uso.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::{amount, printable, PayKind, Receipt, Result, RtError, RtOutcome};

fn esc(s: &str) -> String {
    openfatt_fatturapa::write::escape(s)
}

fn envelope(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">\n<s:Body>\n{}</s:Body>\n</s:Envelope>\n",
        body
    )
}

/// (paymentType, index) secondo la tabella pagamenti dei registratori telematici Epson.
fn payment_codes(k: PayKind) -> (u32, u32) {
    match k {
        PayKind::Cash => (0, 0),
        PayKind::Electronic => (2, 1),
        PayKind::Ticket => (3, 1),
        PayKind::NotCollected => (5, 0),
    }
}

pub fn receipt_xml(r: &Receipt) -> String {
    let op = r.operator.max(1);
    let mut b = String::from("<printerFiscalReceipt>\n");
    if let Some((serial, z, n, date)) = &r.refund_of {
        // documento di reso: riferimento al documento commerciale originale
        b.push_str(&format!(
            "<printRecMessage operator=\"{}\" messageType=\"4\" message=\"{}\" />\n",
            op,
            esc(&format!("RESO {}-{} del {} {}", z, n, date, serial))
        ));
    }
    b.push_str(&format!("<beginFiscalReceipt operator=\"{}\" />\n", op));
    for l in &r.lines {
        let qty = l.qty.abs();
        // prezzo unitario già scontato (il registratore calcola totale = quantità x prezzo)
        let unit = if qty.is_zero() { l.unit_price } else { openfatt_core::money::round2((l.unit_price * qty - l.discount) / qty) };
        let mut desc = printable(&l.description, 38);
        if !l.discount.is_zero() && !l.discount_label.is_empty() {
            desc = printable(&format!("{} {}", desc, l.discount_label), 38);
        }
        let tag = if l.refund { "printRecRefund" } else { "printRecItem" };
        b.push_str(&format!(
            "<{} operator=\"{}\" description=\"{}\" quantity=\"{}\" unitPrice=\"{}\" department=\"{}\" justification=\"1\" />\n",
            tag,
            op,
            esc(&desc),
            openfatt_core::money::fmt_xml_var(qty, 0, 3).replace('.', ","),
            amount(unit, ','),
            l.department
        ));
    }
    if !r.customer_tax_code.trim().is_empty() {
        b.push_str(&format!(
            "<printRecMessage operator=\"{}\" messageType=\"3\" message=\"{}\" />\n",
            op,
            esc(&format!("C.F./P.IVA {}", r.customer_tax_code.trim().to_uppercase()))
        ));
    }
    if !r.lottery_code.trim().is_empty() {
        b.push_str(&format!("<setLotteryCode code=\"{}\" />\n", esc(r.lottery_code.trim())));
    }
    for p in &r.payments {
        let (t, idx) = payment_codes(p.kind);
        b.push_str(&format!(
            "<printRecTotal operator=\"{}\" description=\"{}\" payment=\"{}\" paymentType=\"{}\" index=\"{}\" justification=\"2\" />\n",
            op,
            esc(&printable(&p.description, 30)),
            amount(p.amount, ','),
            t,
            idx
        ));
    }
    if !r.courtesy_message.trim().is_empty() {
        b.push_str(&format!("<printRecMessage operator=\"{}\" messageType=\"3\" message=\"{}\" />\n", op, esc(&printable(&r.courtesy_message, 46))));
    }
    b.push_str(&format!("<endFiscalReceipt operator=\"{}\" />\n</printerFiscalReceipt>\n", op));
    envelope(&b)
}

pub fn z_report_xml(operator: u32) -> String {
    envelope(&format!("<printerFiscalReport>\n<printZReport operator=\"{}\" />\n</printerFiscalReport>\n", operator.max(1)))
}

/// Invio HTTP/1.1 minimo (POST), senza dipendenze esterne.
pub fn post(host: &str, port: u16, timeout_ms: u32, body: &str) -> Result<String> {
    let host = host.trim();
    if host.is_empty() {
        return Err(RtError("Indirizzo IP del registratore non impostato".into()));
    }
    let timeout = Duration::from_millis(timeout_ms.max(2000) as u64);
    let addr = (host, if port == 0 { 80 } else { port })
        .to_socket_addrs()
        .map_err(|e| RtError(format!("Indirizzo {}: {}", host, e)))?
        .next()
        .ok_or_else(|| RtError(format!("Indirizzo {} non valido", host)))?;
    let mut s = TcpStream::connect_timeout(&addr, timeout).map_err(|e| RtError(format!("Registratore non raggiungibile ({}): {}", host, e)))?;
    s.set_read_timeout(Some(timeout + Duration::from_secs(5))).ok();
    s.set_write_timeout(Some(timeout)).ok();
    let path = format!("/cgi-bin/fpmate.cgi?devid=local_printer&timeout={}", timeout_ms.max(2000));
    let req = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: text/xml; charset=utf-8\r\nSOAPAction: \"\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        path,
        host,
        body.len(),
        body
    );
    s.write_all(req.as_bytes()).map_err(|e| RtError(format!("Invio al registratore: {}", e)))?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| RtError(format!("Risposta del registratore: {}", e)))?;
    let text = String::from_utf8_lossy(&buf).to_string();
    let status_ok = text.starts_with("HTTP/1.1 200") || text.starts_with("HTTP/1.0 200");
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    if !status_ok {
        return Err(RtError(format!("Il registratore ha risposto: {}", text.lines().next().unwrap_or(""))));
    }
    Ok(body)
}

/// Interpreta la risposta: <response success="true" code="..." status="..."><addInfo>...</addInfo></response>
pub fn parse_response(xml: &str) -> Result<RtOutcome> {
    let start = xml.find('<').ok_or_else(|| RtError("Risposta vuota dal registratore".into()))?;
    let doc = roxmltree::Document::parse(&xml[start..]).map_err(|e| RtError(format!("Risposta non leggibile: {}", e)))?;
    let resp = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "response")
        .ok_or_else(|| RtError("Risposta senza elemento 'response'".into()))?;
    let get = |name: &str| -> String {
        resp.descendants().find(|n| n.is_element() && n.tag_name().name() == name).and_then(|n| n.text()).unwrap_or("").trim().to_string()
    };
    let success = resp.attribute("success").map(|s| s.eq_ignore_ascii_case("true")).unwrap_or(false);
    let code = resp.attribute("code").unwrap_or("").to_string();
    if !success {
        let hint = match code.as_str() {
            "EPTR_REC_EMPTY" => "carta esaurita",
            "EPTR_COVER_OPEN" => "coperchio aperto",
            "FP_NO_ANSWER" | "FP_NO_ANSWER_NETWORK" => "il registratore non risponde",
            "LAN_TIME_OUT" => "tempo scaduto",
            _ => "",
        };
        return Err(RtError(format!("Il registratore ha rifiutato l'operazione: {} {}", code, hint).trim().to_string()));
    }
    let receipt = get("fiscalReceiptNumber");
    let z = get("zRepNumber");
    let serial = get("serialNumber");
    let amount = get("fiscalReceiptAmount");
    Ok(RtOutcome {
        receipt_number: receipt.clone(),
        z_number: z.clone(),
        serial,
        simulated: false,
        message: if receipt.is_empty() { "Operazione eseguita".into() } else { format!("Documento {}-{} importo {}", z, receipt, amount) },
    })
}
