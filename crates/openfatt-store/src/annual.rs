//! Prospetto per la dichiarazione IVA annuale (modello IVA BASE / IVA, quadri VE, VF, VJ,
//! VL e VT), calcolato dai registri e dalle liquidazioni dell'anno.
//!
//! È un aiuto per il commercialista che presenta la dichiarazione: i righi seguono le
//! istruzioni ufficiali (modelli IVA/2026 per l'anno 2025, provvedimento del 15/01/2026),
//! ma alcune voci dipendono da informazioni che il gestionale non ha (beni ammortizzabili,
//! pro-rata, acquisti indetraibili, plafond): sono segnalate tra gli avvisi.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use openfatt_core::money::{pct_of, round2};
use openfatt_core::vat::{Register, RegisterRow};

use crate::{Result, Store};

/// Un rigo del prospetto: codice (VE23), descrizione e valori per colonna.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Rigo {
    pub code: String,
    pub label: String,
    /// (titolo della colonna, valore)
    pub values: Vec<(String, Decimal)>,
}

impl Rigo {
    fn new(code: &str, label: &str, values: &[(&str, Decimal)]) -> Rigo {
        Rigo { code: code.into(), label: label.into(), values: values.iter().map(|(t, v)| (t.to_string(), *v)).collect() }
    }
    pub fn value(&self, i: usize) -> Decimal {
        self.values.get(i).map(|v| v.1).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Quadro {
    pub code: String,
    pub title: String,
    pub righi: Vec<Rigo>,
}

impl Quadro {
    pub fn rigo(&self, code: &str) -> Option<&Rigo> {
        self.righi.iter().find(|r| r.code == code)
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AnnualVat {
    pub year: i32,
    pub quadri: Vec<Quadro>,
    pub warnings: Vec<String>,
}

impl AnnualVat {
    pub fn get(&self, code: &str) -> Option<&Rigo> {
        self.quadri.iter().find_map(|q| q.rigo(code))
    }
    pub fn value(&self, code: &str, col: usize) -> Decimal {
        self.get(code).map(|r| r.value(col)).unwrap_or_default()
    }
}

/// Aliquote prestampate del quadro VE (righi VE20-VE23).
const VE_RATES: [(&str, u32); 4] = [("VE20", 4), ("VE21", 5), ("VE22", 10), ("VE23", 22)];
/// Aliquote e percentuali di compensazione prestampate del quadro VF (righi VF1-VF13), in decimi.
const VF_RATES: [(&str, u32); 13] = [("VF1", 20), ("VF2", 40), ("VF3", 50), ("VF4", 64), ("VF5", 70), ("VF6", 73), ("VF7", 75), ("VF8", 83), ("VF9", 85), ("VF10", 88), ("VF11", 100), ("VF12", 123), ("VF13", 220)];

/// Regioni del quadro VT (righi VT2-VT22) e province che vi appartengono.
pub const REGIONS: [(&str, &str, &[&str]); 21] = [
    ("VT2", "Abruzzo", &["AQ", "CH", "PE", "TE"]),
    ("VT3", "Basilicata", &["MT", "PZ"]),
    ("VT4", "Bolzano", &["BZ"]),
    ("VT5", "Calabria", &["CS", "CZ", "KR", "RC", "VV"]),
    ("VT6", "Campania", &["AV", "BN", "CE", "NA", "SA"]),
    ("VT7", "Emilia Romagna", &["BO", "FC", "FE", "MO", "PC", "PR", "RA", "RE", "RN"]),
    ("VT8", "Friuli Venezia Giulia", &["GO", "PN", "TS", "UD"]),
    ("VT9", "Lazio", &["FR", "LT", "RI", "RM", "VT"]),
    ("VT10", "Liguria", &["GE", "IM", "SP", "SV"]),
    ("VT11", "Lombardia", &["BG", "BS", "CO", "CR", "LC", "LO", "MB", "MI", "MN", "PV", "SO", "VA"]),
    ("VT12", "Marche", &["AN", "AP", "FM", "MC", "PU"]),
    ("VT13", "Molise", &["CB", "IS"]),
    ("VT14", "Piemonte", &["AL", "AT", "BI", "CN", "NO", "TO", "VB", "VC"]),
    ("VT15", "Puglia", &["BA", "BR", "BT", "FG", "LE", "TA"]),
    ("VT16", "Sardegna", &["CA", "NU", "OR", "SS", "SU", "CI", "VS", "OG", "OT"]),
    ("VT17", "Sicilia", &["AG", "CL", "CT", "EN", "ME", "PA", "RG", "SR", "TP"]),
    ("VT18", "Toscana", &["AR", "FI", "GR", "LI", "LU", "MS", "PI", "PO", "PT", "SI"]),
    ("VT19", "Trento", &["TN"]),
    ("VT20", "Umbria", &["PG", "TR"]),
    ("VT21", "Valle d'Aosta", &["AO"]),
    ("VT22", "Veneto", &["BL", "PD", "RO", "TV", "VE", "VI", "VR"]),
];

pub fn region_of(province: &str) -> Option<usize> {
    let p = province.trim().to_uppercase();
    REGIONS.iter().position(|(_, _, ps)| ps.contains(&p.as_str()))
}

fn rate_tenths(r: Decimal) -> Option<u32> {
    let t = (r * Decimal::TEN).round();
    if (r * Decimal::TEN - t).abs() > Decimal::new(1, 6) {
        return None;
    }
    t.to_string().parse().ok()
}

/// Tipi di autofattura che non entrano nella liquidazione (TD20-TD23, TD29).
fn excluded_fe(t: &str) -> bool {
    matches!(t, "TD20" | "TD21" | "TD22" | "TD23" | "TD29")
}

impl Store {
    /// Prospetto della dichiarazione IVA annuale per l'anno indicato.
    pub fn annual_vat(&self, year: i32) -> Result<AnnualVat> {
        let from = NaiveDate::from_ymd_opt(year, 1, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(year, 12, 31).unwrap();
        let mut warn: Vec<String> = Vec::new();
        let sales = self.vat_register(Register::Sales, from, to)?;
        let purchases = self.vat_register(Register::Purchases, from, to)?;
        let retail = self.retail_register(from, to)?;
        let company = self.company()?;
        let home_region = region_of(&company.province);

        // ------------------------------------------------------------- VE
        let mut ve_rate: BTreeMap<u32, (Decimal, Decimal)> = BTreeMap::new();
        let (mut ve31, mut ve32, mut ve33, mut ve38_split) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
        let mut ve35 = [Decimal::ZERO; 9];
        let mut deferred = false;
        // VT: (consumatori finali, soggetti IVA) imponibile e imposta, regioni dei consumatori
        let mut vt_cons = (Decimal::ZERO, Decimal::ZERO);
        let mut vt_iva = (Decimal::ZERO, Decimal::ZERO);
        let mut vt_reg = [(Decimal::ZERO, Decimal::ZERO); 21];
        let mut unknown_region = Decimal::ZERO;
        let mut other_natures: BTreeMap<String, Decimal> = BTreeMap::new();
        let parties_by_doc = |s: &Store, r: &RegisterRow| -> (bool, String) {
            match s.document(r.document_id) {
                Ok(d) => (d.party.vat_number.trim().is_empty(), d.party.province.clone()),
                Err(_) => (r.party_vat.trim().is_empty(), String::new()),
            }
        };
        for r in sales.iter().filter(|r| !r.reverse_charge && !excluded_fe(&r.fe_type)) {
            let (consumer, province) = parties_by_doc(self, r);
            for a in &r.rates {
                match a.natura.as_deref() {
                    None => {
                        if a.liability == "S" {
                            ve38_split += a.taxable;
                            continue;
                        }
                        if a.liability == "D" {
                            deferred = true;
                        }
                        match rate_tenths(a.rate).filter(|t| VE_RATES.iter().any(|(_, r)| r * 10 == *t)) {
                            Some(t) => {
                                let e = ve_rate.entry(t).or_default();
                                e.0 += a.taxable;
                                e.1 += a.tax;
                            }
                            None => warn.push(format!("Aliquota {}% non prevista nel quadro VE ({} {})", a.rate.normalize(), r.kind_label, r.number)),
                        }
                        if consumer {
                            vt_cons.0 += a.taxable;
                            vt_cons.1 += a.tax;
                            match region_of(&province).or(home_region) {
                                Some(i) => {
                                    vt_reg[i].0 += a.taxable;
                                    vt_reg[i].1 += a.tax;
                                }
                                None => unknown_region += a.taxable,
                            }
                        } else {
                            vt_iva.0 += a.taxable;
                            vt_iva.1 += a.tax;
                        }
                    }
                    Some("N3.5") => ve31 += a.taxable,
                    Some(n) if n.starts_with("N3") => ve32 += a.taxable,
                    Some("N4") => ve33 += a.taxable,
                    Some(n) if n.starts_with("N6") => {
                        ve35[0] += a.taxable;
                        let sub = match n {
                            "N6.1" => 1,
                            "N6.2" => 2,
                            "N6.3" => 3,
                            "N6.4" => 4,
                            "N6.5" => 5,
                            "N6.6" => 6,
                            "N6.7" => 7,
                            "N6.8" => 8,
                            _ => 0,
                        };
                        if sub > 0 {
                            ve35[sub] += a.taxable;
                        }
                    }
                    Some(n) => *other_natures.entry(n.to_string()).or_default() += a.taxable,
                }
            }
        }
        // corrispettivi del banco: consumatori finali, nella regione dell'azienda
        for d in &retail {
            for (label, taxable, tax) in &d.rates {
                let rate = label.trim_end_matches('%').replace(',', ".").parse::<Decimal>().ok();
                match rate.and_then(rate_tenths).filter(|t| VE_RATES.iter().any(|(_, r)| r * 10 == *t)) {
                    Some(t) => {
                        let e = ve_rate.entry(t).or_default();
                        e.0 += *taxable;
                        e.1 += *tax;
                        vt_cons.0 += *taxable;
                        vt_cons.1 += *tax;
                        match home_region {
                            Some(i) => {
                                vt_reg[i].0 += *taxable;
                                vt_reg[i].1 += *tax;
                            }
                            None => unknown_region += *taxable,
                        }
                    }
                    None if label == "N4" => ve33 += *taxable,
                    None => *other_natures.entry(label.clone()).or_default() += *taxable,
                }
            }
        }
        let mut ve = Quadro { code: "VE".into(), title: "Determinazione del volume d'affari e dell'imposta".into(), righi: vec![] };
        let (mut ve24_t, mut ve24_tax_calc, mut ve24_tax_reg) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
        for (code, rate) in VE_RATES {
            let (t, tax) = ve_rate.get(&(rate * 10)).copied().unwrap_or_default();
            let calc = round2(pct_of(t, Decimal::from(rate)));
            ve24_t += t;
            ve24_tax_calc += calc;
            ve24_tax_reg += tax;
            ve.righi.push(Rigo::new(code, &format!("Operazioni imponibili al {}%", rate), &[("Imponibile", round2(t)), ("Imposta", calc)]));
        }
        let ve25 = ve24_tax_reg - ve24_tax_calc;
        ve.righi.push(Rigo::new("VE24", "Totale (VE20-VE23)", &[("Imponibile", round2(ve24_t)), ("Imposta", ve24_tax_calc)]));
        ve.righi.push(Rigo::new("VE25", "Variazioni e arrotondamenti d'imposta", &[("Imposta", ve25)]));
        let ve26 = ve24_tax_calc + ve25;
        ve.righi.push(Rigo::new("VE26", "Totale imposta (VE24 + VE25)", &[("Imposta", ve26)]));
        ve.righi.push(Rigo::new("VE31", "Operazioni non imponibili a seguito di dichiarazione d'intento", &[("Imponibile", round2(ve31))]));
        ve.righi.push(Rigo::new("VE32", "Altre operazioni non imponibili", &[("Imponibile", round2(ve32))]));
        ve.righi.push(Rigo::new("VE33", "Operazioni esenti", &[("Imponibile", round2(ve33))]));
        let labels35 = ["Totale", "Rottami (campo 2)", "Oro e argento (campo 3)", "Subappalto edile (campo 4)", "Fabbricati (campo 5)", "Telefoni cellulari (campo 6)", "Prodotti elettronici (campo 7)", "Servizi edili connessi (campo 8)", "Settore energetico (campo 9)"];
        let v35: Vec<(&str, Decimal)> = labels35.iter().zip(ve35.iter()).map(|(l, v)| (*l, round2(*v))).collect();
        ve.righi.push(Rigo::new("VE35", "Operazioni con inversione contabile (reverse charge)", &v35));
        ve.righi.push(Rigo::new("VE38", "Operazioni verso la PA con scissione dei pagamenti (campo 1)", &[("Imponibile", round2(ve38_split))]));
        ve.righi.push(Rigo::new("VE39", "(meno) Operazioni di anni precedenti con imposta esigibile nell'anno", &[("Imponibile", Decimal::ZERO)]));
        ve.righi.push(Rigo::new("VE40", "(meno) Cessioni di beni ammortizzabili e passaggi interni", &[("Imponibile", Decimal::ZERO)]));
        let ve50 = round2(ve24_t + ve31 + ve32 + ve33 + ve35[0] + ve38_split);
        ve.righi.push(Rigo::new("VE50", "Volume d'affari", &[("Importo", ve50)]));
        if deferred {
            warn.push("Ci sono operazioni a esigibilità differita: verificare i righi VE37 e VE39 (il prospetto le comprende nei righi VE20-VE23)".into());
        }
        for (n, v) in &other_natures {
            warn.push(format!("Operazioni con natura {} (€ {}) non comprese nel volume d'affari: verificarne la collocazione", n, openfatt_core::money::fmt_it(*v, 2)));
        }
        warn.push("VE40: le cessioni di beni ammortizzabili non sono distinte dal gestionale e vanno indicate a mano".into());

        // ------------------------------------------------------------- VF e VJ
        let mut vf_rate: BTreeMap<u32, (Decimal, Decimal)> = BTreeMap::new();
        let (mut vf18_1, mut vf18_2, mut vf19) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
        // VJ: (codice, descrizione) -> imponibile, imposta
        let mut vj: BTreeMap<&'static str, (Decimal, Decimal)> = BTreeMap::new();
        let mut add_vf = |rate: Decimal, t: Decimal, tax: Decimal, warn: &mut Vec<String>, who: &str| match rate_tenths(rate).filter(|x| VF_RATES.iter().any(|(_, r)| r == x)) {
            Some(x) => {
                let e = vf_rate.entry(x).or_default();
                e.0 += t;
                e.1 += tax;
            }
            None => warn.push(format!("Aliquota {}% non prevista nel quadro VF ({})", rate.normalize(), who)),
        };
        for r in purchases.iter().filter(|r| !excluded_fe(&r.fe_type)) {
            let who = format!("{} {}", r.kind_label, r.number);
            if r.reverse_charge {
                // integrazione: imposta a debito (VJ) e a credito (VF)
                let code = match r.fe_type.as_str() {
                    "TD17" | "TD19" => "VJ3",
                    "TD18" => "VJ9",
                    "TD28" => "VJ9b",
                    _ => "VJ6-17",
                };
                for a in r.rates.iter().filter(|a| a.natura.is_none()) {
                    let e = vj.entry(code).or_default();
                    e.0 += a.taxable;
                    e.1 += a.tax;
                    add_vf(a.rate, a.taxable, a.tax, &mut warn, &who);
                }
                continue;
            }
            for a in &r.rates {
                match a.natura.as_deref() {
                    None => add_vf(a.rate, a.taxable, a.tax, &mut warn, &who),
                    // la fattura del fornitore in reverse charge è integrata dall'autofattura
                    Some(n) if n.starts_with("N6") => {}
                    Some("N4") => vf18_2 += a.taxable,
                    Some("N2.2") => vf19 += a.taxable,
                    Some(_) => vf18_1 += a.taxable,
                }
            }
        }
        let mut vf = Quadro { code: "VF".into(), title: "Operazioni passive e IVA ammessa in detrazione".into(), righi: vec![] };
        let (mut vf25_t, mut vf_calc, mut vf_reg) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
        for (code, tenths) in VF_RATES {
            let (t, tax) = vf_rate.get(&tenths).copied().unwrap_or_default();
            let rate = Decimal::from(tenths) / Decimal::TEN;
            let calc = round2(pct_of(t, rate));
            vf25_t += t;
            vf_calc += calc;
            vf_reg += tax;
            if !t.is_zero() || tenths == 40 || tenths == 50 || tenths == 100 || tenths == 220 {
                vf.righi.push(Rigo::new(code, &format!("Acquisti imponibili al {}%", rate.normalize().to_string().replace('.', ",")), &[("Imponibile", round2(t)), ("Imposta", calc)]));
            }
        }
        vf.righi.push(Rigo::new("VF18", "Acquisti non imponibili, non soggetti (campo 1) ed esenti (campo 2)", &[("Campo 1", round2(vf18_1)), ("Campo 2", round2(vf18_2))]));
        vf.righi.push(Rigo::new("VF19", "Acquisti da contribuenti in regime forfettario o di vantaggio (natura N2.2)", &[("Imponibile", round2(vf19))]));
        vf25_t += vf18_1 + vf18_2 + vf19;
        vf.righi.push(Rigo::new("VF25", "Totale acquisti", &[("Imponibile", round2(vf25_t)), ("Imposta", vf_calc)]));
        let vf26 = vf_reg - vf_calc;
        vf.righi.push(Rigo::new("VF26", "Variazioni e arrotondamenti d'imposta", &[("Imposta", vf26)]));
        let vf27 = vf_calc + vf26;
        vf.righi.push(Rigo::new("VF27", "Totale imposta sugli acquisti imponibili", &[("Imposta", vf27)]));
        vf.righi.push(Rigo::new("VF71", "IVA ammessa in detrazione", &[("Imposta", vf27)]));
        warn.push("VF21-VF22 (acquisti con detrazione esclusa o ridotta), VF29 (ripartizione degli acquisti) e pro-rata non sono gestiti: il prospetto considera detraibile tutta l'IVA registrata".into());

        let mut vjq = Quadro { code: "VJ".into(), title: "Imposta relativa a particolari tipologie di operazioni".into(), righi: vec![] };
        let mut vj19 = Decimal::ZERO;
        for (code, label) in [
            ("VJ3", "Acquisti da soggetti non residenti (art. 17, comma 2): TD17 e TD19"),
            ("VJ6-17", "Acquisti interni in reverse charge (TD16): da ripartire tra i righi VJ6-VJ17 secondo il tipo di operazione"),
            ("VJ9", "Acquisti intracomunitari di beni (TD18): rigo del modello IVA ordinario"),
            ("VJ9b", "Acquisti da San Marino (TD28): rigo del modello IVA ordinario"),
        ] {
            if let Some((t, tax)) = vj.get(code) {
                vj19 += *tax;
                vjq.righi.push(Rigo::new(code, label, &[("Imponibile", round2(*t)), ("Imposta", round2(*tax))]));
            }
        }
        vjq.righi.push(Rigo::new("VJ19", "Totale imposta", &[("Imposta", round2(vj19))]));

        // ------------------------------------------------------------- VL
        let liqs = self.vat_liquidations(year, if self.vat_settings()?.periodicity == openfatt_core::vat::Periodicity::Monthly { 12 } else { 4 })?;
        let periodic_due: Decimal = liqs.iter().map(|(_, l)| l.to_pay).sum();
        let interest: Decimal = liqs.iter().map(|(_, l)| l.interest).sum();
        let paid: Decimal = -self
            .cash_entries(Some(from), Some(NaiveDate::from_ymd_opt(year + 1, 1, 31).unwrap()), None)?
            .iter()
            .filter(|e| e.description.starts_with("Versamento IVA") && e.description.contains(&year.to_string()))
            .map(|e| e.amount)
            .sum::<Decimal>();
        let prev_credit = self.vat_settings()?.prev_year_credit;
        let vl1 = round2(ve26 + vj19);
        let vl2 = round2(vf27);
        let mut vl = Quadro { code: "VL".into(), title: "Liquidazione dell'imposta annuale".into(), righi: vec![] };
        vl.righi.push(Rigo::new("VL1", "IVA a debito (VE26 + VJ19)", &[("Debiti", vl1)]));
        vl.righi.push(Rigo::new("VL2", "IVA detraibile (VF71)", &[("Crediti", vl2)]));
        if vl1 >= vl2 {
            vl.righi.push(Rigo::new("VL3", "Imposta dovuta (VL1 - VL2)", &[("Debiti", vl1 - vl2)]));
        } else {
            vl.righi.push(Rigo::new("VL4", "Imposta a credito (VL2 - VL1)", &[("Crediti", vl2 - vl1)]));
        }
        vl.righi.push(Rigo::new("VL8", "Credito risultante dalla dichiarazione precedente", &[("Crediti", prev_credit)]));
        vl.righi.push(Rigo::new("VL30", "IVA periodica dovuta (campo 2) e versata (campo 3)", &[("Dovuta", round2(periodic_due)), ("Versata", round2(paid))]));
        vl.righi.push(Rigo::new("VL36", "Interessi dovuti per le liquidazioni trimestrali", &[("Debiti", round2(interest))]));
        let balance = round2(vl1 - vl2 - prev_credit - paid + interest);
        vl.righi.push(Rigo::new(if balance >= Decimal::ZERO { "VL38" } else { "VL39" }, if balance >= Decimal::ZERO { "Stima del saldo IVA a debito (da verificare)" } else { "Stima del saldo IVA a credito (da verificare)" }, &[("Importo", balance.abs())]));
        if !paid.is_zero() && (paid - periodic_due).abs() > Decimal::ONE {
            warn.push(format!("I versamenti periodici registrati in prima nota (€ {}) non coincidono con l'IVA periodica dovuta (€ {})", openfatt_core::money::fmt_it(paid, 2), openfatt_core::money::fmt_it(periodic_due, 2)));
        }

        // ------------------------------------------------------------- VT
        let mut vt = Quadro { code: "VT".into(), title: "Operazioni verso consumatori finali e soggetti IVA".into(), righi: vec![] };
        vt.righi.push(Rigo::new(
            "VT1",
            "Totale operazioni imponibili, verso consumatori finali e verso soggetti IVA",
            &[("Imponibile totale", round2(vt_cons.0 + vt_iva.0)), ("Imposta totale", round2(vt_cons.1 + vt_iva.1)), ("Imponibile consumatori", round2(vt_cons.0)), ("Imposta consumatori", round2(vt_cons.1)), ("Imponibile soggetti IVA", round2(vt_iva.0)), ("Imposta soggetti IVA", round2(vt_iva.1))],
        ));
        for (i, (code, name, _)) in REGIONS.iter().enumerate() {
            let (t, tax) = vt_reg[i];
            if !t.is_zero() || !tax.is_zero() {
                vt.righi.push(Rigo::new(code, &format!("{}: operazioni verso consumatori finali", name), &[("Imponibile", round2(t)), ("Imposta", round2(tax))]));
            }
        }
        if !unknown_region.is_zero() {
            warn.push(format!("Operazioni verso consumatori finali senza provincia (€ {}): indicare la regione nel quadro VT", openfatt_core::money::fmt_it(unknown_region, 2)));
        }
        Ok(AnnualVat { year, quadri: vec![ve, vf, vjq, vl, vt], warnings: warn })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;
    use openfatt_core::model::*;

    #[test]
    fn annual_return_from_registers() {
        let s = Store::open_in_memory().unwrap();
        let mut c = s.company().unwrap();
        c.province = "TN".into();
        c.vat_number = "12345678903".into();
        s.set_company(&c).unwrap();
        let vats = s.vat_codes().unwrap();
        let v22 = vats.iter().find(|v| v.rate == dec!(22)).unwrap().clone();
        let v10 = vats.iter().find(|v| v.rate == dec!(10)).unwrap().clone();
        let n35 = vats.iter().find(|v| v.natura.as_deref() == Some("N3.5")).unwrap().clone();
        let impresa = s.save_party(&Party { name: "Impresa".into(), vat_number: "01234567890".into(), province: "MI".into(), ..Default::default() }).unwrap();
        let privato = s.save_party(&Party { first_name: "Anna".into(), last_name: "Neri".into(), tax_code: "NRENNA80A41L378X".into(), province: "RM".into(), ..Default::default() }).unwrap();
        let fornitore = s.save_party(&Party { name: "Fornitore".into(), role: PartyRole::Supplier, vat_number: "09876543210".into(), ..Default::default() }).unwrap();
        let mk = |kind: DocKind, party: &Party, price: Decimal, v: &VatCode| {
            let mut d = Document::new(kind);
            d.date = NaiveDate::from_ymd_opt(2025, 3, 10).unwrap();
            d.party_id = Some(party.id);
            if kind.is_purchase() {
                d.external_number = "1".into();
            }
            d.stamp.mode = StampMode::No;
            d.lines.push(DocLine { description: "x".into(), unit_price: price, vat: VatRef::from_code(v), ..Default::default() });
            let d = s.save_document(&d).unwrap();
            s.issue_document(d.id).unwrap();
        };
        mk(DocKind::Invoice, &impresa, dec!(1000), &v22);
        mk(DocKind::Invoice, &privato, dec!(100), &v10);
        mk(DocKind::Invoice, &impresa, dec!(500), &n35);
        mk(DocKind::CreditNote, &impresa, dec!(200), &v22);
        mk(DocKind::PurchaseInvoice, &fornitore, dec!(300), &v22);
        let a = s.annual_vat(2025).unwrap();
        assert_eq!(a.value("VE23", 0), dec!(800));
        assert_eq!(a.value("VE23", 1), dec!(176));
        assert_eq!(a.value("VE22", 0), dec!(100));
        assert_eq!(a.value("VE31", 0), dec!(500));
        assert_eq!(a.value("VE26", 0), dec!(186));
        assert_eq!(a.value("VE50", 0), dec!(1400));
        assert_eq!(a.value("VF13", 0), dec!(300));
        assert_eq!(a.value("VF71", 0), dec!(66));
        assert_eq!(a.value("VL1", 0), dec!(186));
        assert_eq!(a.value("VL3", 0), dec!(120));
        // VT: il privato di Roma è un consumatore finale del Lazio
        assert_eq!(a.value("VT1", 2), dec!(100));
        assert_eq!(a.value("VT1", 4), dec!(800));
        assert_eq!(a.value("VT9", 0), dec!(100));
        assert_eq!(region_of("bz"), Some(2));
    }
}
