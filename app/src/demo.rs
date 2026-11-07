//! Dati dimostrativi per provare il programma senza toccare dati reali.

use chrono::{Datelike, Duration};
use openfatt_core::dec;
use openfatt_core::model::*;
use openfatt_store::{Store, StoreError};

pub fn populate(s: &Store) -> Result<(), StoreError> {
    let today = chrono::Local::now().date_naive();
    s.set_company(&Company {
        name: "Bottega Demo S.r.l.".into(),
        vat_number: "12345678903".into(),
        tax_code: "12345678903".into(),
        regime_fiscale: "RF01".into(),
        address: "Via delle Prove".into(),
        civic: "1".into(),
        zip: "38122".into(),
        city: "Trento".into(),
        province: "TN".into(),
        phone: "0461000000".into(),
        email: "info@bottegademo.example".into(),
        rea_office: "TN".into(),
        rea_number: "999999".into(),
        share_capital: Some(dec!(10000)),
        sole_shareholder: "SU".into(),
        bank_name: "Banca di prova".into(),
        iban: "IT60X0542811101000000123456".into(),
        invoice_footer: "Dati dimostrativi - documento di prova".into(),
        ..Default::default()
    })?;
    let vats = s.vat_codes()?;
    let v22 = vats.iter().find(|v| v.code == "22").unwrap().clone();
    let v10 = vats.iter().find(|v| v.code == "10").unwrap().clone();
    let pays = s.payment_methods()?;
    let bonifico = pays.iter().find(|p| p.name.starts_with("Bonifico 30")).map(|p| p.id);
    let riba = pays.iter().find(|p| p.name.starts_with("RiBa")).map(|p| p.id);
    let sdd = pays.iter().find(|p| p.mp_code == "MP19").map(|p| p.id);
    // codici bancari dimostrativi (quelli veri li comunica la banca)
    s.set_bank_settings(&openfatt_store::BankSettings { sia: "A1B2C".into(), cuc: "DEMO1234".into(), creditor_id: "IT04ZZZ12345678903".into(), account: "Banca".into() })?;

    let riv = s.save_price_list(&openfatt_core::pricing::PriceList { name: "Rivenditori".into(), percent: dec!(-15), round_to: dec!(0.10), ..Default::default() })?;
    s.save_price_list(&openfatt_core::pricing::PriceList { name: "Ingrosso".into(), base: openfatt_core::pricing::PriceBase::Cost, percent: dec!(30), round_to: dec!(0.50), ..Default::default() })?;
    s.save_price_list(&openfatt_core::pricing::PriceList { name: "Pubblico".into(), vat_included: true, round_to: dec!(0.10), ..Default::default() })?;
    let c1 = s.save_party(&Party {
        name: "Rossi Arredamenti S.p.A.".into(),
        vat_number: "06363391001".into(),
        address: "Viale Europa".into(),
        civic: "10".into(),
        zip: "00144".into(),
        city: "Roma".into(),
        province: "RM".into(),
        sdi_code: "ABC1234".into(),
        email: "acquisti@rossi.example".into(),
        iban: "IT36A0301503200000003517230".into(),
        bank_name: "Banca di prova - Roma".into(),
        default_payment_id: riba,
        price_list_id: Some(riv.id),
        ..Default::default()
    })?;
    let c2 = s.save_party(&Party {
        first_name: "Mario".into(),
        last_name: "Bianchi".into(),
        tax_code: "RSSMRA85T10A562S".into(),
        address: "Via Garibaldi".into(),
        civic: "5".into(),
        zip: "38068".into(),
        city: "Rovereto".into(),
        province: "TN".into(),
        default_payment_id: bonifico,
        iban: "IT68B0306909606100000063112".into(),
        sdd_mandate_id: "BOTTEGA-0001".into(),
        sdd_mandate_date: Some(today - Duration::days(400)),
        sdd_scheme: "CORE".into(),
        ..Default::default()
    })?;
    let f1 = s.save_party(&Party {
        role: PartyRole::Supplier,
        name: "Forniture Industriali S.r.l.".into(),
        vat_number: "01234567897".into(),
        address: "Via dell'Industria".into(),
        civic: "22".into(),
        zip: "20100".into(),
        city: "Milano".into(),
        province: "MI".into(),
        iban: "IT98C0200801664000101234567".into(),
        default_payment_id: bonifico,
        ..Default::default()
    })?;

    let mut products = Vec::new();
    for (code, desc, price, cost, vat, service) in [
        ("SED01", "Sedia ergonomica da ufficio", dec!(149.00), dec!(82.00), &v22, false),
        ("SCR01", "Scrivania 160x80 rovere", dec!(320.00), dec!(190.00), &v22, false),
        ("LAM01", "Lampada LED da tavolo", dec!(45.90), dec!(21.50), &v22, false),
        ("LIB01", "Manuale di montaggio (libro)", dec!(18.00), dec!(9.00), &v10, false),
        ("MONT", "Servizio di montaggio (ora)", dec!(38.00), dec!(0), &v22, true),
    ] {
        products.push(s.save_product(&Product {
            code: code.into(),
            description: desc.into(),
            price,
            cost,
            vat_id: Some(vat.id),
            is_service: service,
            manage_stock: !service,
            unit: if service { "h".into() } else { "pz".into() },
            min_stock: if service { dec!(0) } else { dec!(5) },
            ..Default::default()
        })?);
    }
    s.set_price_list_item(riv.id, products[0].id, dec!(10), Some(dec!(119)))?;
    s.save_party_price(&openfatt_core::pricing::PartyPrice { party_id: c2.id, product_id: Some(products[2].id), price: Some(dec!(39)), note: "Cliente fedele".into(), ..Default::default() })?;
    for p in products.iter().filter(|p| p.manage_stock) {
        s.add_manual_stock_move(&StockMove { product_id: p.id, qty: dec!(20), reason: "Carico iniziale".into(), unit_value: p.cost, date: today - Duration::days(60), ..Default::default() })?;
    }
    let line = |p: &Product, qty: rust_decimal::Decimal, disc: &str| DocLine {
        product_id: Some(p.id),
        code: p.code.clone(),
        description: p.description.clone(),
        qty,
        unit: p.unit.clone(),
        unit_price: p.price,
        discounts: disc.into(),
        vat: VatRef::from_code(vats.iter().find(|v| Some(v.id) == p.vat_id).unwrap()),
        unit_cost: p.cost,
        ..Default::default()
    };

    // fatture di servizi più vecchie e non ancora incassate (per scadenzario e solleciti)
    let mut old1 = Document::new(DocKind::Invoice);
    old1.date = today - Duration::days(100);
    old1.party_id = Some(c1.id);
    old1.payment_id = riba;
    old1.lines = vec![line(&products[4], dec!(12), "")];
    let old1 = s.save_document(&old1)?;
    s.issue_document(old1.id)?;
    let mut old2 = Document::new(DocKind::Invoice);
    old2.date = today - Duration::days(70);
    old2.party_id = Some(c2.id);
    old2.payment_id = sdd;
    old2.lines = vec![line(&products[4], dec!(3), "")];
    let old2 = s.save_document(&old2)?;
    s.issue_document(old2.id)?;

    // preventivo -> ordine -> DDT -> fattura
    let mut q = Document::new(DocKind::Quote);
    q.date = today - Duration::days(40);
    q.party_id = Some(c1.id);
    q.payment_id = riba;
    q.lines = vec![line(&products[0], dec!(6), "10"), line(&products[1], dec!(3), "10+5"), line(&products[4], dec!(4), "")];
    let q = s.save_document(&q)?;
    let q = s.issue_document(q.id)?;
    let ord = s.transform(&[q.id], DocKind::CustomerOrder)?;
    let ord = s.issue_document(ord.id)?;
    let mut ddt = s.transform(&[ord.id], DocKind::DeliveryNote)?;
    ddt.date = today - Duration::days(20);
    if let Some(t) = ddt.transport.as_mut() {
        t.reason = "Vendita".into();
        t.by = "Mittente".into();
        t.packages = Some(4);
    }
    let ddt = s.save_document(&ddt)?;
    let ddt = s.issue_document(ddt.id)?;
    let mut inv = s.transform(&[ddt.id], DocKind::Invoice)?;
    inv.date = today - Duration::days(18);
    let inv = s.save_document(&inv)?;
    s.issue_document(inv.id)?;

    // fattura immediata a privato
    let mut f2 = Document::new(DocKind::Invoice);
    f2.date = today - Duration::days(5);
    f2.party_id = Some(c2.id);
    f2.payment_id = bonifico;
    f2.lines = vec![line(&products[2], dec!(2), ""), line(&products[3], dec!(1), "")];
    let f2 = s.save_document(&f2)?;
    s.issue_document(f2.id)?;

    // preventivo aperto
    let mut q2 = Document::new(DocKind::Quote);
    q2.date = today;
    q2.party_id = Some(c2.id);
    q2.lines = vec![line(&products[1], dec!(1), ""), line(&products[0], dec!(1), "")];
    s.save_document(&q2)?;

    // articoli con lotti, varianti e numeri di serie
    let colla = s.save_product(&Product {
        code: "COL01".into(),
        description: "Colla vinilica 1 kg".into(),
        price: dec!(6.50),
        cost: dec!(3.10),
        vat_id: Some(v22.id),
        tracking: Tracking::Lot,
        ..Default::default()
    })?;
    let maglia = s.save_product(&Product {
        code: "MAG01".into(),
        description: "Maglia da lavoro".into(),
        price: dec!(19.00),
        cost: dec!(8.00),
        vat_id: Some(v22.id),
        variants: VariantSpec {
            axis1: "Taglia".into(),
            values1: vec!["S".into(), "M".into(), "L".into()],
            axis2: "Colore".into(),
            values2: vec!["Blu".into(), "Grigio".into()],
            barcodes: vec![VariantBarcode { variant: "M/Blu".into(), barcode: "2000001000025".into() }],
        },
        ..Default::default()
    })?;
    let trapano = s.save_product(&Product {
        code: "TRA01".into(),
        description: "Trapano avvitatore 18V".into(),
        price: dec!(129.00),
        cost: dec!(71.00),
        vat_id: Some(v22.id),
        tracking: Tracking::Serial,
        ..Default::default()
    })?;
    let mut arr = Document::new(DocKind::GoodsReceipt);
    arr.date = today - Duration::days(30);
    arr.party_id = Some(f1.id);
    arr.external_number = "DDT-F 512".into();
    let item = |p: &Product, qty: rust_decimal::Decimal| DocLine {
        product_id: Some(p.id),
        code: p.code.clone(),
        description: p.description.clone(),
        qty,
        unit: p.unit.clone(),
        unit_price: p.cost,
        vat: VatRef::from_code(&v22),
        ..Default::default()
    };
    let mut l1 = item(&colla, dec!(12));
    l1.lot = "CV2611A".into();
    l1.lot_expiry = Some(today + Duration::days(20));
    let mut l2 = item(&colla, dec!(24));
    l2.lot = "CV2702B".into();
    l2.lot_expiry = Some(today + Duration::days(200));
    let mut m1 = item(&maglia, dec!(5));
    m1.variant = "M/Blu".into();
    let mut m2 = item(&maglia, dec!(4));
    m2.variant = "L/Grigio".into();
    let mut t1 = item(&trapano, dec!(3));
    t1.serials = vec!["TR18-0001".into(), "TR18-0002".into(), "TR18-0003".into()];
    arr.lines = vec![l1, l2, m1, m2, t1];
    let arr = s.save_document(&arr)?;
    s.issue_document(arr.id)?;
    // fattura del fornitore per un servizio, da pagare con bonifico
    let mut pinv = Document::new(DocKind::PurchaseInvoice);
    pinv.date = today - Duration::days(20);
    pinv.party_id = Some(f1.id);
    pinv.external_number = "45/A".into();
    pinv.payment_id = bonifico;
    pinv.lines = vec![DocLine { description: "Trasporto e consegna merce".into(), qty: dec!(1), unit_price: dec!(180), vat: VatRef::from_code(&v22), ..Default::default() }];
    let pinv = s.save_document(&pinv)?;
    s.issue_document(pinv.id)?;
    let mut out = Document::new(DocKind::DeliveryNote);
    out.date = today - Duration::days(10);
    out.party_id = Some(c1.id);
    let mut o1 = item(&colla, dec!(15)); // 12 dal lotto in scadenza + 3 dal successivo
    o1.unit_price = colla.price;
    let mut o2 = item(&trapano, dec!(1));
    o2.unit_price = trapano.price;
    o2.serials = vec!["TR18-0002".into()];
    out.lines = vec![o1, o2];
    let out = s.save_document(&out)?;
    s.issue_document(out.id)?;

    // secondo magazzino e trasferimento di merce verso il negozio
    s.save_warehouse(&Warehouse { name: "Negozio".into(), address: "Corso III Novembre 5, Trento".into(), ..Default::default() })?;
    let mut trs = Document::new(DocKind::StockTransfer);
    trs.date = today - Duration::days(7);
    trs.to_warehouse = "Negozio".into();
    trs.lines.push(item(&products[0], dec!(4)));
    let mut tl = item(&trapano, dec!(1));
    tl.serials = vec!["TR18-0003".into()];
    trs.lines.push(tl);
    let trs = s.save_document(&trs)?;
    s.issue_document(trs.id)?;

    // inventario di prova del negozio, parzialmente contato
    let inv = s.create_inventory(today, "Negozio", "Inventario di prova del negozio", None)?;
    for l in s.inventory_lines(inv)? {
        if l.code == "SED01" {
            s.set_inventory_count(l.id, Some(dec!(3)), Some("una sedia danneggiata in esposizione"))?;
        }
        if l.code == "LAM01" {
            s.set_inventory_count(l.id, Some(dec!(2)), None)?;
        }
        if l.code == "TRA01" && l.serial == "TR18-0003" {
            s.set_inventory_count(l.id, Some(dec!(1)), None)?;
        }
    }

    // vendite al banco (estremi del registratore simulati)
    for (prod, qty, cash) in [(&products[2], dec!(1), true), (&products[3], dec!(2), false)] {
        let v = vats.iter().find(|v| Some(v.id) == prod.vat_id).unwrap();
        let gross = openfatt_core::money::round2(prod.price * (rust_decimal::Decimal::ONE + v.rate / rust_decimal::Decimal::ONE_HUNDRED));
        let mut d = Document::new(DocKind::RetailSale);
        d.prices_include_vat = true;
        d.lines.push(DocLine { product_id: Some(prod.id), code: prod.code.clone(), description: prod.description.clone(), qty, unit_price: gross, vat: VatRef::from_code(v), unit_cost: prod.cost, ..Default::default() });
        let total = openfatt_core::calc::compute(&d).map(|t| t.total).unwrap_or_default();
        d.retail = Some(RetailInfo {
            payments: vec![RetailPayment { method: if cash { RetailPayMethod::Cash } else { RetailPayMethod::Electronic }, amount: total }],
            cash_given: total,
            ..Default::default()
        });
        let saved = s.record_retail_sale(&d, "Cassa", "Banca")?;
        s.set_retail_outcome(saved.id, RtStatus::Simulated, "SIMULATORE", "0001", &saved.number.unwrap_or(0).to_string(), "dati dimostrativi")?;
    }

    extended(s, &c1, &c2, &products, &v22)?;
    let _ = today.year();
    Ok(())
}

/// Dati per le funzioni avanzate: agente, ricorrenze, gruppi e distinte base, produzione,
/// contributi ambientali, dichiarazione d'intento, carte, agenda, ordine NSO.
fn extended(s: &Store, c1: &Party, c2: &Party, products: &[Product], v22: &VatCode) -> Result<(), StoreError> {
    let today = chrono::Local::now().date_naive();
    // dichiarante della LIPE: il legale rappresentante (codice fiscale di prova)
    let mut vs = s.vat_settings()?;
    vs.declarant_cf = "RSSMRA85T10A562S".into();
    vs.declarant_role = 1;
    s.set_vat_settings(&vs)?;
    // agente sul cliente Rossi e sulle sue fatture
    let ag = s.save_agent(&openfatt_store::Agent { name: "Marco Bassi".into(), email: "marco.bassi@example.com".into(), rate: dec!(8), on_collected: true, ..Default::default() })?;
    let mut rossi = s.party(c1.id)?.unwrap();
    rossi.agent_id = Some(ag.id);
    rossi.contacts.push(Contact { name: "Laura Rossi".into(), role: "Amministrazione".into(), email: "amministrazione@rossi.example".into(), gets_documents: true, ..Default::default() });
    s.save_party(&rossi)?;
    for d in s.documents(&openfatt_store::DocFilter { kinds: vec![DocKind::Invoice, DocKind::DeliveryNote], ..Default::default() })? {
        if d.party_id == Some(c1.id) {
            let mut full = s.document(d.id)?;
            full.agent_id = Some(ag.id);
            let _ = s.save_document(&full);
        }
    }
    // gruppo (kit) e prodotto con distinta base
    s.save_product(&Product {
        code: "KIT01".into(),
        description: "Postazione completa (sedia, scrivania, lampada)".into(),
        kind: ProductKind::Kit,
        price: dec!(470),
        vat_id: Some(v22.id),
        manage_stock: false,
        components: vec![Component { product_id: products[0].id, qty: dec!(1), note: String::new() }, Component { product_id: products[1].id, qty: dec!(1), note: String::new() }, Component { product_id: products[2].id, qty: dec!(1), note: String::new() }],
        ..Default::default()
    })?;
    let gamba = s.save_product(&Product { code: "GAM01".into(), description: "Gamba in acciaio".into(), cost: dec!(6.50), vat_id: Some(v22.id), category: "Semilavorati".into(), ..Default::default() })?;
    let piano = s.save_product(&Product { code: "PIA01".into(), description: "Piano in rovere 120x60".into(), cost: dec!(42), vat_id: Some(v22.id), category: "Semilavorati".into(), ..Default::default() })?;
    for (p, q) in [(&gamba, dec!(40)), (&piano, dec!(8))] {
        s.add_manual_stock_move(&StockMove { product_id: p.id, qty: q, unit_value: p.cost, reason: "Carico iniziale".into(), date: today - Duration::days(20), ..Default::default() })?;
    }
    let tavolo = s.save_product(&Product {
        code: "TAV01".into(),
        description: "Tavolo riunioni 120x60".into(),
        kind: ProductKind::Assembled,
        price: dec!(210),
        cost: dec!(68),
        vat_id: Some(v22.id),
        category: "Arredo".into(),
        components: vec![Component { product_id: gamba.id, qty: dec!(4), note: String::new() }, Component { product_id: piano.id, qty: dec!(1), note: String::new() }],
        online: true,
        ..Default::default()
    })?;
    let mut prod = Document::new(DocKind::Production);
    prod.date = today - Duration::days(10);
    prod.lines.push(DocLine { product_id: Some(tavolo.id), code: tavolo.code.clone(), description: tavolo.description.clone(), qty: dec!(3), unit: "pz".into(), ..Default::default() });
    let prod = s.save_document(&prod)?;
    s.issue_document(prod.id)?;
    // articolo con contributo RAEE e confezioni
    s.save_product(&Product { code: "LED10".into(), description: "Lampadina LED E27 10W".into(), price: dec!(4.90), cost: dec!(1.80), vat_id: Some(v22.id), eco_fee: dec!(0.15), eco_label: "Contributo RAEE".into(), pack_size: dec!(10), pack_unit: "CF".into(), category: "Illuminazione".into(), ..Default::default() })?;
    // cliente esportatore abituale con dichiarazione d'intento
    s.save_party(&Party {
        name: "Export Meccanica S.p.A.".into(),
        vat_number: "01234567897".into(),
        address: "Via del Porto".into(),
        civic: "12".into(),
        zip: "16126".into(),
        city: "Genova".into(),
        province: "GE".into(),
        sdi_code: "M5UXCR1".into(),
        intents: vec![IntentDeclaration { protocol: format!("{}0123456789012-000001", today.year() % 100), date: Some(chrono::NaiveDate::from_ymd_opt(today.year(), 1, 20).unwrap()), year: today.year(), limit: dec!(150000), ..Default::default() }],
        ..Default::default()
    })?;
    // documento ricorrente: canone mensile di assistenza
    let mut tpl = Document::new(DocKind::Invoice);
    tpl.party_id = Some(c1.id);
    tpl.date = today;
    tpl.payment_id = c1.default_payment_id;
    tpl.lines.push(DocLine { description: "Canone di assistenza {mese} {anno}".into(), qty: dec!(1), unit_price: dec!(120), vat: VatRef::from_code(v22), ..Default::default() });
    let tpl = s.save_document(&tpl)?;
    s.save_recurrence(&openfatt_store::Recurrence { template_id: tpl.id, description: "Assistenza Rossi".into(), every_months: 1, next_date: openfatt_store::add_months(chrono::NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap(), 1), ..Default::default() })?;
    // carta fedeltà e buono regalo
    let fd = s.save_card(&openfatt_store::Card { code: "FD00000001".into(), kind: openfatt_store::CardKind::Loyalty, party_id: Some(c2.id), issued: Some(today), active: true, ..Default::default() })?;
    s.add_card_move(fd.id, today - Duration::days(5), dec!(120), dec!(0), None, "Punti di benvenuto")?;
    let gc = s.save_card(&openfatt_store::Card { code: "GC00000001".into(), kind: openfatt_store::CardKind::Gift, holder: "Buono regalo".into(), issued: Some(today), expiry: Some(openfatt_store::add_months(today, 12)), active: true, ..Default::default() })?;
    s.add_card_move(gc.id, today, dec!(0), dec!(50), None, "Emissione del buono")?;
    // agenda
    s.save_agenda_item(&openfatt_store::AgendaItem { date: today, time: "10:30".into(), kind: openfatt_store::AgendaKind::Call, title: "Richiamare Rossi per il preventivo".into(), party_id: Some(c1.id), ..Default::default() })?;
    s.save_agenda_item(&openfatt_store::AgendaItem { date: today + Duration::days(2), time: "15:00".into(), kind: openfatt_store::AgendaKind::Appointment, title: "Consegna e montaggio postazioni".into(), party_id: Some(c1.id), ..Default::default() })?;
    // ordine NSO dalla Pubblica Amministrazione
    let o = openfatt_interop::nso::parse_order(openfatt_interop::nso::SAMPLE_ORDER).map_err(StoreError::Invalid)?;
    s.import_nso_order(&o)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Percorso completo come dall'interfaccia: dati demo, fatture elettroniche, PDF, scadenze.
    #[test]
    fn demo_archive_end_to_end() {
        let s = Store::open_in_memory().unwrap();
        populate(&s).unwrap();
        let docs = s.documents(&openfatt_store::DocFilter { kinds: vec![DocKind::Invoice], status: Some(DocStatus::Issued), ..Default::default() }).unwrap();
        assert_eq!(docs.len(), 4);
        let out_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/xsd-samples");
        std::fs::create_dir_all(&out_dir).unwrap();
        for d in &docs {
            let (e, issues) = crate::services::create_einvoice(&s, d.id).unwrap_or_else(|e| panic!("{}", e));
            assert!(!openfatt_fatturapa::has_errors(&issues));
            std::fs::write(out_dir.join(format!("demo_{}", e.file_name)), &e.xml).unwrap();
            let doc = s.document(d.id).unwrap();
            let pdf = crate::services::document_pdf(&s, &doc).unwrap();
            assert!(pdf.starts_with(b"%PDF"));
            let pdf2 = crate::services::einvoice_pdf(&e.xml).unwrap();
            assert!(pdf2.starts_with(b"%PDF"));
        }
        // la fattura da DDT è differita (TD24) e riporta i DatiDDT
        let from_ddt = docs.iter().filter(|d| d.party_name.starts_with("Rossi")).max_by_key(|d| d.date).unwrap();
        let e = s.einvoice_for_document(from_ddt.id).unwrap().unwrap();
        let f = openfatt_fatturapa::parse_bytes(&e.xml).unwrap();
        assert_eq!(f.bodies[0].dati_generali.documento.tipo_documento, "TD24");
        assert_eq!(f.bodies[0].dati_generali.ddt.len(), 1);
        // progressivi distinti
        assert_eq!(s.peek_counter("fe_progressivo").unwrap(), 4);
        // scadenze RiBa generate e magazzino scaricato una sola volta (dal DDT)
        assert!(!s.due_list(true, None).unwrap().is_empty());
        let sedia = s.product_by_code("SED01").unwrap().unwrap();
        assert_eq!(s.stock_of(sedia.id).unwrap(), dec!(14));
        // notifica di consegna simulata e importazione di una fattura passiva
        let rc = format!(
            r#"<?xml version="1.0"?><types:RicevutaConsegna xmlns:types="http://www.fatturapa.gov.it/sdi/messaggi/v1.0" versione="1.0"><IdentificativoSdI>42</IdentificativoSdI><NomeFile>{}</NomeFile><DataOraRicezione>2026-01-01T10:00:00Z</DataOraRicezione><DataOraConsegna>2026-01-01T10:01:00Z</DataOraConsegna><MessageId>9</MessageId></types:RicevutaConsegna>"#,
            e.file_name
        );
        s.mark_einvoice_sent(e.id, "test").unwrap();
        let mut sum = crate::services::InboundSummary::default();
        let nname = e.file_name.replace(".xml", "_RC_001.xml");
        crate::services::import_file(&s, &nname, rc.as_bytes(), &mut sum);
        crate::services::import_file(&s, &nname, rc.as_bytes(), &mut sum);
        assert_eq!(sum.notifications, 1, "{:?}", sum.errors);
        assert_eq!(sum.duplicates, 1);
        assert_eq!(s.einvoice(e.id).unwrap().status, openfatt_store::EInvoiceStatus::Delivered);
    }
}
