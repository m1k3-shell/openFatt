//! Estratti conto per la riconciliazione: ISO 20022 camt.053 (BankToCustomerStatement, lo
//! standard pubblico usato anche nel circuito CBI per la rendicontazione dei movimenti) e
//! CSV esportati dall'home banking (data, descrizione, importo o dare/avere).
//!
//! Il camt si legge per nome degli elementi, così vanno bene tutte le versioni
//! (001.02 ... 001.08) e la camt.054 (avvisi di accredito e addebito), che ha le stesse
//! voci Ntry.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::str::FromStr;

/// Movimento dell'estratto conto. Importo positivo = accredito, negativo = addebito.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatementLine {
    pub date: NaiveDate,
    pub value_date: Option<NaiveDate>,
    pub amount: Decimal,
    pub currency: String,
    /// Ordinante (accrediti) o beneficiario (addebiti).
    pub counterparty: String,
    /// Causale (informazioni di rimessa).
    pub remittance: String,
    /// Riferimento della banca o end-to-end (CRO/TRN).
    pub reference: String,
}

impl StatementLine {
    /// Chiave stabile della riga, per non registrarla due volte.
    pub fn key(&self) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in format!("{}|{}|{}|{}|{}", self.date, self.amount.normalize(), self.reference.trim(), self.remittance.trim(), self.counterparty.trim()).bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{:016x}", h)
    }
}

/// Estratto conto letto.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Statement {
    pub iban: String,
    pub opening: Option<Decimal>,
    pub closing: Option<Decimal>,
    pub lines: Vec<StatementLine>,
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn path<'a, 'i>(n: roxmltree::Node<'a, 'i>, names: &[&str]) -> Option<roxmltree::Node<'a, 'i>> {
    let mut cur = n;
    for p in names {
        cur = child(cur, p)?;
    }
    Some(cur)
}

fn text(n: Option<roxmltree::Node>) -> String {
    n.and_then(|n| n.text()).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn date_of(n: Option<roxmltree::Node>) -> Option<NaiveDate> {
    let n = n?;
    let s = text(child(n, "Dt")).to_string();
    let s = if s.is_empty() { text(child(n, "DtTm")) } else { s };
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()
}

fn signed(amount: Decimal, ind: &str) -> Decimal {
    if ind == "DBIT" {
        -amount.abs()
    } else {
        amount.abs()
    }
}

/// Legge un camt.053 (o camt.054).
pub fn parse_camt(xml: &str) -> Result<Statement, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("File non valido: {}", e))?;
    let root = doc.root_element();
    let stmts: Vec<roxmltree::Node> = root.descendants().filter(|n| n.is_element() && matches!(n.tag_name().name(), "Stmt" | "Ntfctn" | "Rpt")).collect();
    if stmts.is_empty() {
        return Err("Non è un estratto conto ISO 20022 (camt.053/054)".into());
    }
    let mut out = Statement::default();
    for st in stmts {
        if out.iban.is_empty() {
            out.iban = text(path(st, &["Acct", "Id", "IBAN"]));
        }
        for bal in st.children().filter(|c| c.is_element() && c.tag_name().name() == "Bal") {
            let code = text(path(bal, &["Tp", "CdOrPrtry", "Cd"]));
            let amt = Decimal::from_str(&text(child(bal, "Amt"))).ok().map(|a| signed(a, &text(child(bal, "CdtDbtInd"))));
            match code.as_str() {
                "OPBD" | "PRCD" if out.opening.is_none() => out.opening = amt,
                "CLBD" => out.closing = amt,
                _ => {}
            }
        }
        for e in st.children().filter(|c| c.is_element() && c.tag_name().name() == "Ntry") {
            // le scritture annullate (RvslInd) restano: le storna la riga successiva
            let amt_node = child(e, "Amt");
            let Some(amount) = Decimal::from_str(&text(amt_node)).ok() else { continue };
            let currency = amt_node.and_then(|n| n.attribute("Ccy")).unwrap_or("EUR").to_string();
            let ind = text(child(e, "CdtDbtInd"));
            let date = date_of(child(e, "BookgDt")).or_else(|| date_of(child(e, "ValDt")));
            let Some(date) = date else { continue };
            let value_date = date_of(child(e, "ValDt"));
            // dettagli: primo TxDtls
            let tx = e.descendants().find(|n| n.is_element() && n.tag_name().name() == "TxDtls");
            let mut remittance: Vec<String> = Vec::new();
            let mut counterparty = String::new();
            let mut reference = text(child(e, "AcctSvcrRef"));
            if let Some(tx) = tx {
                for u in tx.descendants().filter(|n| n.is_element() && n.tag_name().name() == "Ustrd") {
                    remittance.push(text(Some(u)));
                }
                for r in tx.descendants().filter(|n| n.is_element() && n.tag_name().name() == "Ref" && n.parent().map(|p| p.tag_name().name() == "CdtrRefInf").unwrap_or(false)) {
                    remittance.push(text(Some(r)));
                }
                let party = if ind == "DBIT" { "Cdtr" } else { "Dbtr" };
                if let Some(rp) = child(tx, "RltdPties") {
                    counterparty = text(path(rp, &[party, "Nm"]));
                    if counterparty.is_empty() {
                        counterparty = text(path(rp, &[party, "Pty", "Nm"]));
                    }
                }
                let e2e = text(path(tx, &["Refs", "EndToEndId"]));
                if !e2e.is_empty() && e2e != "NOTPROVIDED" {
                    reference = e2e;
                }
            }
            if remittance.is_empty() {
                remittance.push(text(child(e, "AddtlNtryInf")));
            }
            out.lines.push(StatementLine {
                date,
                value_date,
                amount: signed(amount, &ind),
                currency,
                counterparty,
                remittance: remittance.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" "),
                reference,
            });
        }
    }
    Ok(out)
}

fn parse_it_amount(s: &str) -> Option<Decimal> {
    let s = s.trim().trim_start_matches('€').trim().replace(' ', "");
    if s.is_empty() {
        return None;
    }
    // 1.234,56 oppure 1234.56
    let s = if s.contains(',') { s.replace('.', "").replace(',', ".") } else { s };
    Decimal::from_str(&s).ok()
}

fn parse_it_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    for f in ["%d/%m/%Y", "%Y-%m-%d", "%d-%m-%Y", "%d.%m.%Y", "%d/%m/%y"] {
        if let Ok(d) = NaiveDate::parse_from_str(s, f) {
            return Some(d);
        }
    }
    None
}

fn split_csv(line: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == sep && !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).collect()
}

/// Legge un CSV di movimenti: riconosce le colonne dall'intestazione (data contabile/valuta,
/// descrizione/causale, importo oppure dare/avere o entrate/uscite).
pub fn parse_csv(text: &str) -> Result<Statement, String> {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header = lines.next().ok_or("File vuoto")?;
    let sep = [';', '\t', ','].into_iter().max_by_key(|c| header.matches(*c).count()).unwrap_or(';');
    let h: Vec<String> = split_csv(header, sep).into_iter().map(|s| s.to_lowercase()).collect();
    let find = |keys: &[&str]| h.iter().position(|c| keys.iter().any(|k| c.contains(k)));
    let date_col = find(&["data contab", "data oper", "data reg", "data"]).ok_or("Colonna della data non trovata")?;
    let value_col = find(&["valuta"]).filter(|c| *c != date_col);
    let desc_col = find(&["descriz", "causale", "dettagli", "operazione"]);
    let amount_col = find(&["importo", "amount"]);
    let out_col = find(&["dare", "uscit", "addebit"]);
    let in_col = find(&["avere", "entrat", "accredit"]);
    if amount_col.is_none() && out_col.is_none() && in_col.is_none() {
        return Err("Colonna dell'importo non trovata (importo, oppure dare/avere)".into());
    }
    let mut st = Statement::default();
    for l in lines {
        let c = split_csv(l, sep);
        let get = |i: Option<usize>| i.and_then(|i| c.get(i)).map(|s| s.as_str()).unwrap_or("");
        let Some(date) = parse_it_date(get(Some(date_col))) else { continue };
        let amount = match amount_col.and_then(|i| parse_it_amount(get(Some(i)))) {
            Some(a) => a,
            None => {
                let inn = parse_it_amount(get(in_col)).unwrap_or_default().abs();
                let out = parse_it_amount(get(out_col)).unwrap_or_default().abs();
                inn - out
            }
        };
        if amount.is_zero() {
            continue;
        }
        st.lines.push(StatementLine { date, value_date: parse_it_date(get(value_col)), amount, currency: "EUR".into(), counterparty: String::new(), remittance: get(desc_col).to_string(), reference: String::new() });
    }
    if st.lines.is_empty() {
        return Err("Nessun movimento riconosciuto nel file".into());
    }
    Ok(st)
}

/// Legge un estratto conto camt (XML) o CSV.
pub fn parse_statement(content: &str) -> Result<Statement, String> {
    if content.trim_start_matches('\u{feff}').trim_start().starts_with('<') {
        parse_camt(content)
    } else {
        parse_csv(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    pub const CAMT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:camt.053.001.02">
 <BkToCstmrStmt>
  <GrpHdr><MsgId>STM-1</MsgId><CreDtTm>2026-11-30T18:00:00</CreDtTm></GrpHdr>
  <Stmt>
   <Id>1</Id>
   <Acct><Id><IBAN>IT60X0542811101000000123456</IBAN></Id></Acct>
   <Bal><Tp><CdOrPrtry><Cd>OPBD</Cd></CdOrPrtry></Tp><Amt Ccy="EUR">1000.00</Amt><CdtDbtInd>CRDT</CdtDbtInd><Dt><Dt>2026-11-01</Dt></Dt></Bal>
   <Bal><Tp><CdOrPrtry><Cd>CLBD</Cd></CdOrPrtry></Tp><Amt Ccy="EUR">1601.50</Amt><CdtDbtInd>CRDT</CdtDbtInd><Dt><Dt>2026-11-30</Dt></Dt></Bal>
   <Ntry>
    <Amt Ccy="EUR">610.00</Amt><CdtDbtInd>CRDT</CdtDbtInd><Sts>BOOK</Sts>
    <BookgDt><Dt>2026-11-10</Dt></BookgDt><ValDt><Dt>2026-11-10</Dt></ValDt>
    <AcctSvcrRef>BANK-001</AcctSvcrRef>
    <NtryDtls><TxDtls>
     <Refs><EndToEndId>E2E-ROSSI-42</EndToEndId></Refs>
     <RltdPties><Dbtr><Nm>Rossi Arredamenti Srl</Nm></Dbtr></RltdPties>
     <RmtInf><Ustrd>Saldo fattura n. 42 del 15/10/2026</Ustrd></RmtInf>
    </TxDtls></NtryDtls>
   </Ntry>
   <Ntry>
    <Amt Ccy="EUR">2.50</Amt><CdtDbtInd>DBIT</CdtDbtInd><Sts>BOOK</Sts>
    <BookgDt><Dt>2026-11-30</Dt></BookgDt>
    <AddtlNtryInf>Commissioni e spese tenuta conto</AddtlNtryInf>
   </Ntry>
   <Ntry>
    <Amt Ccy="EUR">6.00</Amt><CdtDbtInd>DBIT</CdtDbtInd><Sts>BOOK</Sts>
    <BookgDt><DtTm>2026-11-20T10:00:00</DtTm></BookgDt>
    <NtryDtls><TxDtls><RltdPties><Cdtr><Nm>Telefonia Spa</Nm></Cdtr></RltdPties><RmtInf><Ustrd>Addebito SDD bolletta</Ustrd></RmtInf></TxDtls></NtryDtls>
   </Ntry>
  </Stmt>
 </BkToCstmrStmt>
</Document>"#;

    #[test]
    fn camt053_letto_con_segni_controparti_e_saldi() {
        let s = parse_statement(CAMT).unwrap();
        assert_eq!(s.iban, "IT60X0542811101000000123456");
        assert_eq!(s.opening, Some(dec!(1000.00)));
        assert_eq!(s.closing, Some(dec!(1601.50)));
        assert_eq!(s.lines.len(), 3);
        let a = &s.lines[0];
        assert_eq!(a.amount, dec!(610.00));
        assert_eq!(a.counterparty, "Rossi Arredamenti Srl");
        assert_eq!(a.reference, "E2E-ROSSI-42");
        assert!(a.remittance.contains("fattura n. 42"));
        assert_eq!(s.lines[1].amount, dec!(-2.50));
        assert_eq!(s.lines[1].remittance, "Commissioni e spese tenuta conto");
        assert_eq!(s.lines[2].date, NaiveDate::from_ymd_opt(2026, 11, 20).unwrap());
        assert_eq!(s.lines[2].counterparty, "Telefonia Spa");
        // saldo iniziale + movimenti = saldo finale
        let sum: Decimal = s.lines.iter().map(|l| l.amount).sum();
        assert_eq!(s.opening.unwrap() + sum, s.closing.unwrap());
        // chiave stabile e diversa per righe diverse
        assert_eq!(a.key(), parse_statement(CAMT).unwrap().lines[0].key());
        assert_ne!(a.key(), s.lines[1].key());
    }

    #[test]
    fn csv_con_dare_avere_o_importo() {
        let a = "Data contabile;Data valuta;Descrizione;Dare;Avere\n10/11/2026;10/11/2026;\"Bonifico da Rossi; fattura 42\";;610,00\n30/11/2026;30/11/2026;Commissioni;2,50;\n";
        let s = parse_statement(a).unwrap();
        assert_eq!(s.lines.len(), 2);
        assert_eq!(s.lines[0].amount, dec!(610.00));
        assert_eq!(s.lines[0].remittance, "Bonifico da Rossi; fattura 42");
        assert_eq!(s.lines[1].amount, dec!(-2.50));
        let b = "data,descrizione,importo
2026-11-10,Incasso,\"1.234,56\"
";
        assert_eq!(parse_statement(b).unwrap().lines[0].amount, dec!(1234.56));
        let c = "Data;Operazione;Importo\n10/11/2026;Incasso POS;-1.234,56\n";
        assert_eq!(parse_statement(c).unwrap().lines[0].amount, dec!(-1234.56));
        assert!(parse_statement("nulla").is_err());
        assert!(parse_statement("<x/>").is_err());
    }
}
