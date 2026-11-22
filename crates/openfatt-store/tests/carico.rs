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

/// Tempi delle interrogazioni che le schermate fanno più spesso, sull'archivio grande già
/// creato da `archivio_grande` (sola lettura).
///   cargo test --release -p openfatt-store --test carico tempi -- --ignored --nocapture
#[test]
#[ignore]
fn tempi_delle_interrogazioni() {
    use openfatt_store::{Direction, EInvoiceStatus};
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/perf/grande.gadb");
    if !path.exists() {
        eprintln!("manca {}: eseguire prima archivio_grande", path.display());
        return;
    }
    let s = Store::open(&path).unwrap();
    let today = chrono::Local::now().date_naive();
    macro_rules! m {
        ($label:expr, $e:expr) => {{
            let start = Instant::now();
            let r = $e;
            t($label, start);
            r
        }};
    }
    m!("generation", s.generation());
    m!("company", s.company().unwrap());
    m!("warehouses", s.warehouses().unwrap());
    m!("accounts", s.accounts().unwrap());
    m!("expiring_lots", s.expiring_lots(today + chrono::Duration::days(30)).unwrap());
    m!("stock_moves(None, 2000)", s.stock_moves(None, 2000).unwrap());
    m!("stock_moves(Some(42), 2000)", s.stock_moves(Some(42), 2000).unwrap());
    m!("products(\"\", true)", s.products("", true).unwrap());
    m!("stock_report", s.stock_report("", None).unwrap());
    m!("reminder_candidates", s.reminder_candidates(today, 0, 0).unwrap());
    m!("due_list_with_drafts(true)", s.due_list_with_drafts(true, None).unwrap());
    m!("due_list_with_drafts(false)", s.due_list_with_drafts(false, None).unwrap());
    m!("einvoices(Out)", s.einvoices(Direction::Out, "", None).unwrap());
    m!("documents_without_einvoice", s.documents_without_einvoice().unwrap());
    m!("einvoice_count", s.einvoice_count(Direction::Out, EInvoiceStatus::Sent).unwrap());
    m!("due_recurrences", s.due_recurrences(today).unwrap());
    m!("agenda", s.agenda(today, today).unwrap());
    m!("documents(Quote, Draft)", s.documents(&DocFilter { kinds: vec![DocKind::Quote], status: Some(DocStatus::Draft), ..Default::default() }).unwrap());
    m!("inventory_sessions", s.inventory_sessions().unwrap());
    m!("documents(tutti)", s.documents(&DocFilter::default()).unwrap());
    m!("parties(tutti)", s.parties(&PartyFilter { include_inactive: true, ..Default::default() }).unwrap());
    m!("products_page(\"vite\", 60)", s.products_page("vite", false, 60).unwrap());
    m!("documents(search vite)", s.documents(&DocFilter { search: "vite".into(), ..Default::default() }).unwrap());
    m!("parties(search vite)", s.parties(&PartyFilter { search: "vite".into(), ..Default::default() }).unwrap());
    m!("products(\"vite\")", s.products("vite", false).unwrap());
    m!("account_balances", s.account_balances().unwrap());
    let year = chrono::Datelike::year(&today);
    let (gf, gt) = (chrono::NaiveDate::from_ymd_opt(year, 3, 1).unwrap(), chrono::NaiveDate::from_ymd_opt(year, 3, 31).unwrap());
    m!("vat_register(vendite, marzo)", s.vat_register(openfatt_core::vat::Register::Sales, gf, gt).unwrap());
    m!("vat_register(acquisti, marzo)", s.vat_register(openfatt_core::vat::Register::Purchases, gf, gt).unwrap());
    m!("retail_register(marzo)", s.retail_register(gf, gt).unwrap());
    m!("deferred_vat(marzo)", s.deferred_vat(gf, gt).unwrap());
    m!("vat_liquidations(anno, 12)", s.vat_liquidations(year, 12).unwrap());
    m!("vat_liquidations(anno, 12) di nuovo", s.vat_liquidations(year, 12).unwrap());
    m!("vat_register(vendite, anno)", s.vat_register(openfatt_core::vat::Register::Sales, chrono::NaiveDate::from_ymd_opt(year, 1, 1).unwrap(), chrono::NaiveDate::from_ymd_opt(year, 12, 31).unwrap()).unwrap());
    m!("annual_vat", s.annual_vat(year).unwrap());
    m!("statistics(cliente, anno)", s.statistics(openfatt_store::StatBy::Customer, chrono::NaiveDate::from_ymd_opt(year, 1, 1).unwrap(), chrono::NaiveDate::from_ymd_opt(year, 12, 31).unwrap()).unwrap());
    m!("statistics(cliente, anno) di nuovo", s.statistics(openfatt_store::StatBy::Customer, chrono::NaiveDate::from_ymd_opt(year, 1, 1).unwrap(), chrono::NaiveDate::from_ymd_opt(year, 12, 31).unwrap()).unwrap());
    m!("statistics(articolo, anno)", s.statistics(openfatt_store::StatBy::Product, chrono::NaiveDate::from_ymd_opt(year, 1, 1).unwrap(), chrono::NaiveDate::from_ymd_opt(year, 12, 31).unwrap()).unwrap());
}
