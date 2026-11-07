//! Prova di carico: archivio con 100.000 articoli, 20.000 clienti e 30.000 fatture.
//!
//! Si esegue a mano (richiede alcuni minuti in modalità release):
//!   cargo test --release -p openfatt-store --test carico -- --ignored --nocapture
//! L'archivio resta in target/perf/grande.gadb per provare l'interfaccia
//! (OPENFATT_ARCHIVE=target/perf/grande.gadb).

use std::time::Instant;

use chrono::NaiveDate;
use openfatt_core::dec;
use openfatt_core::model::*;
use openfatt_store::{DocFilter, PartyFilter, Store};

const PRODUCTS: usize = 100_000;
const PARTIES: usize = 20_000;
const INVOICES: usize = 30_000;

fn t(label: &str, start: Instant) -> f64 {
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    println!("{:<58} {:>10.1} ms", label, ms);
    ms
}

#[test]
#[ignore]
fn archivio_grande() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/perf");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("grande.gadb");
    let s = Store::open(&path).unwrap();
    let v = s.vat_codes().unwrap().into_iter().find(|v| v.rate == dec!(22)).unwrap();
    // si crea solo quello che manca (la prova si può interrompere e riprendere)
    let have_products = s.products_page("", true, usize::MAX).unwrap().len();
    if have_products < PRODUCTS {
        let start = Instant::now();
        s.with_transaction(|| {
            for i in have_products..PRODUCTS {
                let ean12 = format!("80{:010}", i);
                let ean = format!("{}{}", ean12, openfatt_core::model::ean13_check_digit(&ean12).unwrap());
                s.save_product(&Product {
                    code: format!("ART{:06}", i),
                    description: format!("Articolo di prova {} {}", ["vite", "bullone", "dado", "rondella", "tassello", "cavo", "interruttore", "presa"][i % 8], i),
                    category: format!("Categoria {}", i % 40),
                    price: rust_decimal::Decimal::new(100 + (i as i64 % 9000), 2),
                    cost: rust_decimal::Decimal::new(50 + (i as i64 % 4000), 2),
                    vat_id: Some(v.id),
                    barcode: ean,
                    min_stock: if i % 10 == 0 { dec!(5) } else { dec!(0) },
                    ..Default::default()
                })?;
            }
            Ok(())
        })
        .unwrap();
        t(&format!("creazione di {} articoli", PRODUCTS - have_products), start);
    }
    let have_parties = s.parties(&PartyFilter { include_inactive: true, ..Default::default() }).unwrap().len();
    if have_parties < PARTIES {
        let start = Instant::now();
        s.with_transaction(|| {
            for i in have_parties..PARTIES {
                s.save_party(&Party { name: format!("Cliente {} S.r.l.", i), vat_number: format!("{:011}", 10_000_000 + i), city: ["Roma", "Milano", "Torino", "Trento"][i % 4].into(), email: format!("c{}@example.com", i), ..Default::default() })?;
            }
            Ok(())
        })
        .unwrap();
        t(&format!("creazione di {} clienti", PARTIES - have_parties), start);
    }
    let have_invoices = s.documents(&DocFilter { kinds: vec![DocKind::Invoice], ..Default::default() }).unwrap().len();
    if have_invoices < INVOICES {
        let start = Instant::now();
        let parties = s.parties(&PartyFilter::default()).unwrap();
        s.with_transaction(|| {
            for i in have_invoices..INVOICES {
                let p = &parties[i % parties.len()];
                let mut d = Document::new(DocKind::Invoice);
                d.date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days((i * 300 / INVOICES) as i64);
                d.party_id = Some(p.id);
                for k in 0..5 {
                    let pid = ((i * 7 + k * 13) % PRODUCTS) as i64 + 1;
                    d.lines.push(DocLine { product_id: Some(pid), code: format!("ART{:06}", pid - 1), description: "Articolo".into(), qty: dec!(2), unit_price: dec!(10), vat: VatRef::from_code(&v), ..Default::default() });
                }
                let d = s.save_document(&d)?;
                s.issue_document(d.id)?;
            }
            Ok(())
        })
        .unwrap();
        t(&format!("emissione di {} fatture da 5 righe", INVOICES - have_invoices), start);
    }
    println!("dimensione archivio: {:.1} MB", s.file_size() as f64 / 1_048_576.0);

    let mut worst = 0.0f64;
    let mut check = |label: &str, start: Instant, limit_ms: f64| {
        let ms = t(label, start);
        worst = worst.max(ms / limit_ms);
        assert!(ms < limit_ms * 4.0, "{} troppo lento: {:.0} ms", label, ms);
    };
    let start = Instant::now();
    let all = s.products("", false).unwrap();
    assert_eq!(all.len(), PRODUCTS);
    check("elenco completo degli articoli (100.000)", start, 2500.0);
    let start = Instant::now();
    let r = s.products_page("bullone 99", false, 60).unwrap();
    assert!(!r.is_empty());
    check("ricerca mentre si scrive (prime 60)", start, 150.0);
    let start = Instant::now();
    let r = s.products_page("ART0999", false, 60).unwrap();
    assert_eq!(r[0].code, "ART099900");
    check("ricerca per inizio di codice", start, 150.0);
    let start = Instant::now();
    let ean12 = format!("80{:010}", 77_777);
    let ean = format!("{}{}", ean12, openfatt_core::model::ean13_check_digit(&ean12).unwrap());
    assert_eq!(s.find_by_code(&ean).unwrap().unwrap().0.code, "ART077777");
    check("lettura di un codice a barre", start, 20.0);
    let start = Instant::now();
    assert!(s.find_by_code("ART050000").unwrap().is_some());
    check("ricerca per codice esatto", start, 20.0);
    let start = Instant::now();
    let rep = s.stock_report("", None).unwrap();
    assert_eq!(rep.len(), PRODUCTS);
    check("giacenze di tutti gli articoli", start, 4000.0);
    let start = Instant::now();
    let c = s.parties(&PartyFilter { search: "Cliente 1999".into(), ..Default::default() }).unwrap();
    assert!(!c.is_empty());
    check("ricerca clienti", start, 300.0);
    let start = Instant::now();
    let docs = s.documents(&DocFilter { kinds: vec![DocKind::Invoice], ..Default::default() }).unwrap();
    assert!(docs.len() >= INVOICES);
    check("elenco delle fatture (30.000)", start, 2000.0);
    let start = Instant::now();
    let docs = s.documents(&DocFilter { kinds: vec![DocKind::Invoice], year: Some(2026), search: "Cliente 1234 ".into(), ..Default::default() }).unwrap();
    let _ = docs;
    check("ricerca fatture per cliente", start, 2000.0);
    let start = Instant::now();
    let d = s.document(12_345).unwrap();
    assert_eq!(d.lines.len(), 5);
    check("apertura di un documento", start, 20.0);
    let start = Instant::now();
    let mut nd = Document::new(DocKind::Invoice);
    nd.party_id = Some(1);
    nd.lines.push(DocLine { product_id: Some(42), description: "x".into(), unit_price: dec!(1), vat: VatRef::from_code(&v), ..Default::default() });
    let nd = s.save_document(&nd).unwrap();
    s.issue_document(nd.id).unwrap();
    check("emissione di una fattura nuova", start, 150.0);
    let start = Instant::now();
    let _ = s.last_prices(1, 42, false, 10).unwrap();
    check("ultimi prezzi praticati", start, 200.0);
    let start = Instant::now();
    let _ = s.due_list(true, None).unwrap();
    check("scadenzario aperto", start, 1500.0);
    println!("rapporto peggiore tra tempo misurato e obiettivo: {:.2}", worst);
}
