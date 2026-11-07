//! Ri.Ba. in formato CBI: record di 120 caratteri separati da CR LF.
//!
//! Struttura: un record di testa `IB`, per ogni ricevuta i record 14 (importo, scadenza,
//! banche), 20 (creditore), 30 (debitore), 40 (indirizzo e banca del debitore),
//! 50 (riferimento al debito), 51 (numero ricevuta), 70 (chiusura), infine il record di
//! coda `EF` con numero di disposizioni, totale e numero di record.

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::{cents, fmt_ddmmyy, it_bank_codes, riba_text, Company, Problems};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RibaItem {
    pub due: NaiveDate,
    pub amount: Decimal,
    pub debtor_name: String,
    /// Codice fiscale o partita IVA del debitore.
    pub debtor_tax_id: String,
    pub debtor_address: String,
    pub debtor_zip: String,
    pub debtor_city: String,
    pub debtor_province: String,
    /// IBAN della banca d'appoggio del debitore (da cui si ricavano ABI e CAB) ...
    pub debtor_iban: String,
    /// ... oppure ABI e CAB indicati direttamente.
    pub debtor_abi: String,
    pub debtor_cab: String,
    pub debtor_bank: String,
    /// Codice cliente (fino a 16 caratteri).
    pub customer_code: String,
    /// Riferimento al debito (es. "FATTURA N. 12 DEL 03/09/2026"), fino a 80 caratteri.
    pub description: String,
}

impl RibaItem {
    /// ABI e CAB della banca del debitore.
    pub fn debtor_bank_codes(&self) -> Option<(String, String)> {
        let abi = self.debtor_abi.trim();
        let cab = self.debtor_cab.trim();
        if abi.len() == 5 && cab.len() == 5 && abi.chars().chain(cab.chars()).all(|c| c.is_ascii_digit()) {
            return Some((abi.to_string(), cab.to_string()));
        }
        it_bank_codes(&self.debtor_iban).map(|(a, c, _)| (a, c))
    }
}

fn fit(s: &str, n: usize) -> String {
    let t: String = riba_text(s).chars().take(n).collect();
    format!("{:<width$}", t, width = n)
}

fn num(v: i64, n: usize) -> String {
    format!("{:0>width$}", v, width = n)
}

/// Nome del supporto: univoco per mittente e data (20 caratteri).
pub fn support_name(created: chrono::NaiveDateTime, sia: &str) -> String {
    format!("{}{}", created.format("%d%m%y%H%M%S"), sia.trim()).chars().take(20).collect()
}

/// Controlla i dati e costruisce il file. `created` è la data di creazione del flusso.
pub fn riba_file(c: &Company, created: NaiveDate, support: &str, items: &[RibaItem]) -> Result<String, Problems> {
    let mut p = Vec::new();
    let sia = c.sia.trim().to_ascii_uppercase();
    if sia.len() != 5 || !sia.chars().all(|c| c.is_ascii_alphanumeric()) {
        p.push("Codice SIA dell'azienda mancante o non valido (5 caratteri, lo comunica la banca)".to_string());
    }
    let own = it_bank_codes(&c.iban);
    if own.is_none() {
        p.push("IBAN italiano dell'azienda mancante o non valido".to_string());
    }
    if c.tax_id.trim().is_empty() {
        p.push("Partita IVA o codice fiscale dell'azienda mancante".to_string());
    }
    if items.is_empty() {
        p.push("Nessuna ricevuta da presentare".to_string());
    }
    for it in items {
        let who = if it.debtor_name.trim().is_empty() { "(senza nome)".to_string() } else { it.debtor_name.trim().to_string() };
        if it.amount <= Decimal::ZERO {
            p.push(format!("{}: importo non positivo", who));
        }
        if cents(it.amount) > 9_999_999_999_999 {
            p.push(format!("{}: importo troppo grande", who));
        }
        if it.debtor_bank_codes().is_none() {
            p.push(format!("{}: manca la banca d'appoggio (IBAN italiano oppure ABI e CAB) nell'anagrafica", who));
        }
        if it.debtor_tax_id.trim().is_empty() {
            p.push(format!("{}: manca il codice fiscale o la partita IVA", who));
        }
        if it.debtor_city.trim().is_empty() || it.debtor_address.trim().is_empty() {
            p.push(format!("{}: manca l'indirizzo", who));
        }
    }
    if !p.is_empty() {
        return Err(p);
    }
    let (abi, cab, account) = own.unwrap();
    let date = fmt_ddmmyy(created);
    let support = format!("{:<20}", support.chars().take(20).collect::<String>());
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(" IB{}{}{}{}{}E{}", sia, abi, date, support, " ".repeat(74), " ".repeat(6)));
    let creditor24 = fit(&c.name, 24);
    let zip_city = format!("{} {} {}", c.zip.trim(), c.city.trim(), c.province.trim());
    let mut total: i64 = 0;
    for (i, it) in items.iter().enumerate() {
        let n = num((i + 1) as i64, 7);
        let amount = cents(it.amount);
        total += amount;
        let (dabi, dcab) = it.debtor_bank_codes().unwrap();
        // 14: disposizione
        lines.push(format!(
            " 14{}{}{}30000{}-{}{}{}{}{}{}{}4{}{}E",
            n,
            " ".repeat(12),
            fmt_ddmmyy(it.due),
            num(amount, 13),
            abi,
            cab,
            account,
            dabi,
            dcab,
            " ".repeat(12),
            sia,
            fit(&it.customer_code, 16),
            " ".repeat(6)
        ));
        // 20: creditore
        lines.push(format!(" 20{}{}{}{}{}{}", n, creditor24, fit(&c.address, 24), fit(&zip_city, 24), fit(&c.tax_id, 24), " ".repeat(14)));
        // 30: debitore
        lines.push(format!(" 30{}{}{}{}", n, fit(&it.debtor_name, 60), fit(&it.debtor_tax_id, 16), " ".repeat(34)));
        // 40: indirizzo del debitore e banca domiciliataria
        let zip: String = it.debtor_zip.trim().chars().filter(|c| c.is_ascii_digit()).collect();
        let city = riba_text(&it.debtor_city).chars().take(22).collect::<String>();
        let city_prov = format!("{:<22} {:<2}", city, riba_text(&it.debtor_province).chars().take(2).collect::<String>());
        lines.push(format!(" 40{}{}{}{}{}", n, fit(&it.debtor_address, 30), format!("{:0>5}", zip.chars().take(5).collect::<String>()), city_prov, fit(&it.debtor_bank, 50)));
        // 50: riferimento al debito
        lines.push(format!(" 50{}{}{}{}{}", n, fit(&it.description, 80), " ".repeat(10), fit(&c.tax_id, 16), " ".repeat(4)));
        // 51: numero ricevuta attribuito dal creditore
        lines.push(format!(" 51{}{}{}{}", n, num((i + 1) as i64, 10), fit(&c.name, 20), " ".repeat(80)));
        // 70: chiusura della disposizione
        lines.push(format!(" 70{}{}", n, " ".repeat(110)));
    }
    let records = items.len() * 7 + 2;
    lines.push(format!(
        " EF{}{}{}{}{}{}{}{}{}{}E{}",
        sia,
        abi,
        date,
        support,
        " ".repeat(6),
        num(items.len() as i64, 7),
        num(total, 15),
        "0".repeat(15),
        num(records as i64, 7),
        " ".repeat(24),
        " ".repeat(6)
    ));
    debug_assert!(lines.iter().all(|l| l.chars().count() == 120), "{:?}", lines.iter().map(|l| l.chars().count()).collect::<Vec<_>>());
    let mut out = lines.join("\r\n");
    out.push_str("\r\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn company() -> Company {
        Company {
            name: "Bottega Demo S.r.l.".into(),
            address: "Via Roma 1".into(),
            zip: "38122".into(),
            city: "Trento".into(),
            province: "TN".into(),
            tax_id: "01234567897".into(),
            iban: "IT60X0542811101000000123456".into(),
            sia: "A1B2C".into(),
            ..Default::default()
        }
    }

    fn item(amount: rust_decimal::Decimal) -> RibaItem {
        RibaItem {
            due: NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(),
            amount,
            debtor_name: "Rossi Arredamenti S.p.A.".into(),
            debtor_tax_id: "06363391001".into(),
            debtor_address: "Viale Europa 10".into(),
            debtor_zip: "00144".into(),
            debtor_city: "Roma".into(),
            debtor_province: "RM".into(),
            debtor_iban: "IT36A0301503200000003517230".into(),
            debtor_bank: "Banca di prova - Roma".into(),
            customer_code: "C0001".into(),
            description: "Fattura n. 12 del 30/11/2026".into(),
            ..Default::default()
        }
    }

    #[test]
    fn tracciato_completo() {
        let f = riba_file(&company(), NaiveDate::from_ymd_opt(2026, 11, 7).unwrap(), "071126103000A1B2C", &[item(dec!(1220.00)), item(dec!(350.5))]).unwrap();
        let lines: Vec<&str> = f.trim_end_matches("\r\n").split("\r\n").collect();
        assert_eq!(lines.len(), 16);
        assert!(lines.iter().all(|l| l.len() == 120));
        let ib = lines[0];
        assert_eq!(&ib[1..3], "IB");
        assert_eq!(&ib[3..8], "A1B2C");
        assert_eq!(&ib[8..13], "05428");
        assert_eq!(&ib[13..19], "071126");
        assert_eq!(&ib[113..114], "E");
        let r14 = lines[1];
        assert_eq!(&r14[1..3], "14");
        assert_eq!(&r14[3..10], "0000001");
        assert_eq!(&r14[22..28], "311226"); // scadenza
        assert_eq!(&r14[28..33], "30000"); // causale
        assert_eq!(&r14[33..46], "0000000122000"); // importo in centesimi
        assert_eq!(&r14[46..47], "-");
        assert_eq!(&r14[47..57], "0542811101"); // ABI e CAB del creditore
        assert_eq!(&r14[57..69], "000000123456"); // conto
        assert_eq!(&r14[69..79], "0301503200"); // ABI e CAB del debitore
        assert_eq!(&r14[91..96], "A1B2C");
        assert_eq!(&r14[96..97], "4");
        assert_eq!(&r14[119..120], "E");
        assert!(lines[3].contains("ROSSI ARREDAMENTI S.P.A."));
        let r40 = lines[4];
        assert_eq!(&r40[40..45], "00144");
        assert_eq!(&r40[45..70], "ROMA                   RM");
        assert!(lines[5][10..90].starts_with("FATTURA N. 12 DEL 30/11/2026"));
        assert_eq!(&lines[5][100..116], "01234567897     ");
        let ef = lines[15];
        assert_eq!(&ef[1..3], "EF");
        assert_eq!(&ef[45..52], "0000002"); // disposizioni
        assert_eq!(&ef[52..67], "000000000157050"); // totale
        assert_eq!(&ef[82..89], "0000016"); // record
        assert_eq!(&ef[13..39], &ib[13..39]); // stessa data e supporto
    }

    #[test]
    fn dati_mancanti() {
        let mut c = company();
        c.sia.clear();
        let mut it = item(dec!(10));
        it.debtor_iban.clear();
        let e = riba_file(&c, NaiveDate::from_ymd_opt(2026, 11, 7).unwrap(), "X", &[it]).unwrap_err();
        assert!(e.iter().any(|x| x.contains("SIA")));
        assert!(e.iter().any(|x| x.contains("banca d'appoggio")));
        // ABI e CAB indicati a mano bastano
        let mut it = item(dec!(10));
        it.debtor_iban.clear();
        it.debtor_abi = "03069".into();
        it.debtor_cab = "09606".into();
        assert!(riba_file(&company(), NaiveDate::from_ymd_opt(2026, 11, 7).unwrap(), "X", &[it]).unwrap().contains("0306909606"));
    }
}
