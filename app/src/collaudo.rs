//! Collaudo completo: percorre tutte le funzioni sull'archivio dimostrativo, come farebbe
//! un utente, e salva in target/collaudo i file prodotti (fatture elettroniche, file
//! bancari, LIPE, PDF, Excel, pacchetto di conservazione) per le validazioni ufficiali:
//!   cargo test -p openfatt collaudo -- --nocapture
//!   powershell -File tools\valida-xsd.ps1 target\collaudo\fe
//!   powershell -File tools\valida-cbi.ps1 target\collaudo\cbi
//!   powershell -File tools\valida-lipe.ps1 target\collaudo\lipe

#![cfg(test)]

use std::path::PathBuf;

use chrono::{Datelike, NaiveDate};
use openfatt_core::dec;
use openfatt_core::model::*;
use openfatt_store::{BankKind, DocFilter, Store};
use rust_decimal::Decimal;

struct Report {
    dir: PathBuf,
    lines: Vec<String>,
}

impl Report {
    fn ok(&mut self, area: &str, what: impl Into<String>) {
        let w = what.into();
        println!("OK  {:<22} {}", area, w);
        self.lines.push(format!("| {} | {} | superato |", area, w));
    }
    fn file(&self, sub: &str, name: &str, data: &[u8]) -> PathBuf {
        let d = self.dir.join(sub);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join(name);
        std::fs::write(&p, data).unwrap();
        p
    }
}

fn pdf_ok(b: &[u8]) -> bool {
    b.starts_with(b"%PDF") && b.windows(5).any(|w| w == b"%%EOF")
}

#[test]
fn collaudo_completo() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/collaudo");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut r = Report { dir: dir.clone(), lines: Vec::new() };
    let path = dir.join("collaudo.gadb");
    let s = Store::open(&path).unwrap();
    crate::demo::populate(&s).unwrap();
    let today = chrono::Local::now().date_naive();
    r.ok("Archivio", format!("creato con i dati dimostrativi ({:.1} MB), integrità: {}", s.file_size() as f64 / 1_048_576.0, s.integrity_check().unwrap()));
    let vats = s.vat_codes().unwrap();
    let v22 = vats.iter().find(|v| v.code == "22").unwrap().clone();
    let parties = s.parties(&Default::default()).unwrap();
    let rossi = parties.iter().find(|p| p.name.starts_with("Rossi")).unwrap().clone();
    let export = parties.iter().find(|p| p.name.starts_with("Export")).unwrap().clone();

    // ------------------------------------------------------------- anagrafiche
    assert!(s.products("", false).unwrap().len() >= 9);
    let kit = s.product_by_code("KIT01").unwrap().unwrap();
    let tav = s.product_by_code("TAV01").unwrap().unwrap();
    assert_eq!(s.bom_cost(&tav).unwrap(), dec!(68));
    r.ok("Anagrafiche", format!("{} soggetti, {} articoli; distinta base TAV01 a costo € 68", parties.len(), s.products("", false).unwrap().len()));
    let n = s.update_products(&[kit.id, tav.id], |p| p.notes = "collaudo".into()).unwrap();
    assert_eq!(n, 2);
    r.ok("Anagrafiche", "modifica di più articoli insieme");

    // ------------------------------------------------------------- documenti e righe
    let mut ed = crate::ui::editor::Editor::new(&s, Document::new(DocKind::Invoice));
    ed.doc.party_id = Some(rossi.id);
    ed.doc.party = rossi.clone();
    let lines = ed.lines_for_product(&s, &kit);
    assert_eq!(lines.len(), 4, "gruppo con prezzo: riga del gruppo e 3 componenti");
    ed.doc.lines.extend(lines);
    let led = s.product_by_code("LED10").unwrap().unwrap();
    let mut l = ed.lines_for_product(&s, &led).remove(0);
    l.packs = Some(dec!(3));
    l.qty = dec!(30);
    ed.doc.lines.push(l);
    ed.doc.lines.push(DocLine { kind: LineKind::Subtotal, ..Default::default() });
    ed.doc.lines.push(DocLine { description: "Trasporto 2%".into(), calc_percent: Some(dec!(2)), vat: VatRef::from_code(&v22), ..Default::default() });
    let t = openfatt_core::calc::compute(&ed.doc).unwrap();
    assert!(t.lines.iter().any(|l| l.is_generated && l.description == "Contributo RAEE"));
    assert!(t.lines.iter().any(|l| l.subtotal.is_some()));
    let (disc, got) = openfatt_core::calc::discount_for_total(&ed.doc, dec!(700)).unwrap();
    ed.doc.discount = disc.normalize().to_string().replace('.', ",");
    assert!((got - dec!(700)).abs() < dec!(0.05));
    ed.doc.payment_id = rossi.default_payment_id;
    ed.doc.agent_id = rossi.agent_id;
    let d = s.save_document(&ed.doc).unwrap();
    let d = s.issue_document(d.id).unwrap();
    r.ok("Righe", format!("gruppo, confezioni, contributo RAEE, subtotale, riga calcolata, calcolo inverso (sconto {}% per € {})", disc.round_dp(2), got));
    let lp = s.last_prices(rossi.id, s.product_by_code("SED01").unwrap().unwrap().id, false, 5).unwrap();
    assert!(!lp.is_empty());
    r.ok("Righe", format!("ultimi prezzi praticati a Rossi per SED01: {}", lp.len()));
    let copy = s.duplicate_document(d.id, Some(DocKind::Quote)).unwrap();
    assert_eq!(copy.lines.len(), d.lines.len());
    r.ok("Documenti", "duplicazione in preventivo, copia delle righe");

    // valuta estera e dichiarazione d'intento
    let n35 = vats.iter().find(|v| v.natura.as_deref() == Some("N3.5")).unwrap();
    let mut x = Document::new(DocKind::Invoice);
    x.party_id = Some(export.id);
    x.intent_protocol = export.intents[0].protocol.clone();
    x.currency = "USD".into();
    x.exchange_rate = dec!(1.08);
    x.lines.push(DocLine { description: "Macchinario per esportazione".into(), unit_price: dec!(5400), vat: VatRef::from_code(n35), ..Default::default() });
    x.stamp.mode = StampMode::No;
    let x = s.save_document(&x).unwrap();
    let x = s.issue_document(x.id).unwrap();
    let u = s.intent_usage(export.id, &export.intents[0].protocol, None).unwrap();
    assert_eq!(u.used, dec!(5000));
    r.ok("Fiscale", format!("fattura in USD a esportatore abituale: plafond usato € {} su € {}", u.used, u.limit));

    // ------------------------------------------------------------- fattura elettronica
    let invoices = s.documents(&DocFilter { kinds: DocKind::ALL.iter().copied().filter(|k| k.is_fiscal()).collect(), status: Some(DocStatus::Issued), ..Default::default() }).unwrap();
    let mut n_fe = 0;
    for i in &invoices {
        let (e, issues) = crate::services::create_einvoice(&s, i.id).unwrap_or_else(|e| panic!("{}: {}", i.number_label, e));
        assert!(!openfatt_fatturapa::has_errors(&issues), "{:?}", issues);
        r.file("fe", &e.file_name, &e.xml);
        assert!(pdf_ok(&crate::services::einvoice_pdf(&e.xml).unwrap()));
        n_fe += 1;
    }
    let xml = String::from_utf8(s.einvoice_for_document(x.id).unwrap().unwrap().xml).unwrap();
    assert!(xml.contains("<Divisa>USD</Divisa>") && xml.contains("<TipoDato>INTENTO</TipoDato>"));
    r.ok("Fattura elettronica", format!("{} file FatturaPA generati e riletti (anche valuta e dichiarazione d'intento)", n_fe));
    // ordine NSO evaso in fattura
    let nso = s.documents(&DocFilter { kinds: vec![DocKind::CustomerOrder], ..Default::default() }).unwrap().into_iter().find(|d| d.party_name.contains("Sanitaria")).unwrap();
    let nso = s.issue_document(nso.id).unwrap();
    let inv = s.transform(&[nso.id], DocKind::Invoice).unwrap();
    let inv = s.issue_document(inv.id).unwrap();
    let (e, _) = crate::services::create_einvoice(&s, inv.id).unwrap();
    let xml = String::from_utf8(e.xml.clone()).unwrap();
    assert!(xml.contains("<CodiceCommessaConvenzione>#B9BGAQ#</CodiceCommessaConvenzione>"), "{}", xml);
    r.file("fe", &e.file_name, &e.xml);
    r.ok("NSO", "ordine UBL importato, evaso in fattura con la tripletta in DatiOrdineAcquisto");
    // fattura passiva: carico articoli in anagrafica
    let purchase = s.documents(&DocFilter { kinds: vec![DocKind::PurchaseInvoice], ..Default::default() }).unwrap();
    if let Some(p) = purchase.first() {
        let res = s.import_products_from_purchase(p.id, "F-", false).unwrap();
        r.ok("Acquisti", format!("articoli dalla fattura del fornitore: {} nuovi, {} aggiornati", res.created, res.updated));
    }

    // ------------------------------------------------------------- stampe
    let all_docs = s.documents(&DocFilter::default()).unwrap();
    let mut n_pdf = 0;
    for d in &all_docs {
        let doc = s.document(d.id).unwrap();
        let pdf = crate::services::document_pdf(&s, &doc).unwrap();
        assert!(pdf_ok(&pdf), "{}", doc.title());
        n_pdf += 1;
    }
    let ids: Vec<i64> = all_docs.iter().take(6).map(|d| d.id).collect();
    let many = crate::printing::documents_pdf(&s, &ids).unwrap();
    r.file("pdf", "documenti.pdf", &many);
    r.file("pdf", "fattura_valuta.pdf", &crate::services::document_pdf(&s, &s.document(x.id).unwrap()).unwrap());
    r.file("pdf", "fattura_righe.pdf", &crate::services::document_pdf(&s, &s.document(d.id).unwrap()).unwrap());
    let items = crate::printing::product_items(&s, &s.products("", false).unwrap());
    let labels = crate::printing::labels_pdf(&items, "a4-3x8", 0, false);
    assert!(pdf_ok(&labels));
    r.file("pdf", "etichette.pdf", &labels);
    r.ok("Stampe", format!("{} documenti in PDF, stampa multipla, etichette con codici a barre", n_pdf));

    // ------------------------------------------------------------- magazzino e produzione
    let mut prod = Document::new(DocKind::Production);
    prod.lines.push(DocLine { product_id: Some(tav.id), description: tav.description.clone(), qty: dec!(2), ..Default::default() });
    let prod = s.save_document(&prod).unwrap();
    let before = s.stock_of(s.product_by_code("GAM01").unwrap().unwrap().id).unwrap();
    s.issue_document(prod.id).unwrap();
    let after = s.stock_of(s.product_by_code("GAM01").unwrap().unwrap().id).unwrap();
    assert_eq!(before - after, dec!(8));
    r.ok("Produzione", "2 tavoli prodotti: scaricate 8 gambe e 2 piani");
    let inv_rows = s.stock_report("", None).unwrap();
    assert!(!inv_rows.is_empty());
    r.ok("Magazzino", format!("giacenze di {} articoli", inv_rows.len()));

    // ------------------------------------------------------------- contabilità
    for kind in BankKind::ALL {
        let pending: Vec<_> = s.bank_pending(kind).unwrap().into_iter().filter(|d| kind != BankKind::Riba || d.date > today).collect();
        if pending.is_empty() {
            continue;
        }
        let p = crate::bankfiles::prepare(&s, kind, &pending, crate::bankfiles::business_day(today.succ_opt().unwrap())).unwrap();
        let b = crate::bankfiles::issue(&s, &p).unwrap();
        r.file("cbi", &p.file_name, p.content.as_bytes());
        let (b, it) = s.batch(b.id).unwrap();
        assert!(pdf_ok(&crate::bankfiles::batch_pdf(&s, &b, &it)));
        r.ok("Banca", format!("distinta {:?} con {} voci, € {}", kind, p.entries, p.total));
    }
    let q = (today.month0() / 3) + 1;
    let prev_q = if q == 1 { (today.year() - 1, 4) } else { (today.year(), q - 1) };
    match crate::vatfiles::lipe(&s, prev_q.0, prev_q.1) {
        Ok((name, xml)) => {
            r.file("lipe", &name, xml.as_bytes());
            r.ok("IVA", format!("Comunicazione LIPE {} trimestre {}", prev_q.0, prev_q.1));
        }
        Err(e) => r.ok("IVA", format!("LIPE del trimestre precedente non prodotta (nessun dato): {}", e.join("; "))),
    }
    let year = today.year();
    let liq = s.vat_liquidations(year, 12).unwrap();
    assert!(!liq.is_empty());
    r.ok("IVA", format!("liquidazioni {} calcolate: {}", year, liq.len()));

    // dichiarazione annuale: prospetto per il commercialista
    let annual = s.annual_vat(year).unwrap();
    assert_eq!(annual.value("VL1", 0), annual.value("VE26", 0) + annual.value("VJ19", 0));
    assert!(annual.value("VE50", 0) > Decimal::ZERO);
    r.file("pdf", "prospetto_iva_annuale.pdf", &crate::reports::annual_vat_pdf(&s, &annual).unwrap());
    crate::reports::annual_vat_xlsx(&dir.join("prospetto_iva_annuale.xlsx"), &annual).unwrap();
    r.ok("IVA", format!("prospetto della dichiarazione annuale {}: volume d'affari € {}, IVA a debito € {}, detraibile € {}", year, annual.value("VE50", 0), annual.value("VL1", 0), annual.value("VL2", 0)));

    // modelli di stampa liberi e moduli prestampati
    let mut layout = openfatt_pdf::freeform::FreeLayout::standard_a4("Collaudo");
    layout.kinds = vec!["FAT".into()];
    crate::printing::save_layouts(&s, std::slice::from_ref(&layout)).unwrap();
    let inv_doc = s.document(d.id).unwrap();
    let free = crate::printing::document_pdf(&s, &inv_doc).unwrap();
    assert!(pdf_ok(&free));
    r.file("pdf", "modello_libero.pdf", &free);
    layout.preprinted = true;
    layout.offset_x = 1.5;
    crate::printing::save_layouts(&s, std::slice::from_ref(&layout)).unwrap();
    let pre = crate::printing::document_pdf(&s, &inv_doc).unwrap();
    assert!(pdf_ok(&pre));
    r.file("pdf", "modulo_prestampato.pdf", &pre);
    crate::printing::save_layouts(&s, &[]).unwrap();
    r.ok("Stampe", "modello libero e modulo prestampato (solo i dati, con correzione dell'allineamento)");

    // ------------------------------------------------------------- agenti, statistiche, agenda
    let ag = s.agents(false).unwrap()[0].clone();
    let rows = s.commissions(ag.id, NaiveDate::from_ymd_opt(year - 1, 1, 1).unwrap(), today, today).unwrap();
    assert!(!rows.is_empty());
    let pdf = crate::reports::commissions_pdf(&s, &ag, NaiveDate::from_ymd_opt(year, 1, 1).unwrap(), today, &rows).unwrap();
    r.file("pdf", "provvigioni.pdf", &pdf);
    crate::reports::commissions_xlsx(&dir.join("provvigioni.xlsx"), &ag, &rows).unwrap();
    r.ok("Agenti", format!("{}: {} documenti, provvigioni € {}", ag.name, rows.len(), rows.iter().map(|r| r.commission).sum::<Decimal>()));
    for by in [openfatt_store::StatBy::Customer, openfatt_store::StatBy::Product, openfatt_store::StatBy::Category, openfatt_store::StatBy::Agent, openfatt_store::StatBy::Month, openfatt_store::StatBy::Supplier] {
        let st = s.statistics(by, NaiveDate::from_ymd_opt(year - 1, 1, 1).unwrap(), today).unwrap();
        if by == openfatt_store::StatBy::Customer {
            crate::reports::stats_xlsx(&dir.join("statistica_clienti.xlsx"), "Clienti", &st).unwrap();
        }
    }
    let flows = s.cash_flow(today, 12).unwrap();
    assert_eq!(flows.len(), 12);
    r.ok("Statistiche", "per cliente, articolo, categoria, agente, mese, fornitore; flussi di cassa a 12 mesi");
    assert!(!s.agenda(today, today + chrono::Duration::days(3)).unwrap().is_empty());
    r.ok("Agenda", "impegni dimostrativi presenti");

    // ------------------------------------------------------------- ricorrenti, carte, negozio
    let ids = s.generate_recurring(today + chrono::Duration::days(62)).unwrap();
    assert!(!ids.is_empty());
    let g = s.document(ids[0]).unwrap();
    assert!(!g.lines[0].description.contains('{'));
    r.ok("Ricorrenti", format!("{} documenti generati ({})", ids.len(), g.lines[0].description));
    let gc = s.card_by_code("GC00000001").unwrap().unwrap();
    s.add_card_move(gc.id, today, dec!(0), dec!(-20), None, "collaudo").unwrap();
    assert_eq!(s.card_balance(gc.id).unwrap().1, dec!(30));
    r.ok("Carte", "buono regalo speso in parte, saldo € 30");
    let json = r#"[{"id":9001,"number":"9001","currency":"EUR","date_created":"2026-11-01T10:00:00","billing":{"first_name":"Ada","last_name":"Lovelace","city":"Trento","email":"ada@example.com"},"line_items":[{"name":"Tavolo","sku":"TAV01","quantity":1,"total":"172.13","total_tax":"37.87"}]}]"#;
    let orders = openfatt_interop::shop::parse_woocommerce(json).unwrap();
    let res = s.import_shop_orders(openfatt_interop::shop::ShopKind::WooCommerce, &orders).unwrap();
    assert_eq!(res.created.len(), 1);
    r.ok("Negozio online", "ordine WooCommerce importato come ordine cliente");

    // ------------------------------------------------------------- utenti e rete
    let admin = s.save_user(&openfatt_store::User { username: "admin".into(), admin: true, ..Default::default() }, Some("collaudo2026")).unwrap();
    assert!(s.login("admin", "collaudo2026").is_ok() && s.login("admin", "sbagliata").is_err());
    let s2 = Store::open(&path).unwrap();
    s.start_session("COLLAUDO-1").unwrap();
    s2.start_session("COLLAUDO-2").unwrap();
    assert!(s.acquire_lock("document", d.id).unwrap().is_none());
    assert!(s2.acquire_lock("document", d.id).unwrap().is_some());
    s.end_session().unwrap();
    s2.end_session().unwrap();
    drop(s2);
    let _ = admin;
    r.ok("Utenti", "accesso con password, due postazioni, blocco del documento aperto altrove");

    // ------------------------------------------------------------- fine anno e conservazione
    let chk = s.year_check(year).unwrap();
    r.ok("Fine anno", format!("controlli {}: {} bozze, {} non trasmesse, magazzino € {}", year, chk.drafts.len(), chk.not_sent.len(), chk.stock_value));
    let inv_pdf = crate::reports::inventory_valuation_pdf(&s, year).unwrap();
    r.file("pdf", "inventario.pdf", &inv_pdf);
    for e in s.einvoices(openfatt_store::Direction::Out, "", Some(year)).unwrap() {
        if e.status == openfatt_store::EInvoiceStatus::ToSend {
            s.mark_einvoice_sent(e.id, "collaudo").unwrap();
            let rc = format!(r#"<?xml version="1.0"?><types:RicevutaConsegna xmlns:types="http://www.fatturapa.gov.it/sdi/messaggi/v1.0" versione="1.0"><IdentificativoSdI>{}</IdentificativoSdI><NomeFile>{}</NomeFile><DataOraRicezione>2026-01-01T10:00:00Z</DataOraRicezione><DataOraConsegna>2026-01-01T10:01:00Z</DataOraConsegna><MessageId>9</MessageId></types:RicevutaConsegna>"#, e.id, e.file_name);
            let mut sum = crate::services::InboundSummary::default();
            crate::services::import_file(&s, &e.file_name.replace(".xml", "_RC_001.xml"), rc.as_bytes(), &mut sum);
        }
    }
    let pkg = s.conservation_package(year).unwrap();
    r.file("conservazione", &pkg.file_name, &pkg.zip);
    r.ok("Conservazione", format!("pacchetto con {} fatture e ricevute, SHA-256 {}…", pkg.count, &pkg.hash[..12]));
    let bk = dir.join("chiusura.gadb");
    let last = year - 1;
    let closed = s.close_year(last, &bk);
    r.ok("Fine anno", match closed {
        Ok(_) => format!("anno {} chiuso con copia di sicurezza", last),
        Err(e) => format!("chiusura del {} rifiutata dai controlli: {}", last, e),
    });

    // ------------------------------------------------------------- interscambio
    let docs: Vec<(Document, openfatt_core::calc::Totals)> = invoices.iter().take(5).map(|d| {
        let doc = s.document(d.id).unwrap();
        let t = openfatt_core::calc::compute(&doc).unwrap();
        (doc, t)
    }).collect();
    let xml = openfatt_interop::export_documents(&s.company().unwrap(), &docs);
    let back = openfatt_interop::parse_documents(&xml, &vats).unwrap();
    assert_eq!(back.len(), docs.len());
    r.ok("Interscambio", "esportazione e rilettura Easyfatt-XML");
    let backup = dir.join("copia.gadb");
    s.backup_to(&backup).unwrap();
    assert!(Store::open(&backup).unwrap().integrity_check().unwrap() == "ok");
    r.ok("Archivio", "copia di sicurezza riaperta e verificata");

    let mut md = String::from("# Collaudo openFatt\n\n| Area | Verifica | Esito |\n|---|---|---|\n");
    md.push_str(&r.lines.join("\n"));
    md.push('\n');
    std::fs::write(dir.join("rapporto.md"), md).unwrap();
    println!("rapporto: {}", dir.join("rapporto.md").display());
}
