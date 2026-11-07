//! Messaggi XML CBI per bonifici SEPA (CBIPaymentRequest.00.04.01) e addebiti diretti
//! SEPA (CBISDDReqLogMsg.00.01.01), da caricare nell'home banking.

use std::collections::BTreeMap;

use chrono::{NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;

use crate::{creditor_id_valid, it_bank_codes, normalize_iban, sepa_text, Company, Problems};

pub const SCT_NAMESPACE: &str = "urn:CBI:xsd:CBIPaymentRequest.00.04.01";
pub const SDD_NAMESPACE: &str = "urn:CBI:xsd:CBISDDReqLogMsg.00.01.01";

/// Bonifico a un beneficiario.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SctItem {
    pub name: String,
    pub iban: String,
    pub amount: Decimal,
    /// Riferimento univoco end-to-end (max 35).
    pub end_to_end: String,
    /// Causale (max 140).
    pub remittance: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum SddScheme {
    /// Consumatori e imprese.
    #[default]
    Core,
    /// Solo imprese (nessun diritto di rimborso senza motivo).
    B2b,
}

impl SddScheme {
    pub fn code(self) -> &'static str {
        match self {
            SddScheme::Core => "CORE",
            SddScheme::B2b => "B2B",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum SeqType {
    /// Primo addebito di un mandato ricorrente.
    First,
    /// Addebiti successivi.
    #[default]
    Recurring,
    /// Ultimo addebito.
    Final,
    /// Addebito singolo (mandato non ricorrente).
    OneOff,
}

impl SeqType {
    pub fn code(self) -> &'static str {
        match self {
            SeqType::First => "FRST",
            SeqType::Recurring => "RCUR",
            SeqType::Final => "FNAL",
            SeqType::OneOff => "OOFF",
        }
    }
}

/// Addebito diretto su un debitore.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SddItem {
    pub name: String,
    pub iban: String,
    pub amount: Decimal,
    pub collection: NaiveDate,
    pub mandate_id: String,
    pub mandate_date: NaiveDate,
    pub scheme: SddScheme,
    pub seq: SeqType,
    pub end_to_end: String,
    pub remittance: String,
}

struct Xml {
    s: String,
    depth: usize,
}

impl Xml {
    fn new() -> Self {
        Xml { s: String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"), depth: 0 }
    }
    fn open(&mut self, tag: &str) {
        self.s.push_str(&format!("{}<{}>\n", "  ".repeat(self.depth), tag));
        self.depth += 1;
    }
    fn open_ns(&mut self, tag: &str, ns: &str) {
        self.s.push_str(&format!("<{} xmlns=\"{}\">\n", tag, ns));
        self.depth += 1;
    }
    fn close(&mut self, tag: &str) {
        self.depth -= 1;
        self.s.push_str(&format!("{}</{}>\n", "  ".repeat(self.depth), tag));
    }
    fn el(&mut self, tag: &str, v: &str) {
        self.s.push_str(&format!("{}<{}>{}</{}>\n", "  ".repeat(self.depth), tag, esc(v), tag));
    }
    fn amount(&mut self, tag: &str, v: Decimal) {
        self.s.push_str(&format!("{}<{} Ccy=\"EUR\">{}</{}>\n", "  ".repeat(self.depth), tag, amount(v), tag));
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn amount(d: Decimal) -> String {
    format!("{:.2}", d.round_dp(2))
}

fn initiating_party(x: &mut Xml, c: &Company) {
    x.open("InitgPty");
    x.el("Nm", &sepa_text(&c.name, 70));
    x.open("Id");
    x.open("OrgId");
    x.open("Othr");
    x.el("Id", c.cuc.trim());
    x.el("Issr", "CBI");
    x.close("Othr");
    x.close("OrgId");
    x.close("Id");
    x.close("InitgPty");
}

fn check_company(c: &Company, p: &mut Problems) -> Option<String> {
    if c.cuc.trim().is_empty() || c.cuc.trim().len() > 35 {
        p.push("Codice CUC dell'azienda mancante (lo comunica la banca)".into());
    }
    if c.name.trim().is_empty() {
        p.push("Ragione sociale dell'azienda mancante".into());
    }
    match it_bank_codes(&c.iban) {
        Some((abi, _, _)) => Some(abi),
        None => {
            p.push("IBAN italiano dell'azienda mancante o non valido".into());
            None
        }
    }
}

fn check_item_iban(name: &str, iban: &str, p: &mut Problems) {
    if !openfatt_core::fiscal::iban_valido(iban) {
        p.push(format!("{}: IBAN mancante o non valido", if name.trim().is_empty() { "(senza nome)" } else { name.trim() }));
    }
}

fn check_ids(items: impl Iterator<Item = (String, String)>, p: &mut Problems) {
    let mut seen = std::collections::BTreeSet::new();
    for (name, e2e) in items {
        if e2e.trim().is_empty() || e2e.len() > 35 {
            p.push(format!("{}: riferimento end-to-end mancante o oltre 35 caratteri", name));
        } else if !seen.insert(e2e) {
            p.push(format!("{}: riferimento end-to-end ripetuto", name));
        }
    }
}

/// Distinta di bonifici SEPA dal conto dell'azienda, tutti con la stessa data di esecuzione.
pub fn sct_cbi(c: &Company, msg_id: &str, created: NaiveDateTime, execution: NaiveDate, items: &[SctItem]) -> Result<String, Problems> {
    let mut p = Vec::new();
    let abi = check_company(c, &mut p);
    if items.is_empty() {
        p.push("Nessun bonifico da disporre".into());
    }
    for it in items {
        check_item_iban(&it.name, &it.iban, &mut p);
        if it.amount <= Decimal::ZERO {
            p.push(format!("{}: importo non positivo", it.name));
        }
        if sepa_text(&it.name, 70).is_empty() {
            p.push("Beneficiario senza nome".into());
        }
    }
    check_ids(items.iter().map(|i| (i.name.clone(), i.end_to_end.clone())), &mut p);
    if !p.is_empty() {
        return Err(p);
    }
    let total: Decimal = items.iter().map(|i| i.amount.round_dp(2)).sum();
    let mut x = Xml::new();
    x.open_ns("CBIPaymentRequest", SCT_NAMESPACE);
    x.open("GrpHdr");
    x.el("MsgId", &sepa_text(msg_id, 35));
    x.el("CreDtTm", &created.format("%Y-%m-%dT%H:%M:%S").to_string());
    x.el("NbOfTxs", &items.len().to_string());
    x.el("CtrlSum", &amount(total));
    initiating_party(&mut x, c);
    x.close("GrpHdr");
    x.open("PmtInf");
    x.el("PmtInfId", &sepa_text(msg_id, 35));
    x.el("PmtMtd", "TRF");
    x.open("PmtTpInf");
    x.el("InstrPrty", "NORM");
    x.open("SvcLvl");
    x.el("Cd", "SEPA");
    x.close("SvcLvl");
    x.close("PmtTpInf");
    x.open("ReqdExctnDt");
    x.el("Dt", &execution.format("%Y-%m-%d").to_string());
    x.close("ReqdExctnDt");
    x.open("Dbtr");
    x.el("Nm", &sepa_text(&c.name, 70));
    x.close("Dbtr");
    x.open("DbtrAcct");
    x.open("Id");
    x.el("IBAN", &normalize_iban(&c.iban));
    x.close("Id");
    x.close("DbtrAcct");
    x.open("DbtrAgt");
    x.open("FinInstnId");
    x.open("ClrSysMmbId");
    x.el("MmbId", &abi.unwrap());
    x.close("ClrSysMmbId");
    x.close("FinInstnId");
    x.close("DbtrAgt");
    x.el("ChrgBr", "SLEV");
    for (i, it) in items.iter().enumerate() {
        x.open("CdtTrfTxInf");
        x.open("PmtId");
        x.el("InstrId", &(i + 1).to_string());
        x.el("EndToEndId", &sepa_text(&it.end_to_end, 35));
        x.close("PmtId");
        x.open("PmtTpInf");
        x.open("CtgyPurp");
        x.el("Cd", "SUPP");
        x.close("CtgyPurp");
        x.close("PmtTpInf");
        x.open("Amt");
        x.amount("InstdAmt", it.amount);
        x.close("Amt");
        x.open("Cdtr");
        x.el("Nm", &sepa_text(&it.name, 70));
        x.close("Cdtr");
        x.open("CdtrAcct");
        x.open("Id");
        x.el("IBAN", &normalize_iban(&it.iban));
        x.close("Id");
        x.close("CdtrAcct");
        let r = sepa_text(&it.remittance, 140);
        if !r.is_empty() {
            x.open("RmtInf");
            x.el("Ustrd", &r);
            x.close("RmtInf");
        }
        x.close("CdtTrfTxInf");
    }
    x.close("PmtInf");
    x.close("CBIPaymentRequest");
    Ok(x.s)
}

/// Distinta di addebiti diretti SEPA. Le disposizioni sono raggruppate per schema
/// (Core/B2B), tipo di sequenza e data di addebito, come richiesto dal messaggio.
pub fn sdd_cbi(c: &Company, msg_id: &str, created: NaiveDateTime, items: &[SddItem]) -> Result<String, Problems> {
    let mut p = Vec::new();
    let abi = check_company(c, &mut p);
    if !creditor_id_valid(&c.creditor_id) {
        p.push("Identificativo del creditore SEPA (CID) mancante o non valido (lo rilascia la banca)".into());
    }
    if items.is_empty() {
        p.push("Nessun addebito da presentare".into());
    }
    for it in items {
        check_item_iban(&it.name, &it.iban, &mut p);
        if it.amount <= Decimal::ZERO {
            p.push(format!("{}: importo non positivo", it.name));
        }
        if it.mandate_id.trim().is_empty() || it.mandate_id.trim().len() > 35 {
            p.push(format!("{}: riferimento del mandato mancante", it.name));
        }
        if it.mandate_date > it.collection {
            p.push(format!("{}: il mandato è firmato dopo la data di addebito", it.name));
        }
    }
    check_ids(items.iter().map(|i| (i.name.clone(), i.end_to_end.clone())), &mut p);
    if !p.is_empty() {
        return Err(p);
    }
    let abi = abi.unwrap();
    let mut groups: BTreeMap<(SddScheme, SeqType, NaiveDate), Vec<&SddItem>> = BTreeMap::new();
    for it in items {
        groups.entry((it.scheme, it.seq, it.collection)).or_default().push(it);
    }
    let total: Decimal = items.iter().map(|i| i.amount.round_dp(2)).sum();
    let mut x = Xml::new();
    x.open_ns("CBISDDReqLogMsg", SDD_NAMESPACE);
    x.open("GrpHdr");
    x.el("MsgId", &sepa_text(msg_id, 35));
    x.el("CreDtTm", &created.format("%Y-%m-%dT%H:%M:%S").to_string());
    x.el("NbOfTxs", &items.len().to_string());
    x.el("CtrlSum", &amount(total));
    initiating_party(&mut x, c);
    x.close("GrpHdr");
    let mut n = 0;
    for (g, ((scheme, seq, date), list)) in groups.iter().enumerate() {
        x.open("PmtInf");
        x.el("PmtInfId", &format!("{}-{}", sepa_text(msg_id, 30), g + 1));
        x.el("PmtMtd", "DD");
        x.open("PmtTpInf");
        x.open("SvcLvl");
        x.el("Cd", "SEPA");
        x.close("SvcLvl");
        x.open("LclInstrm");
        x.el("Cd", scheme.code());
        x.close("LclInstrm");
        x.el("SeqTp", seq.code());
        x.close("PmtTpInf");
        x.el("ReqdColltnDt", &date.format("%Y-%m-%d").to_string());
        x.open("Cdtr");
        x.el("Nm", &sepa_text(&c.name, 70));
        x.close("Cdtr");
        x.open("CdtrAcct");
        x.open("Id");
        x.el("IBAN", &normalize_iban(&c.iban));
        x.close("Id");
        x.close("CdtrAcct");
        x.open("CdtrAgt");
        x.open("FinInstnId");
        x.open("ClrSysMmbId");
        x.el("MmbId", &abi);
        x.close("ClrSysMmbId");
        x.close("FinInstnId");
        x.close("CdtrAgt");
        x.el("ChrgBr", "SLEV");
        x.open("CdtrSchmeId");
        x.open("Id");
        x.open("PrvtId");
        x.open("Othr");
        x.el("Id", &normalize_iban(&c.creditor_id));
        x.open("SchmeNm");
        x.el("Prtry", "SEPA");
        x.close("SchmeNm");
        x.close("Othr");
        x.close("PrvtId");
        x.close("Id");
        x.close("CdtrSchmeId");
        for it in list {
            n += 1;
            x.open("DrctDbtTxInf");
            x.open("PmtId");
            x.el("InstrId", &n.to_string());
            x.el("EndToEndId", &sepa_text(&it.end_to_end, 35));
            x.close("PmtId");
            x.amount("InstdAmt", it.amount);
            x.open("DrctDbtTx");
            x.open("MndtRltdInf");
            x.el("MndtId", &sepa_text(&it.mandate_id, 35));
            x.el("DtOfSgntr", &it.mandate_date.format("%Y-%m-%d").to_string());
            x.close("MndtRltdInf");
            x.close("DrctDbtTx");
            x.open("Dbtr");
            x.el("Nm", &sepa_text(&it.name, 70));
            x.close("Dbtr");
            x.open("DbtrAcct");
            x.open("Id");
            x.el("IBAN", &normalize_iban(&it.iban));
            x.close("Id");
            x.close("DbtrAcct");
            let r = sepa_text(&it.remittance, 140);
            if !r.is_empty() {
                x.open("RmtInf");
                x.el("Ustrd", &r);
                x.close("RmtInf");
            }
            x.close("DrctDbtTxInf");
        }
        x.close("PmtInf");
    }
    x.close("CBISDDReqLogMsg");
    Ok(x.s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn d(y: i32, m: u32, g: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, g).unwrap()
    }

    pub(crate) fn company() -> Company {
        Company {
            name: "Bottega Demo S.r.l.".into(),
            tax_id: "01234567897".into(),
            iban: "IT60X0542811101000000123456".into(),
            cuc: "1234567X".into(),
            creditor_id: "IT91ZZZ01234567897".into(),
            ..Default::default()
        }
    }

    pub(crate) fn sct_sample() -> String {
        let items = vec![
            SctItem { name: "Forniture Industriali S.r.l.".into(), iban: "IT36A0301503200000003517230".into(), amount: dec!(1220.5), end_to_end: "SCT-1-1".into(), remittance: "Saldo fattura n. 45/A del 03/10/2026".into() },
            SctItem { name: "Caffè & Co. s.n.c.".into(), iban: "IT60 X054 2811 1010 0000 0123 456".into(), amount: dec!(80), end_to_end: "SCT-1-2".into(), remittance: String::new() },
        ];
        sct_cbi(&company(), "SCT-2026-1", d(2026, 11, 7).and_hms_opt(10, 30, 0).unwrap(), d(2026, 11, 10), &items).unwrap()
    }

    pub(crate) fn sdd_sample() -> String {
        let base = SddItem { name: "Rossi Arredamenti S.p.A.".into(), iban: "IT36A0301503200000003517230".into(), amount: dec!(100), collection: d(2026, 11, 30), mandate_id: "MAND-0001".into(), mandate_date: d(2024, 1, 15), ..Default::default() };
        let items = vec![
            SddItem { end_to_end: "SDD-1-1".into(), remittance: "Fattura n. 12 del 31/10/2026".into(), ..base.clone() },
            SddItem { end_to_end: "SDD-1-2".into(), amount: dec!(50.25), seq: SeqType::First, mandate_id: "MAND-0002".into(), ..base.clone() },
            SddItem { end_to_end: "SDD-1-3".into(), scheme: SddScheme::B2b, collection: d(2026, 12, 31), ..base.clone() },
        ];
        sdd_cbi(&company(), "SDD-2026-1", d(2026, 11, 7).and_hms_opt(10, 30, 0).unwrap(), &items).unwrap()
    }

    #[test]
    fn bonifici() {
        let x = sct_sample();
        let doc = roxmltree::Document::parse(&x).unwrap();
        let root = doc.root_element();
        assert_eq!(root.tag_name().namespace(), Some(SCT_NAMESPACE));
        let text = |n: &str| doc.descendants().find(|e| e.has_tag_name(n)).and_then(|e| e.text()).unwrap_or("").to_string();
        assert_eq!(text("NbOfTxs"), "2");
        assert_eq!(text("CtrlSum"), "1300.50");
        assert_eq!(text("MmbId"), "05428");
        assert!(x.contains("<Issr>CBI</Issr>"));
        assert!(x.contains("<InstdAmt Ccy=\"EUR\">1220.50</InstdAmt>"));
        assert!(x.contains("<Nm>Caffe + Co. s.n.c.</Nm>"));
        assert!(x.contains("<IBAN>IT60X0542811101000000123456</IBAN>"));
        assert_eq!(doc.descendants().filter(|e| e.has_tag_name("RmtInf")).count(), 1);
    }

    #[test]
    fn addebiti_raggruppati() {
        let x = sdd_sample();
        let doc = roxmltree::Document::parse(&x).unwrap();
        assert_eq!(doc.root_element().tag_name().namespace(), Some(SDD_NAMESPACE));
        // tre gruppi: CORE/RCUR/30-11, CORE/FRST/30-11, B2B/RCUR/31-12
        assert_eq!(doc.descendants().filter(|e| e.has_tag_name("PmtInf")).count(), 3);
        assert!(x.contains("<SeqTp>FRST</SeqTp>") && x.contains("<Cd>B2B</Cd>"));
        assert!(x.contains("<CtrlSum>250.25</CtrlSum>"));
        assert!(x.contains("<Prtry>SEPA</Prtry>"));
    }

    #[test]
    fn controlli() {
        let mut c = company();
        c.cuc.clear();
        c.creditor_id = "IT00ZZZ01234567897".into();
        let it = SddItem { name: "X".into(), iban: "IT00".into(), amount: dec!(0), collection: d(2026, 1, 1), mandate_date: d(2026, 2, 1), end_to_end: "A".into(), ..Default::default() };
        let e = sdd_cbi(&c, "M", d(2026, 1, 1).and_hms_opt(0, 0, 0).unwrap(), &[it.clone(), it]).unwrap_err();
        for w in ["CUC", "CID", "IBAN", "importo", "mandato mancante", "firmato dopo", "ripetuto"] {
            assert!(e.iter().any(|x| x.contains(w)), "manca {}: {:?}", w, e);
        }
    }

    /// Scrive gli esempi in target/xsd-samples per la validazione con tools/valida-xsd.ps1.
    #[test]
    fn esempi_per_validazione_xsd() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/xsd-samples/cbi");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bonifici.xml"), sct_sample()).unwrap();
        std::fs::write(dir.join("addebiti.xml"), sdd_sample()).unwrap();
    }
}
