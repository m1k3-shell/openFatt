use std::path::PathBuf;

use chrono::NaiveDate;
use openfatt_core::calc::{compute, due_dates};
use openfatt_core::dec;
use openfatt_core::model::*;

use super::*;

pub fn sample_company() -> Company {
    Company {
        name: "Officina Rossi S.r.l.".into(),
        vat_number: "12345678903".into(),
        tax_code: "12345678903".into(),
        regime_fiscale: "RF01".into(),
        address: "Via Garibaldi".into(),
        civic: "12".into(),
        zip: "38122".into(),
        city: "Trento".into(),
        province: "TN".into(),
        rea_office: "TN".into(),
        rea_number: "123456".into(),
        share_capital: Some(dec!(10000)),
        sole_shareholder: "SM".into(),
        email: "info@officinarossi.example".into(),
        bank_name: "Banca di prova".into(),
        iban: "IT60X0542811101000000123456".into(),
        ..Default::default()
    }
}

pub fn sample_customer() -> Party {
    Party {
        id: 1,
        code: "C001".into(),
        name: "Città & Servizi S.p.A.".into(),
        vat_number: "06363391001".into(),
        address: "Viale Europa".into(),
        civic: "5/B".into(),
        zip: "00144".into(),
        city: "Roma".into(),
        province: "RM".into(),
        sdi_code: "ABC1234".into(),
        ..Default::default()
    }
}

fn v22() -> VatRef {
    VatRef { vat_id: Some(1), rate: dec!(22), natura: None, legal_ref: String::new(), split: false }
}

fn samples_dir() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/xsd-samples");
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn finalize(doc: &mut Document) -> openfatt_core::calc::Totals {
    doc.status = DocStatus::Issued;
    if doc.number.is_none() {
        doc.number = Some(7);
    }
    let t = compute(doc).unwrap();
    if let Some(p) = doc.payment.clone() {
        doc.due_dates = due_dates(doc.date, t.net_to_pay, &p);
    }
    t
}

fn assert_clean(name: &str, g: &Generated) {
    let errors: Vec<String> = g.issues.iter().filter(|i| i.severity == Severity::Error).map(|i| i.to_string()).collect();
    assert!(errors.is_empty(), "{}: {:#?}\n{}", name, errors, g.xml);
    std::fs::write(samples_dir().join(format!("{}.xml", name)), &g.xml).unwrap();
}

#[test]
fn invoice_full_roundtrip() {
    let company = sample_company();
    let mut doc = Document::new(DocKind::Invoice);
    doc.date = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
    doc.party = sample_customer();
    doc.payment = Some(PaymentMethod {
        name: "Bonifico 30/60".into(),
        mp_code: "MP05".into(),
        installments: vec![
            Installment { days: 30, end_of_month: true, extra_days: 0, percent: dec!(50) },
            Installment { days: 60, end_of_month: true, extra_days: 0, percent: dec!(50) },
        ],
        show_iban: true,
        ..Default::default()
    });
    doc.causale = "Fornitura di marzo € 100 – materiale vario".into();
    doc.related.push(RelatedDoc { kind: "ORDINE".into(), number: "PO-77".into(), date: NaiveDate::from_ymd_opt(2026, 3, 1), ..Default::default() });
    doc.lines.push(DocLine { code: "ART-1".into(), description: "Bulloni zincati".into(), qty: dec!(100), unit: "pz".into(), unit_price: dec!(0.1234), discounts: "10+5".into(), vat: v22(), ..Default::default() });
    doc.lines.push(DocLine { kind: LineKind::Text, description: "Consegna al magazzino nord".into(), ..Default::default() });
    doc.lines.push(DocLine { description: "Trasporto".into(), qty: dec!(1), unit_price: dec!(25), vat: v22(), ..Default::default() });
    doc.lines.push(DocLine {
        description: "Reso imballi".into(),
        qty: dec!(-2),
        unit_price: dec!(1.5),
        vat: v22(),
        ..Default::default()
    });
    doc.lines.push(DocLine {
        description: "Servizio esente".into(),
        qty: dec!(1),
        unit_price: dec!(40),
        vat: VatRef { vat_id: None, rate: dec!(0), natura: Some("N4".into()), legal_ref: "Art. 10 DPR 633/72".into(), split: false },
        ..Default::default()
    });
    let t = finalize(&mut doc);
    let g = generate(&company, &doc, &t, &BuildOptions { progressivo: "00001".into(), ..Default::default() }).unwrap();
    assert_clean("fattura_completa", &g);

    let back = parse(&g.xml).unwrap();
    assert_eq!(back, g.fattura, "la rilettura deve restituire lo stesso modello");
    let b = &back.bodies[0];
    assert_eq!(b.linee.len(), 5);
    assert_eq!(b.linee[3].quantita, Some(dec!(2)));
    assert_eq!(b.linee[3].prezzo_unitario, dec!(-1.5));
    assert_eq!(b.dati_generali.documento.causale[0], "Fornitura di marzo EUR 100 - materiale vario");
    assert_eq!(b.pagamenti[0].condizioni, "TP01");
    assert_eq!(b.pagamenti[0].dettagli.len(), 2);
    assert!(g.xml.contains("<Denominazione>Città &amp; Servizi S.p.A.</Denominazione>"));
}

#[test]
fn forfettario_bollo_inps() {
    let mut company = sample_company();
    company.name.clear();
    company.first_name = "Mario".into();
    company.last_name = "Rossi".into();
    company.tax_code = "RSSMRA85T10A562S".into();
    company.regime_fiscale = "RF19".into();
    company.rea_office.clear();
    let mut doc = Document::new(DocKind::Invoice);
    doc.date = NaiveDate::from_ymd_opt(2026, 2, 1).unwrap();
    let mut cust = sample_customer();
    cust.name.clear();
    cust.first_name = "Lucia".into();
    cust.last_name = "Bianchi".into();
    cust.vat_number.clear();
    cust.tax_code = "RSSMRA85T10A562S".into();
    cust.sdi_code.clear();
    cust.pec = "lucia@pec.example.it".into();
    doc.party = cust;
    let n22 = VatRef { vat_id: None, rate: dec!(0), natura: Some("N2.2".into()), legal_ref: "Operazione in franchigia art. 1 c. 54-89 L. 190/2014".into(), split: false };
    doc.lines.push(DocLine { description: "Consulenza".into(), qty: dec!(1), unit_price: dec!(800), vat: n22.clone(), pension_fund: true, ..Default::default() });
    doc.pension_fund = Some(PensionFund { kind: "TC22".into(), rate: dec!(4), vat: n22, subject_to_withholding: false });
    let t = finalize(&mut doc);
    let g = generate(&company, &doc, &t, &BuildOptions { progressivo: "00002".into(), ..Default::default() }).unwrap();
    assert_clean("forfettario", &g);
    let d = &g.fattura.bodies[0].dati_generali.documento;
    assert_eq!(d.dati_bollo.as_ref().unwrap().importo_bollo, Some(dec!(2.00)));
    assert_eq!(d.importo_totale_documento, Some(dec!(834.00)));
    assert_eq!(g.fattura.header.dati_trasmissione.codice_destinatario, "0000000");
    assert_eq!(g.fattura.header.dati_trasmissione.pec_destinatario.as_deref(), Some("lucia@pec.example.it"));
    // il trasmittente persona fisica usa il codice fiscale
    assert_eq!(g.fattura.header.dati_trasmissione.id_trasmittente.codice, "RSSMRA85T10A562S");
}

#[test]
fn professional_withholding_pa_split() {
    let company = sample_company();
    let mut doc = Document::new(DocKind::Fee);
    doc.date = NaiveDate::from_ymd_opt(2026, 4, 2).unwrap();
    let mut pa = sample_customer();
    pa.is_pa = true;
    pa.sdi_code = "UFY9MH".into();
    doc.party = pa;
    doc.vat_liability = "S".into();
    doc.related.push(RelatedDoc { kind: "ORDINE".into(), number: "123".into(), cig: "Z1234567890".into(), ..Default::default() });
    doc.lines.push(DocLine { description: "Progettazione".into(), qty: dec!(1), unit_price: dec!(1000), vat: v22(), withholding: true, pension_fund: true, ..Default::default() });
    doc.pension_fund = Some(PensionFund { kind: "TC04".into(), rate: dec!(4), vat: v22(), subject_to_withholding: false });
    doc.withholdings.push(Withholding::default());
    doc.payment = Some(PaymentMethod { mp_code: "MP05".into(), installments: vec![Installment { days: 30, ..Default::default() }], show_iban: true, ..Default::default() });
    let t = finalize(&mut doc);
    assert_eq!(t.net_to_pay, dec!(840.00)); // 1040 - 200 ritenuta (IVA 228,80 versata dalla PA)
    let g = generate(&company, &doc, &t, &BuildOptions { progressivo: "00003".into(), ..Default::default() }).unwrap();
    assert_clean("parcella_pa_split", &g);
    assert_eq!(g.fattura.versione, "FPA12");
    assert_eq!(g.fattura.bodies[0].riepilogo[0].esigibilita_iva.as_deref(), Some("S"));
}

#[test]
fn credit_note_and_differed_invoice() {
    let company = sample_company();
    let mut nc = Document::new(DocKind::CreditNote);
    nc.date = NaiveDate::from_ymd_opt(2026, 5, 5).unwrap();
    nc.party = sample_customer();
    nc.related.push(RelatedDoc { kind: "FATTURA".into(), number: "7".into(), date: NaiveDate::from_ymd_opt(2026, 3, 10), ..Default::default() });
    nc.lines.push(DocLine { description: "Storno parziale".into(), qty: dec!(1), unit_price: dec!(10), vat: v22(), ..Default::default() });
    let t = finalize(&mut nc);
    let g = generate(&company, &nc, &t, &BuildOptions { progressivo: "00004".into(), ..Default::default() }).unwrap();
    assert_clean("nota_credito", &g);
    assert_eq!(g.fattura.bodies[0].dati_generali.documento.tipo_documento, "TD04");

    let mut fd = Document::new(DocKind::Invoice);
    fd.fe_type = "TD24".into();
    fd.date = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
    fd.party = sample_customer();
    fd.lines.push(DocLine { description: "Merce DDT 1".into(), qty: dec!(2), unit_price: dec!(10), vat: v22(), ..Default::default() });
    fd.lines.push(DocLine { description: "Merce DDT 2".into(), qty: dec!(3), unit_price: dec!(10), vat: v22(), ..Default::default() });
    let t = finalize(&mut fd);
    let opts = BuildOptions {
        progressivo: "00005".into(),
        ddt_refs: vec![
            DdtRef { number: "1".into(), date: NaiveDate::from_ymd_opt(2026, 5, 3), line_indexes: vec![0] },
            DdtRef { number: "2".into(), date: NaiveDate::from_ymd_opt(2026, 5, 20), line_indexes: vec![1] },
        ],
        ..Default::default()
    };
    let g = generate(&company, &fd, &t, &opts).unwrap();
    assert_clean("fattura_differita", &g);
    assert_eq!(g.fattura.bodies[0].dati_generali.ddt.len(), 2);
    assert_eq!(g.fattura.bodies[0].dati_generali.ddt[1].riferimento_numero_linea, vec![2]);
}

#[test]
fn reverse_charge_integration_td17() {
    let company = sample_company();
    let mut doc = Document::new(DocKind::SelfInvoice);
    doc.fe_type = "TD17".into();
    doc.date = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
    doc.party = Party {
        name: "Cloud Services GmbH".into(),
        vat_country: "DE".into(),
        vat_number: "DE123456789".into(),
        address: "Hauptstrasse 1".into(),
        zip: "10115".into(),
        city: "Berlin".into(),
        country: "DE".into(),
        role: PartyRole::Supplier,
        ..Default::default()
    };
    doc.related.push(RelatedDoc { kind: "FATTURA".into(), number: "INV-2026-55".into(), date: NaiveDate::from_ymd_opt(2026, 6, 1), ..Default::default() });
    doc.lines.push(DocLine { description: "Abbonamento software giugno".into(), qty: dec!(1), unit_price: dec!(100), vat: v22(), ..Default::default() });
    doc.stamp.mode = StampMode::No;
    let t = finalize(&mut doc);
    let g = generate(&company, &doc, &t, &BuildOptions { progressivo: "00006".into(), ..Default::default() }).unwrap();
    assert_clean("integrazione_td17", &g);
    assert_eq!(g.fattura.header.cedente.id_fiscale_iva.paese, "DE");
    assert_eq!(g.fattura.header.cedente.id_fiscale_iva.codice, "123456789");
    assert_eq!(g.fattura.header.soggetto_emittente.as_deref(), Some("CC"));
}

#[test]
fn validator_detects_errors() {
    let company = sample_company();
    let mut doc = Document::new(DocKind::Invoice);
    doc.party = sample_customer();
    doc.lines.push(DocLine { description: "x".into(), qty: dec!(1), unit_price: dec!(10), vat: v22(), ..Default::default() });
    let t = finalize(&mut doc);
    let mut f = build(&company, &doc, &t, &BuildOptions { progressivo: "1".into(), ..Default::default() }).unwrap();
    f.bodies[0].linee[0].prezzo_totale = dec!(11);
    f.bodies[0].riepilogo[0].imposta = dec!(3);
    f.header.cessionario.id_fiscale_iva = None;
    f.header.cessionario.codice_fiscale = None;
    let codes: Vec<String> = validate(&f).into_iter().filter(|i| i.severity == Severity::Error).map(|i| i.code).collect();
    assert!(codes.contains(&"00423".to_string()), "{:?}", codes);
    assert!(codes.contains(&"00421".to_string()), "{:?}", codes);
    assert!(codes.contains(&"00417".to_string()), "{:?}", codes);
}

fn spec(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs").join(path)
}

#[test]
fn official_examples_parse_and_validate() {
    for n in ["FPR01", "FPR02", "FPR03", "FPA01", "FPA02", "FPA03"] {
        let p = spec(&format!("esempi/IT01234567890_{}.xml", n));
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let f = parse_bytes(&bytes).unwrap_or_else(|e| panic!("{}: {}", n, e));
        assert!(!f.bodies.is_empty());
        let issues = validate(&f);
        // gli esempi ufficiali usano partite IVA fittizie: si ignorano i controlli di forma su di esse
        let serious: Vec<String> = issues
            .iter()
            .filter(|i| i.severity == Severity::Error && !["00300", "00301", "00305", "00306", "00302", "GA-PRG"].contains(&i.code.as_str()))
            .map(|i| i.to_string())
            .collect();
        if n == "FPR02" || n == "FPR03" {
            // Gli esempi ufficiali FPR02/FPR03 (2014) ha riepilogo 27,00 a fronte di righe per 25,00:
            // con i controlli in vigore lo SdI lo scarterebbe. Il validatore deve accorgersene.
            assert!(serious.iter().any(|s| s.contains("00422")), "{:#?}", serious);
        } else {
            assert!(serious.is_empty(), "{}: {:#?}", n, serious);
        }
        // rigenerare l'XML dal modello letto deve produrre un file rileggibile identico
        let again = parse(&to_xml(&f)).unwrap();
        assert_eq!(again, f, "{}", n);
    }
}

#[test]
fn official_notifications() {
    for t in ["RC", "NS", "MC", "NE", "DT", "AT", "MT", "EC", "SE"] {
        let p = spec(&format!("notifiche/IT01234567890_11111_{}_001.xml", t));
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let n = notify::parse_notification_bytes(&bytes).unwrap_or_else(|e| panic!("{}: {}", t, e));
        assert_eq!(n.kind.code(), t);
        assert_eq!(n.sdi_id, "111");
        if t == "NS" {
            assert_eq!(n.errors[0].code, "00100");
        }
    }
}

#[test]
fn p7m_wrapped_invoice() {
    let p = spec("esempi/IT01234567890_FPR01.xml");
    let Ok(bytes) = std::fs::read(&p) else { return };
    let signed = crate::p7m::tests::fake_p7m(&bytes, true);
    let (f, xml) = read_any(&signed).unwrap();
    assert_eq!(xml, bytes);
    assert_eq!(f.versione, "FPR12");
}

#[test]
fn lot_and_serial_in_description() {
    let company = sample_company();
    let mut doc = Document::new(DocKind::Invoice);
    doc.date = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
    doc.party = sample_customer();
    doc.lines.push(DocLine {
        description: "Colla vinilica".into(),
        qty: dec!(2),
        unit_price: dec!(6.5),
        vat: v22(),
        lot: "CV2611A".into(),
        lot_expiry: NaiveDate::from_ymd_opt(2026, 11, 26),
        variant: "1 kg".into(),
        ..Default::default()
    });
    doc.lines.push(DocLine { description: "Trapano".into(), qty: dec!(1), unit_price: dec!(129), vat: v22(), serials: vec!["TR18-0002".into()], ..Default::default() });
    let t = finalize(&mut doc);
    let g = generate(&company, &doc, &t, &BuildOptions { progressivo: "00007".into(), ..Default::default() }).unwrap();
    assert_clean("lotti_seriali", &g);
    assert_eq!(g.fattura.bodies[0].linee[0].descrizione, "Colla vinilica (Var. 1 kg - Lotto CV2611A scad. 26/11/2026)");
    assert_eq!(g.fattura.bodies[0].linee[1].descrizione, "Trapano (S/N TR18-0002)");
}

#[test]
fn intent_subtotal_currency_and_nso() {
    let company = sample_company();
    let mut doc = Document::new(DocKind::Invoice);
    doc.date = NaiveDate::from_ymd_opt(2026, 4, 2).unwrap();
    doc.party = sample_customer();
    doc.party.intents.push(IntentDeclaration { protocol: "21012345678901234-000001".into(), date: NaiveDate::from_ymd_opt(2026, 1, 15), year: 2026, limit: dec!(50000), ..Default::default() });
    doc.intent_protocol = "21012345678901234-000001".into();
    doc.currency = "USD".into();
    doc.exchange_rate = dec!(1.0850);
    // ordine NSO: tripletta ID, data, #ufficio ordinante#
    doc.related.push(RelatedDoc { kind: "ORDINE".into(), number: "110".into(), date: NaiveDate::from_ymd_opt(2026, 3, 30), commessa: "#B9BGAQ#".into(), ..Default::default() });
    let n35 = VatRef { vat_id: None, rate: dec!(0), natura: Some("N3.5".into()), legal_ref: "Non imponibile art. 8 c.1 lett. c DPR 633/72".into(), split: false };
    doc.lines.push(DocLine { description: "Macchina utensile".into(), qty: dec!(1), unit_price: dec!(1000), vat: n35.clone(), eco_fee: dec!(2.5), eco_label: "Contributo RAEE".into(), ..Default::default() });
    doc.lines.push(DocLine { kind: LineKind::Subtotal, ..Default::default() });
    doc.lines.push(DocLine { description: "Imballo 2%".into(), calc_percent: Some(dec!(2)), vat: n35, ..Default::default() });
    doc.stamp.mode = StampMode::No;
    let t = finalize(&mut doc);
    assert_eq!(t.taxable, dec!(1022.55));
    let g = generate(&company, &doc, &t, &BuildOptions { progressivo: "00009".into(), ..Default::default() }).unwrap();
    assert_clean("fattura_intento_valuta_nso", &g);
    let back = parse(&g.xml).unwrap();
    assert_eq!(back.bodies[0].dati_generali.documento.divisa, "USD");
    let l = &back.bodies[0].linee;
    assert_eq!(l.len(), 4, "riga, contributo RAEE, subtotale, imballo");
    assert!(l[0].altri_dati_gestionali.iter().any(|a| a.tipo_dato == "INTENTO" && a.riferimento_data == NaiveDate::from_ymd_opt(2026, 1, 15)));
    assert!(l[0].altri_dati_gestionali.iter().any(|a| a.tipo_dato == "IMPONEUR" && a.riferimento_numero == Some(dec!(942.44))));
    assert_eq!(l[1].descrizione, "Contributo RAEE");
    assert!(l[2].descrizione.starts_with("Subtotale USD 1.002,50"), "{}", l[2].descrizione);
    assert!(g.xml.contains("<CodiceCommessaConvenzione>#B9BGAQ#</CodiceCommessaConvenzione>"));
}
