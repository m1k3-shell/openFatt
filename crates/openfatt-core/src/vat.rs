//! Registri IVA, liquidazione periodica e Comunicazione delle liquidazioni periodiche (LIPE).
//!
//! Fonti: d.P.R. 633/1972 (artt. 23, 24, 25), d.P.R. 100/1998 e d.P.R. 542/1999 (art. 7),
//! istruzioni e specifiche tecniche della Comunicazione liquidazioni periodiche IVA
//! (provvedimento 14 marzo 2024, schema `fornituraIvp_2018_v1.xsd`, in `specs/lipe`).
//!
//! Le funzioni sono pure: ricevono le righe dei registri già raccolte dall'archivio.

use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::money::{fmt_eur, round2};

// ---------------------------------------------------------------------------
// Periodi
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Periodicity {
    #[default]
    Monthly,
    Quarterly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Period {
    pub year: i32,
    /// Mese (1-12) per le liquidazioni mensili, trimestre (1-4) per le trimestrali.
    pub index: u32,
    pub quarterly: bool,
}

const MONTHS: [&str; 12] = ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"];

impl Period {
    pub fn new(year: i32, index: u32, p: Periodicity) -> Period {
        Period { year, index, quarterly: p == Periodicity::Quarterly }
    }
    pub fn count(self) -> u32 {
        if self.quarterly { 4 } else { 12 }
    }
    pub fn first(self) -> NaiveDate {
        let m = if self.quarterly { (self.index - 1) * 3 + 1 } else { self.index };
        NaiveDate::from_ymd_opt(self.year, m, 1).unwrap()
    }
    pub fn last(self) -> NaiveDate {
        let m = if self.quarterly { self.index * 3 } else { self.index };
        let next = if m == 12 { NaiveDate::from_ymd_opt(self.year + 1, 1, 1) } else { NaiveDate::from_ymd_opt(self.year, m + 1, 1) };
        next.unwrap().pred_opt().unwrap()
    }
    pub fn label(self) -> String {
        if self.quarterly {
            format!("{}° trimestre {}", self.index, self.year)
        } else {
            format!("{} {}", MONTHS[(self.index - 1) as usize], self.year)
        }
    }
    pub fn prev(self) -> Option<Period> {
        (self.index > 1).then(|| Period { index: self.index - 1, ..self })
    }
    /// Trimestre solare a cui appartiene (per la Comunicazione).
    pub fn quarter(self) -> u32 {
        if self.quarterly { self.index } else { (self.index - 1) / 3 + 1 }
    }
    /// Il periodo che contiene la data.
    pub fn containing(d: NaiveDate, p: Periodicity) -> Period {
        match p {
            Periodicity::Monthly => Period { year: d.year(), index: d.month(), quarterly: false },
            Periodicity::Quarterly => Period { year: d.year(), index: (d.month() - 1) / 3 + 1, quarterly: true },
        }
    }
    /// Codice tributo F24 del versamento.
    pub fn f24_code(self) -> &'static str {
        const M: [&str; 12] = ["6001", "6002", "6003", "6004", "6005", "6006", "6007", "6008", "6009", "6010", "6011", "6012"];
        const Q: [&str; 4] = ["6031", "6032", "6033", "6099"];
        if self.quarterly { Q[(self.index - 1) as usize] } else { M[(self.index - 1) as usize] }
    }
    /// Scadenza del versamento (senza slittamenti per festivi: si sposta al primo giorno
    /// lavorativo se cade di sabato o domenica). Il 4° trimestre si versa con la dichiarazione annuale.
    pub fn payment_due(self) -> NaiveDate {
        let d = if self.quarterly {
            match self.index {
                1 => NaiveDate::from_ymd_opt(self.year, 5, 16),
                2 => NaiveDate::from_ymd_opt(self.year, 8, 20),
                3 => NaiveDate::from_ymd_opt(self.year, 11, 16),
                _ => NaiveDate::from_ymd_opt(self.year + 1, 3, 16),
            }
        } else if self.index == 12 {
            NaiveDate::from_ymd_opt(self.year + 1, 1, 16)
        } else {
            NaiveDate::from_ymd_opt(self.year, self.index + 1, 16)
        }
        .unwrap();
        weekday_forward(d)
    }
    /// Scadenza della Comunicazione del trimestre a cui appartiene il periodo.
    pub fn lipe_due(self) -> NaiveDate {
        let q = self.quarter();
        let d = match q {
            1 => NaiveDate::from_ymd_opt(self.year, 5, 31),
            2 => NaiveDate::from_ymd_opt(self.year, 9, 30),
            3 => NaiveDate::from_ymd_opt(self.year, 11, 30),
            _ => NaiveDate::from_ymd_opt(self.year + 1, 3, 1).map(|d| d.pred_opt().unwrap()),
        }
        .unwrap();
        weekday_forward(d)
    }
}

fn weekday_forward(mut d: NaiveDate) -> NaiveDate {
    while matches!(d.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun) {
        d = d.succ_opt().unwrap();
    }
    d
}

/// Soglia sotto la quale il debito non si versa e passa al periodo successivo:
/// 100 euro dal 2024 (d.lgs. 1/2024), 25,82 euro prima.
pub fn payment_threshold(year: i32) -> Decimal {
    if year >= 2024 {
        Decimal::ONE_HUNDRED
    } else {
        Decimal::new(2582, 2)
    }
}

// ---------------------------------------------------------------------------
// Righe dei registri
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Register {
    /// Fatture emesse (art. 23).
    Sales,
    /// Fatture di acquisto (art. 25).
    Purchases,
}

/// Imponibile e imposta per aliquota (o natura) di un documento.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RateAmount {
    pub rate: Decimal,
    pub natura: Option<String>,
    pub taxable: Decimal,
    pub tax: Decimal,
    /// I immediata, D differita, S scissione dei pagamenti.
    pub liability: String,
}

impl RateAmount {
    pub fn label(&self) -> String {
        match &self.natura {
            Some(n) => n.clone(),
            None => format!("{}%", self.rate.normalize()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RegisterRow {
    pub document_id: i64,
    /// Data di registrazione (vendite: data del documento; acquisti: data di ricezione).
    pub date: NaiveDate,
    pub doc_date: NaiveDate,
    /// Numero (vendite) o protocollo (acquisti).
    pub number: String,
    /// Numero del fornitore (acquisti).
    pub external_number: String,
    pub kind_label: String,
    pub fe_type: String,
    pub party: String,
    pub party_vat: String,
    /// Importi già con il segno (le note di credito sono negative).
    pub rates: Vec<RateAmount>,
    pub total: Decimal,
    /// Autofattura per integrazione (reverse charge): l'IVA è anche detratta. Nel registro
    /// acquisti compare come copia, esclusa dai totali (è già conteggiata dalle vendite).
    pub reverse_charge: bool,
}

impl RegisterRow {
    pub fn taxable(&self) -> Decimal {
        self.rates.iter().map(|r| r.taxable).sum()
    }
    pub fn tax(&self) -> Decimal {
        self.rates.iter().map(|r| r.tax).sum()
    }
}

/// Corrispettivi di un giorno per aliquota.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RetailRow {
    pub date: NaiveDate,
    pub count: usize,
    pub gross: Decimal,
    /// (aliquota o natura, imponibile, imposta)
    pub rates: Vec<(String, Decimal, Decimal)>,
}

/// Imposta ad esigibilità differita diventata esigibile (o detraibile) nel periodo,
/// per incasso/pagamento o per decorso di un anno.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeferredRow {
    pub document_id: i64,
    pub label: String,
    pub register: Option<Register>,
    pub tax: Decimal,
}

/// Riepilogo per aliquota di un registro.
pub fn summary(rows: &[RegisterRow]) -> Vec<RateAmount> {
    let mut out: Vec<RateAmount> = Vec::new();
    for r in rows {
        for a in &r.rates {
            match out.iter_mut().find(|x| x.rate == a.rate && x.natura == a.natura && x.liability == a.liability) {
                Some(x) => {
                    x.taxable += a.taxable;
                    x.tax += a.tax;
                }
                None => out.push(a.clone()),
            }
        }
    }
    out.sort_by(|a, b| b.rate.cmp(&a.rate).then(a.natura.cmp(&b.natura)).then(a.liability.cmp(&b.liability)));
    out
}

// ---------------------------------------------------------------------------
// Liquidazione
// ---------------------------------------------------------------------------

/// Totali del periodo ricavati dai registri (righi VP2-VP5).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PeriodTotals {
    /// VP2: operazioni attive al netto dell'IVA (fatture e corrispettivi).
    pub sales_taxable: Decimal,
    /// VP3: operazioni passive al netto dell'IVA.
    pub purchases_taxable: Decimal,
    /// VP4: IVA esigibile.
    pub vat_due: Decimal,
    /// VP5: IVA detratta.
    pub vat_deducted: Decimal,
    /// IVA in scissione dei pagamenti (non esigibile dal cedente).
    pub split_payment: Decimal,
    /// IVA ad esigibilità differita sorta nel periodo e non ancora esigibile.
    pub deferred_pending: Decimal,
}

impl PeriodTotals {
    pub fn compute(sales: &[RegisterRow], purchases: &[RegisterRow], retail: &[RetailRow], deferred: &[DeferredRow]) -> PeriodTotals {
        let mut t = PeriodTotals::default();
        for r in sales {
            if r.reverse_charge {
                // integrazione della fattura del fornitore (reverse charge): l'imponibile è un
                // acquisto (VP3), l'imposta è sia esigibile (VP4) sia detratta (VP5)
                for a in &r.rates {
                    t.purchases_taxable += a.taxable;
                    t.vat_due += a.tax;
                    t.vat_deducted += a.tax;
                }
                continue;
            }
            for a in &r.rates {
                t.sales_taxable += a.taxable;
                match a.liability.as_str() {
                    "S" => t.split_payment += a.tax,
                    "D" => t.deferred_pending += a.tax,
                    _ => t.vat_due += a.tax,
                }
            }
        }
        for r in purchases.iter().filter(|r| !r.reverse_charge) {
            for a in &r.rates {
                t.purchases_taxable += a.taxable;
                if a.liability == "D" {
                    continue;
                }
                t.vat_deducted += a.tax;
            }
        }
        for d in retail {
            for (_, taxable, tax) in &d.rates {
                t.sales_taxable += *taxable;
                t.vat_due += *tax;
            }
        }
        for d in deferred {
            match d.register {
                Some(Register::Purchases) => t.vat_deducted += d.tax,
                _ => t.vat_due += d.tax,
            }
        }
        t.sales_taxable = round2(t.sales_taxable);
        t.purchases_taxable = round2(t.purchases_taxable);
        t.vat_due = round2(t.vat_due);
        t.vat_deducted = round2(t.vat_deducted);
        t
    }
}

/// Dati inseriti a mano per un periodo (righi VP9-VP11, VP13).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PeriodInputs {
    /// VP9: credito dell'anno precedente usato in questa liquidazione.
    pub prev_year_credit: Decimal,
    /// VP10: versamenti auto con F24 elementi identificativi.
    pub car_payments: Decimal,
    /// VP11: crediti d'imposta usati.
    pub tax_credits: Decimal,
    /// VP13: acconto dovuto (dicembre o 4° trimestre), con il metodo (1-4).
    pub advance: Decimal,
    pub advance_method: u8,
    /// Versamento eseguito (data), se registrato.
    pub paid_on: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Liquidation {
    pub totals: PeriodTotals,
    pub inputs: PeriodInputs,
    /// VP6
    pub vat_payable: Decimal,
    pub vat_credit: Decimal,
    /// VP7: debito del periodo precedente non versato perché non superiore alla soglia.
    pub prev_debt: Decimal,
    /// VP8: credito del periodo precedente (stesso anno).
    pub prev_credit: Decimal,
    /// VP12: interessi 1% per le liquidazioni trimestrali.
    pub interest: Decimal,
    /// VP14
    pub to_pay: Decimal,
    pub credit: Decimal,
    /// L'importo da versare supera la soglia: va versato ora.
    pub pay_now: bool,
    /// 4° trimestre dei contribuenti trimestrali: si versa con la dichiarazione annuale.
    pub annual: bool,
}

/// Liquidazione di un periodo dati i totali, gli importi manuali e il risultato del periodo
/// precedente dello stesso anno.
pub fn liquidate(period: Period, totals: PeriodTotals, inputs: PeriodInputs, prev: Option<&Liquidation>) -> Liquidation {
    let mut l = Liquidation { totals: totals.clone(), inputs: inputs.clone(), ..Default::default() };
    let a = totals.vat_due - totals.vat_deducted;
    if a > Decimal::ZERO {
        l.vat_payable = a;
    } else {
        l.vat_credit = -a;
    }
    let threshold = payment_threshold(period.year);
    if let Some(p) = prev {
        // VP7 non ammesso per gennaio, dicembre e per i trimestri 1, 4 (specifiche tecniche)
        let vp7_allowed = !(period.index == 1 || (!period.quarterly && period.index == 12) || (period.quarterly && period.index == 4));
        if vp7_allowed && !p.pay_now && !p.annual {
            l.prev_debt = p.to_pay;
        }
        l.prev_credit = p.credit;
    }
    let annual = period.quarterly && period.index == 4;
    let base = l.vat_payable + l.prev_debt - l.vat_credit - l.prev_credit - inputs.prev_year_credit - inputs.car_payments - inputs.tax_credits;
    if period.quarterly && !annual && base > Decimal::ZERO {
        l.interest = round2(base / Decimal::ONE_HUNDRED);
    }
    let result = base + l.interest - inputs.advance;
    if result > Decimal::ZERO {
        l.to_pay = result;
    } else {
        l.credit = -result;
    }
    l.annual = annual;
    // il debito sotto soglia si riporta solo se il periodo successivo può riceverlo
    let next_accepts = match (period.quarterly, period.index) {
        (true, 3) => false,
        (true, 4) => false,
        (false, 11) | (false, 12) => false,
        _ => true,
    };
    l.pay_now = !annual && l.to_pay > Decimal::ZERO && (l.to_pay > threshold || !next_accepts);
    l
}

impl Liquidation {
    /// Descrizione del risultato per l'utente.
    pub fn outcome(&self, period: Period) -> String {
        if self.annual {
            if self.to_pay > Decimal::ZERO {
                return format!("€ {} da versare con il saldo della dichiarazione annuale (codice 6099), con gli interessi dell'1%", fmt_eur(self.to_pay));
            }
            return format!("Credito di € {} da riportare nella dichiarazione annuale", fmt_eur(self.credit));
        }
        if self.pay_now {
            format!("€ {} da versare entro il {} con F24, codice tributo {}", fmt_eur(self.to_pay), period.payment_due().format("%d/%m/%Y"), period.f24_code())
        } else if self.to_pay > Decimal::ZERO {
            format!("€ {} non si versano (non superiori a {} €): passano al periodo successivo", fmt_eur(self.to_pay), payment_threshold(period.year).normalize())
        } else if self.credit > Decimal::ZERO {
            format!("Credito di € {} da riportare nel periodo successivo", fmt_eur(self.credit))
        } else {
            "Nessun importo da versare".to_string()
        }
    }
}

/// Liquidazioni di tutti i periodi dell'anno fino a `upto` compreso.
pub fn liquidate_year(year: i32, p: Periodicity, upto: u32, totals: impl Fn(Period) -> PeriodTotals, inputs: impl Fn(Period) -> PeriodInputs) -> Vec<(Period, Liquidation)> {
    let mut out: Vec<(Period, Liquidation)> = Vec::new();
    for i in 1..=upto {
        let period = Period::new(year, i, p);
        let prev = out.last().map(|x| &x.1);
        let l = liquidate(period, totals(period), inputs(period), prev);
        out.push((period, l));
    }
    out
}

// ---------------------------------------------------------------------------
// Comunicazione liquidazioni periodiche IVA (LIPE)
// ---------------------------------------------------------------------------

/// Dati del contribuente per la Comunicazione.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LipeSubject {
    pub tax_code: String,
    pub vat_number: String,
    /// Dichiarante (obbligatorio per i soggetti diversi dalle persone fisiche).
    pub declarant_cf: String,
    pub declarant_role: u8,
}

fn vp(d: Decimal) -> String {
    let r = round2(d.abs());
    let s = format!("{:.2}", r).replace('.', ",");
    if d < Decimal::ZERO && !r.is_zero() {
        format!("-{}", s)
    } else {
        s
    }
}

/// Controlli sui dati del contribuente.
pub fn lipe_check(s: &LipeSubject) -> Vec<String> {
    let mut p = Vec::new();
    let cf = s.tax_code.trim().to_ascii_uppercase();
    if !(cf.len() == 11 && cf.chars().all(|c| c.is_ascii_digit())) && !crate::fiscal::codice_fiscale_valido(&cf) {
        p.push("Codice fiscale dell'azienda mancante o non valido (Opzioni > Azienda)".into());
    }
    if !crate::fiscal::partita_iva_valida(s.vat_number.trim()) {
        p.push("Partita IVA dell'azienda mancante o non valida".into());
    }
    if cf.len() == 11 {
        if !crate::fiscal::codice_fiscale_valido(&s.declarant_cf.trim().to_ascii_uppercase()) {
            p.push("Per le società serve il codice fiscale del dichiarante (es. il legale rappresentante)".into());
        }
        if !matches!(s.declarant_role, 1..=9 | 11..=15) {
            p.push("Codice carica del dichiarante non valido".into());
        }
    }
    p
}

/// File XML della Comunicazione per un trimestre: un modulo per ogni liquidazione del trimestre.
pub fn lipe_xml(s: &LipeSubject, year: i32, modules: &[(Period, Liquidation)]) -> Result<String, Vec<String>> {
    let mut p = lipe_check(s);
    if modules.is_empty() || modules.len() > 5 {
        p.push("Da 1 a 5 liquidazioni per Comunicazione".into());
    }
    let q0 = modules.first().map(|m| m.0.quarter());
    if modules.iter().any(|m| Some(m.0.quarter()) != q0 || m.0.year != year) {
        p.push("Le liquidazioni devono essere dello stesso trimestre".into());
    }
    if !p.is_empty() {
        return Err(p);
    }
    let cf = s.tax_code.trim().to_ascii_uppercase();
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    x.push_str("<iv:Fornitura xmlns:iv=\"urn:www.agenziaentrate.gov.it:specificheTecniche:sco:ivp\" xmlns:ds=\"http://www.w3.org/2000/09/xmldsig#\">\n");
    x.push_str("  <iv:Intestazione>\n    <iv:CodiceFornitura>IVP18</iv:CodiceFornitura>\n  </iv:Intestazione>\n");
    x.push_str("  <iv:Comunicazione identificativo=\"00001\">\n    <iv:Frontespizio>\n");
    x.push_str(&format!("      <iv:CodiceFiscale>{}</iv:CodiceFiscale>\n", cf));
    x.push_str(&format!("      <iv:AnnoImposta>{}</iv:AnnoImposta>\n", year));
    x.push_str(&format!("      <iv:PartitaIVA>{}</iv:PartitaIVA>\n", s.vat_number.trim()));
    if cf.len() == 11 {
        x.push_str(&format!("      <iv:CFDichiarante>{}</iv:CFDichiarante>\n", s.declarant_cf.trim().to_ascii_uppercase()));
        x.push_str(&format!("      <iv:CodiceCaricaDichiarante>{}</iv:CodiceCaricaDichiarante>\n", s.declarant_role));
    }
    x.push_str("      <iv:FirmaDichiarazione>1</iv:FirmaDichiarazione>\n    </iv:Frontespizio>\n    <iv:DatiContabili>\n");
    for (n, (period, l)) in modules.iter().enumerate() {
        let el = |x: &mut String, tag: &str, v: Decimal| x.push_str(&format!("        <iv:{}>{}</iv:{}>\n", tag, vp(v), tag));
        x.push_str("      <iv:Modulo>\n");
        x.push_str(&format!("        <iv:NumeroModulo>{}</iv:NumeroModulo>\n", n + 1));
        if period.quarterly {
            // contribuenti trimestrali (art. 7 d.P.R. 542/1999): il 4° trimestre si indica con 5
            x.push_str(&format!("        <iv:Trimestre>{}</iv:Trimestre>\n", if period.index == 4 { 5 } else { period.index }));
        } else {
            x.push_str(&format!("        <iv:Mese>{}</iv:Mese>\n", period.index));
        }
        el(&mut x, "TotaleOperazioniAttive", l.totals.sales_taxable);
        el(&mut x, "TotaleOperazioniPassive", l.totals.purchases_taxable);
        el(&mut x, "IvaEsigibile", l.totals.vat_due);
        el(&mut x, "IvaDetratta", l.totals.vat_deducted);
        if l.vat_credit > Decimal::ZERO {
            el(&mut x, "IvaCredito", l.vat_credit);
        } else {
            el(&mut x, "IvaDovuta", l.vat_payable);
        }
        if !l.prev_debt.is_zero() {
            el(&mut x, "DebitoPrecedente", l.prev_debt);
        }
        if !l.prev_credit.is_zero() {
            el(&mut x, "CreditoPeriodoPrecedente", l.prev_credit);
        }
        if !l.inputs.prev_year_credit.is_zero() {
            el(&mut x, "CreditoAnnoPrecedente", l.inputs.prev_year_credit);
        }
        if !l.inputs.car_payments.is_zero() {
            el(&mut x, "VersamentiAutoUE", l.inputs.car_payments);
        }
        if !l.inputs.tax_credits.is_zero() && !l.annual {
            el(&mut x, "CreditiImposta", l.inputs.tax_credits);
        }
        if !l.interest.is_zero() {
            el(&mut x, "InteressiDovuti", l.interest);
        }
        if l.inputs.advance >= Decimal::new(10329, 2) {
            x.push_str(&format!("        <iv:Metodo>{}</iv:Metodo>\n", l.inputs.advance_method.clamp(1, 4)));
            el(&mut x, "Acconto", l.inputs.advance);
        }
        if !l.annual {
            if l.credit > Decimal::ZERO {
                el(&mut x, "ImportoACredito", l.credit);
            } else {
                el(&mut x, "ImportoDaVersare", l.to_pay);
            }
        }
        x.push_str("      </iv:Modulo>\n");
    }
    x.push_str("    </iv:DatiContabili>\n  </iv:Comunicazione>\n</iv:Fornitura>\n");
    Ok(x)
}

/// Nome del file: IT + codice fiscale del trasmittente + _LI_ + progressivo (max 5 caratteri).
pub fn lipe_file_name(sender_cf: &str, progressive: u64) -> String {
    format!("IT{}_LI_{:05}.xml", sender_cf.trim().to_ascii_uppercase(), progressive % 100_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dec;

    fn tot(due: Decimal, ded: Decimal) -> PeriodTotals {
        PeriodTotals { sales_taxable: due * dec!(5), purchases_taxable: ded * dec!(5), vat_due: due, vat_deducted: ded, ..Default::default() }
    }

    #[test]
    fn periodi_e_scadenze() {
        let m = Period::new(2026, 2, Periodicity::Monthly);
        assert_eq!((m.first(), m.last()), (NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(), NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()));
        assert_eq!(m.f24_code(), "6002");
        assert_eq!(m.payment_due(), NaiveDate::from_ymd_opt(2026, 3, 16).unwrap());
        assert_eq!(m.lipe_due(), NaiveDate::from_ymd_opt(2026, 6, 1).unwrap()); // 31/5/2026 è domenica
        let q = Period::new(2026, 2, Periodicity::Quarterly);
        assert_eq!(q.last(), NaiveDate::from_ymd_opt(2026, 6, 30).unwrap());
        assert_eq!(q.f24_code(), "6032");
        assert_eq!(q.payment_due(), NaiveDate::from_ymd_opt(2026, 8, 20).unwrap());
        assert_eq!(Period::containing(NaiveDate::from_ymd_opt(2026, 11, 7).unwrap(), Periodicity::Quarterly).index, 4);
        assert_eq!(Period::new(2026, 12, Periodicity::Monthly).payment_due(), NaiveDate::from_ymd_opt(2027, 1, 18).unwrap()); // 16/1/2027 è sabato
        assert_eq!(payment_threshold(2023), dec!(25.82));
        assert_eq!(payment_threshold(2026), dec!(100));
    }

    #[test]
    fn totali_dai_registri() {
        let d = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let ra = |t: Decimal, v: Decimal, l: &str| RateAmount { rate: dec!(22), natura: None, taxable: t, tax: v, liability: l.into() };
        let sales = vec![
            RegisterRow { date: d, rates: vec![ra(dec!(1000), dec!(220), "I")], ..Default::default() },
            RegisterRow { date: d, rates: vec![ra(dec!(500), dec!(110), "S")], ..Default::default() },
            RegisterRow { date: d, rates: vec![ra(dec!(-100), dec!(-22), "I")], ..Default::default() },
            RegisterRow { date: d, rates: vec![ra(dec!(200), dec!(44), "D")], ..Default::default() },
            // integrazione reverse charge
            RegisterRow { date: d, rates: vec![ra(dec!(300), dec!(66), "I")], reverse_charge: true, ..Default::default() },
        ];
        let purchases = vec![RegisterRow { date: d, rates: vec![ra(dec!(400), dec!(88), "I")], ..Default::default() }];
        let retail = vec![RetailRow { date: d, count: 3, gross: dec!(122), rates: vec![("22".into(), dec!(100), dec!(22))] }];
        let deferred = vec![DeferredRow { tax: dec!(10), register: Some(Register::Sales), ..Default::default() }];
        let t = PeriodTotals::compute(&sales, &purchases, &retail, &deferred);
        assert_eq!(t.sales_taxable, dec!(1700)); // 1000+500-100+200+100 (l'integrazione va negli acquisti)
        assert_eq!(t.vat_due, dec!(296)); // 220-22+66+22+10 (scissione e differita esclusi)
        assert_eq!(t.split_payment, dec!(110));
        assert_eq!(t.deferred_pending, dec!(44));
        assert_eq!(t.vat_deducted, dec!(154)); // 88 + 66 integrazione
        assert_eq!(t.purchases_taxable, dec!(700));
    }

    #[test]
    fn catena_mensile_con_crediti_e_debiti_sotto_soglia() {
        let totals = |p: Period| match p.index {
            1 => tot(dec!(1000), dec!(1200)), // credito 200
            2 => tot(dec!(1250), dec!(1000)), // 250 - 200 = 50: sotto soglia
            3 => tot(dec!(1000), dec!(900)),  // 100 + 50 = 150: si versa
            _ => tot(dec!(0), dec!(0)),
        };
        let y = liquidate_year(2026, Periodicity::Monthly, 3, totals, |_| PeriodInputs::default());
        assert_eq!(y[0].1.credit, dec!(200));
        assert_eq!(y[1].1.prev_credit, dec!(200));
        assert_eq!((y[1].1.to_pay, y[1].1.pay_now), (dec!(50), false));
        assert_eq!(y[2].1.prev_debt, dec!(50));
        assert_eq!((y[2].1.to_pay, y[2].1.pay_now), (dec!(150), true));
        assert!(y[2].1.outcome(y[2].0).contains("6003"));
        // novembre: il debito sotto soglia non può passare a dicembre, quindi si versa
        let n = liquidate(Period::new(2026, 11, Periodicity::Monthly), tot(dec!(50), dec!(0)), PeriodInputs::default(), None);
        assert!(n.pay_now);
    }

    #[test]
    fn trimestrali_con_interessi_e_quarto_trimestre() {
        let y = liquidate_year(2026, Periodicity::Quarterly, 4, |_| tot(dec!(1000), dec!(400)), |p| PeriodInputs { advance: if p.index == 4 { dec!(300) } else { dec!(0) }, advance_method: 1, ..Default::default() });
        assert_eq!(y[0].1.interest, dec!(6.00));
        assert_eq!(y[0].1.to_pay, dec!(606.00));
        assert!(y[0].1.outcome(y[0].0).contains("6031"));
        let q4 = &y[3].1;
        assert!(q4.annual && q4.interest.is_zero() && !q4.pay_now);
        assert_eq!(q4.to_pay, dec!(300)); // 600 - 300 di acconto
    }

    fn sample_xml(quarterly: bool, cf: &str) -> String {
        let s = LipeSubject { tax_code: cf.into(), vat_number: "12345678903".into(), declarant_cf: "RSSMRA85T10A562S".into(), declarant_role: 1 };
        let p = if quarterly { Periodicity::Quarterly } else { Periodicity::Monthly };
        let totals = |p: Period| match p.index {
            1 => tot(dec!(1000), dec!(1200)),
            2 => tot(dec!(1250), dec!(1000)),
            _ => tot(dec!(1000.456), dec!(900)),
        };
        let inputs = |p: Period| PeriodInputs { prev_year_credit: if p.index == 1 { dec!(150) } else { dec!(0) }, ..Default::default() };
        let y = liquidate_year(2026, p, if quarterly { 4 } else { 3 }, totals, inputs);
        let mods: Vec<_> = if quarterly { y[3..].to_vec() } else { y };
        lipe_xml(&s, 2026, &mods).unwrap()
    }

    #[test]
    fn comunicazione_lipe() {
        let x = sample_xml(false, "12345678903");
        assert_eq!(x.matches("<iv:Modulo>").count(), 3);
        assert!(x.contains("<iv:CFDichiarante>RSSMRA85T10A562S</iv:CFDichiarante>"));
        assert!(x.contains("<iv:IvaCredito>200,00</iv:IvaCredito>"));
        assert!(x.contains("<iv:CreditoAnnoPrecedente>150,00</iv:CreditoAnnoPrecedente>"));
        assert!(x.contains("<iv:ImportoACredito>350,00</iv:ImportoACredito>"));
        assert!(x.contains("<iv:IvaDovuta>100,46</iv:IvaDovuta>"));
        let q = sample_xml(true, "RSSMRA85T10A562S");
        assert!(q.contains("<iv:Trimestre>5</iv:Trimestre>"));
        assert!(!q.contains("ImportoDaVersare") && !q.contains("CFDichiarante"));
        assert_eq!(lipe_file_name("12345678903", 7), "IT12345678903_LI_00007.xml");
        let bad = LipeSubject { tax_code: "12345678903".into(), vat_number: "12345678903".into(), ..Default::default() };
        assert!(lipe_check(&bad).iter().any(|e| e.contains("dichiarante")));
        // file per la validazione con tools/valida-lipe.ps1
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/xsd-samples/lipe");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lipe-mensile.xml"), x).unwrap();
        std::fs::write(dir.join("lipe-trimestrale.xml"), q).unwrap();
    }
}
