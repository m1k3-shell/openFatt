//! File di interscambio in formato INI (versione 2) per gli adattatori dei registratori.
//! Struttura pubblica: sezione [General], sezioni [PagamN] e [RigaN]; decimali con il punto;
//! i ritorni a capo nei messaggi si scrivono #0D#0A.

use std::process::Command;

use crate::{amount, printable, PayKind, Receipt, Result, RtError, RtOutcome};

fn pay_tender(k: PayKind) -> u32 {
    match k {
        PayKind::Cash => 1,
        PayKind::Electronic => 2,
        PayKind::Ticket => 3,
        PayKind::NotCollected => 4,
    }
}

/// Produce il file di interscambio.
pub fn to_ini(r: &Receipt, port: &str, ip: &str) -> String {
    let mut o = String::new();
    o.push_str("[General]\r\n");
    o.push_str("Versione=2\r\n");
    o.push_str("Note=File creato da openFatt\r\n");
    let msg: Vec<String> = r.courtesy_message.lines().map(|l| printable(l, 46)).filter(|l| !l.is_empty()).collect();
    o.push_str(&format!("MsgCortesia={}\r\n", msg.join("#0D#0A")));
    o.push_str(&format!("Porta={}\r\n", if port.is_empty() { "Ethernet" } else { port }));
    o.push_str(&format!("IndirizzoIP={}\r\n", ip));
    o.push_str(&format!("CodFiscalePartitaIva={}\r\n", r.customer_tax_code.trim().to_uppercase()));
    o.push_str(&format!("TotResto={}\r\n", amount(r.change, '.')));
    o.push_str(&format!("TotScontrino={}\r\n", amount(r.total, '.')));
    o.push_str(&format!("NumRighe={}\r\n", r.lines.len()));
    o.push_str(&format!("NumPagam={}\r\n", r.payments.len()));
    for (i, p) in r.payments.iter().enumerate() {
        o.push_str(&format!("[Pagam{}]\r\n", i + 1));
        o.push_str(&format!("Desc={}\r\n", printable(&p.description, 30)));
        o.push_str(&format!("Importo={}\r\n", amount(p.amount, '.')));
        o.push_str(&format!("Tender={}\r\n", pay_tender(p.kind)));
    }
    for (i, l) in r.lines.iter().enumerate() {
        o.push_str(&format!("[Riga{}]\r\n", i + 1));
        o.push_str(&format!("Desc={}\r\n", printable(&l.description, 40)));
        o.push_str(&format!("PercIva={}\r\n", l.vat_rate.normalize()));
        o.push_str(&format!("Reparto={}\r\n", l.department));
        o.push_str(&format!("Reso={}\r\n", if l.refund { "True" } else { "False" }));
        o.push_str(&format!("Qta={}\r\n", l.qty.normalize()));
        o.push_str(&format!("PrezzoUnitario={}\r\n", amount(l.unit_price, '.')));
        if !l.discount.is_zero() {
            o.push_str(&format!("DescSconto={}\r\n", printable(&l.discount_label, 30)));
            o.push_str(&format!("ImportoSconto={}\r\n", amount(l.discount, '.')));
        }
    }
    o
}

/// Scrive il file e lancia l'adattatore passandogli il percorso. Codice di uscita 0 = emesso.
pub fn run_adapter(exe: &str, port: &str, ip: &str, r: &Receipt) -> Result<RtOutcome> {
    let exe_path = std::path::Path::new(exe);
    if !exe_path.exists() {
        return Err(RtError(format!("Adattatore non trovato: {}", exe)));
    }
    let dir = exe_path.parent().unwrap_or(std::path::Path::new("."));
    let file = dir.join("SCONTR.TXT");
    // il file è in codifica Windows-1252 come i file INI di Windows
    let content: Vec<u8> = to_ini(r, port, ip).chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }).collect();
    std::fs::write(&file, content).map_err(|e| RtError(format!("Scrittura di {}: {}", file.display(), e)))?;
    let status = Command::new(exe_path).arg(&file).current_dir(dir).status().map_err(|e| RtError(format!("Avvio dell'adattatore: {}", e)))?;
    if status.success() {
        Ok(RtOutcome { message: "Documento inviato al registratore tramite adattatore".into(), ..Default::default() })
    } else {
        Err(RtError(format!("L'adattatore ha restituito il codice {}", status.code().unwrap_or(-1))))
    }
}
