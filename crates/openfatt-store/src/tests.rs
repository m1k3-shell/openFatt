//! Test basati sulle fixture F-xx della specifica di lavoro (SPECIFICA_EQUIVALENTE.md §8).

use chrono::NaiveDate;
use openfatt_core::dec;
use openfatt_core::model::*;

use super::*;

fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, dd).unwrap()
}

fn setup() -> (Store, Party, Product, VatRef) {
    let s = Store::open_in_memory().unwrap();
    let c = s
        .save_party(&Party { name: "Cliente Uno S.r.l.".into(), vat_number: "12345678903".into(), address: "Via Roma 1".into(), zip: "38122".into(), city: "Trento".into(), province: "TN".into(), sdi_code: "ABCDEFG".into(), ..Default::default() })
        .unwrap();
    let v = s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap();
    let p = s.save_product(&Product { code: "A1".into(), description: "Articolo uno".into(), price: dec!(10), vat_id: Some(v.id), cost: dec!(6), ..Default::default() }).unwrap();
    (s, c, p, VatRef::from_code(&v))
}

fn line(p: &Product, qty: rust_decimal::Decimal, vat: &VatRef) -> DocLine {
    DocLine { product_id: Some(p.id), code: p.code.clone(), description: p.description.clone(), qty, unit_price: p.price, vat: vat.clone(), ..Default::default() }
}

fn new_doc(s: &Store, kind: DocKind, party: &Party, date: NaiveDate, lines: Vec<DocLine>) -> Document {
    let mut doc = Document::new(kind);
    doc.date = date;
    doc.party_id = Some(party.id);
    doc.lines = lines;
    s.save_document(&doc).unwrap()
}

#[test]
fn numbering_is_progressive_and_chronological() {
    let (s, c, p, v) = setup();
    let a = new_doc(&s, DocKind::Invoice, &c, d(2026, 1, 10), vec![line(&p, dec!(1), &v)]);
    let a = s.issue_document(a.id).unwrap();
    assert_eq!(a.number, Some(1));
    // la nota di credito condivide il registro vendite
    let nc = new_doc(&s, DocKind::CreditNote, &c, d(2026, 1, 11), vec![line(&p, dec!(1), &v)]);
    assert_eq!(s.issue_document(nc.id).unwrap().number, Some(2));
    // data anteriore all'ultima emessa: rifiutata
    let b = new_doc(&s, DocKind::Invoice, &c, d(2026, 1, 5), vec![line(&p, dec!(1), &v)]);
    assert!(matches!(s.issue_document(b.id), Err(StoreError::Invalid(_))));
    // un DDT ha un registro proprio
    let ddt = new_doc(&s, DocKind::DeliveryNote, &c, d(2026, 1, 1), vec![line(&p, dec!(1), &v)]);
    assert_eq!(s.issue_document(ddt.id).unwrap().number, Some(1));
    // nuovo anno: si riparte da 1
    let n = new_doc(&s, DocKind::Invoice, &c, d(2027, 1, 2), vec![line(&p, dec!(1), &v)]);
    assert_eq!(s.issue_document(n.id).unwrap().number, Some(1));
}

#[test]
fn f02_two_ddt_grouped_into_invoice_without_second_unload() {
    let (s, c, p, v) = setup();
    s.add_manual_stock_move(&StockMove { product_id: p.id, qty: dec!(10), reason: "Carico iniziale".into(), date: d(2026, 1, 1), ..Default::default() }).unwrap();
    let d1 = new_doc(&s, DocKind::DeliveryNote, &c, d(2026, 2, 1), vec![line(&p, dec!(2), &v)]);
    let d1 = s.issue_document(d1.id).unwrap();
    let d2 = new_doc(&s, DocKind::DeliveryNote, &c, d(2026, 2, 10), vec![line(&p, dec!(3), &v)]);
    let d2 = s.issue_document(d2.id).unwrap();
    assert_eq!(s.stock_of(p.id).unwrap(), dec!(5));

    let inv = s.transform(&[d1.id, d2.id], DocKind::Invoice).unwrap();
    assert_eq!(inv.fe_type, "TD24");
    let qty: rust_decimal::Decimal = inv.lines.iter().filter(|l| l.kind == LineKind::Item).map(|l| l.qty).sum();
    assert_eq!(qty, dec!(5));
    let mut inv = inv;
    inv.date = d(2026, 2, 28);
    let inv = s.save_document(&inv).unwrap();
    let inv = s.issue_document(inv.id).unwrap();
    // nessun secondo scarico
    assert_eq!(s.stock_of(p.id).unwrap(), dec!(5));
    let ddts = s.ddt_sources(&inv).unwrap();
    assert_eq!(ddts.len(), 2);
    // i DDT risultano evasi: una seconda fatturazione non ha righe
    assert!(s.transform(&[d1.id], DocKind::Invoice).is_err());
}

#[test]
fn f03_partial_fulfillment() {
    let (s, c, p, v) = setup();
    let ord = new_doc(&s, DocKind::CustomerOrder, &c, d(2026, 3, 1), vec![line(&p, dec!(10), &v)]);
    let ord = s.issue_document(ord.id).unwrap();
    let mut ddt1 = s.transform(&[ord.id], DocKind::DeliveryNote).unwrap();
    ddt1.lines[0].qty = dec!(4);
    let ddt1 = s.save_document(&ddt1).unwrap();
    s.issue_document(ddt1.id).unwrap();
    assert_eq!(s.fulfilled_qty(ord.lines[0].id).unwrap(), dec!(4));
    assert_eq!(s.fulfillment_state(&ord).unwrap(), (false, true));
    let ddt2 = s.transform(&[ord.id], DocKind::DeliveryNote).unwrap();
    assert_eq!(ddt2.lines[0].qty, dec!(6));
    s.issue_document(ddt2.id).unwrap();
    assert_eq!(s.fulfillment_state(&ord).unwrap(), (true, false));
    // l'ordine evaso non è più in impegno
    let report = s.stock_report("", None).unwrap();
    assert_eq!(report[0].ordered_by_customers, dec!(0));
}

#[test]
fn f04_f05_payments_and_writeoff() {
    let (s, c, _p, v) = setup();
    let pay = s.payment_methods().unwrap().into_iter().find(|p| p.name.starts_with("RiBa")).unwrap();
    let mut doc = Document::new(DocKind::Invoice);
    doc.date = d(2026, 1, 15);
    doc.party_id = Some(c.id);
    doc.payment_id = Some(pay.id);
    doc.lines.push(DocLine { description: "Servizio".into(), qty: dec!(1), unit_price: dec!(81.97), vat: v.clone(), ..Default::default() });
    let doc = s.save_document(&doc).unwrap();
    let doc = s.issue_document(doc.id).unwrap();
    assert_eq!(doc.due_dates.len(), 2);
    let total: rust_decimal::Decimal = doc.due_dates.iter().map(|x| x.amount).sum();
    assert_eq!(total, dec!(100.00)); // 81,97 + 18,03
    // F-04: incasso 30 sulla prima rata, poi il saldo
    let first = &doc.due_dates[0];
    s.pay_due(first.id, d(2026, 2, 28), dec!(30), "Banca", false).unwrap();
    s.pay_due(first.id, d(2026, 3, 2), first.amount - dec!(30), "Banca", false).unwrap();
    // F-05: seconda rata incassata con 1 euro di abbuono
    let second = &doc.due_dates[1];
    s.pay_due(second.id, d(2026, 3, 31), second.amount - dec!(1), "Banca", true).unwrap();
    let open = s.due_list(true, None).unwrap();
    assert!(open.is_empty());
    let cash: rust_decimal::Decimal = s.cash_entries(None, None, Some("Banca")).unwrap().iter().map(|e| e.amount).sum();
    assert_eq!(cash, dec!(99.00));
    let row = s.due_list(false, None).unwrap().into_iter().find(|r| r.id == second.id).unwrap();
    assert_eq!(row.writeoff, dec!(1.00));
    // documento con pagamenti: non modificabile
    assert!(s.save_document(&s.document(doc.id).unwrap()).is_err());
    // eccedenza rifiutata
    assert!(s.pay_due(first.id, d(2026, 4, 1), dec!(1), "Banca", false).is_err());
}

#[test]
fn f11_duplicate_notification_is_ignored() {
    let (s, c, p, v) = setup();
    let mut company = Company { name: "Azienda".into(), vat_number: "06363391001".into(), address: "Via X".into(), zip: "00100".into(), city: "Roma".into(), province: "RM".into(), ..Default::default() };
    company.regime_fiscale = "RF01".into();
    s.set_company(&company).unwrap();
    let doc = new_doc(&s, DocKind::Invoice, &c, d(2026, 5, 1), vec![line(&p, dec!(1), &v)]);
    let doc = s.issue_document(doc.id).unwrap();
    let t = openfatt_core::calc::compute(&doc).unwrap();
    let g = openfatt_fatturapa::generate(&company, &doc, &t, &openfatt_fatturapa::BuildOptions { progressivo: "00001".into(), ..Default::default() }).unwrap();
    assert!(g.is_sendable(), "{:?}", g.issues);
    let e = s.create_outgoing_einvoice(&doc, "IT06363391001_00001.xml", &g.xml, &g.fattura).unwrap();
    s.mark_einvoice_sent(e.id, "PEC").unwrap();
    // il documento trasmesso non è più modificabile
    assert!(s.document_lock_reason(doc.id).unwrap().is_some());
    let rc = r#"<?xml version="1.0"?><types:RicevutaConsegna xmlns:types="http://www.fatturapa.gov.it/sdi/messaggi/v1.0" versione="1.0">
      <IdentificativoSdI>999</IdentificativoSdI><NomeFile>IT06363391001_00001.xml</NomeFile>
      <DataOraRicezione>2026-05-01T10:00:00Z</DataOraRicezione><DataOraConsegna>2026-05-01T10:05:00Z</DataOraConsegna>
      <MessageId>1</MessageId></types:RicevutaConsegna>"#;
    let n = openfatt_fatturapa::notify::parse_notification(rc).unwrap();
    assert_eq!(s.apply_notification("IT06363391001_00001_RC_001.xml", &n, rc.as_bytes()).unwrap(), Some(e.id));
    assert_eq!(s.apply_notification("IT06363391001_00001_RC_001.xml", &n, rc.as_bytes()).unwrap(), None);
    let e = s.einvoice(e.id).unwrap();
    assert_eq!(e.status, EInvoiceStatus::Delivered);
    assert_eq!(e.sdi_id, "999");
    assert_eq!(s.einvoice_events(e.id).unwrap().len(), 1);
}

#[test]
fn incoming_invoice_registration() {
    let s = Store::open_in_memory().unwrap();
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/esempi/IT01234567890_FPR01.xml");
    let Ok(bytes) = std::fs::read(&path) else { return };
    let (f, xml) = openfatt_fatturapa::read_any(&bytes).unwrap();
    let id = s.import_incoming("IT01234567890_FPR01.xml", &xml, None, &f, "123").unwrap().unwrap();
    // seconda importazione dello stesso file: ignorata
    assert!(s.import_incoming("IT01234567890_FPR01.xml", &xml, None, &f, "123").unwrap().is_none());
    let docs = s.register_incoming(id).unwrap();
    assert_eq!(docs.len(), 1);
    let doc = s.document(docs[0]).unwrap();
    assert_eq!(doc.kind, DocKind::PurchaseInvoice);
    assert_eq!(doc.status, DocStatus::Issued);
    let t = openfatt_core::calc::compute(&doc).unwrap();
    assert_eq!(t.total, openfatt_fatturapa::to_domain::declared_total(&f.bodies[0]));
    assert!(s.party(doc.party_id.unwrap()).unwrap().unwrap().role.is_supplier());
    assert_eq!(s.einvoice(id).unwrap().status, EInvoiceStatus::Registered);
}

#[test]
fn f13_backup_and_reopen() {
    let dir = std::env::temp_dir().join(format!("ga-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("azienda.gadb");
    let bak = dir.join("backup.gadb");
    let _ = std::fs::remove_file(&src);
    {
        let s = Store::open(&src).unwrap();
        s.save_party(&Party { name: "Prova".into(), ..Default::default() }).unwrap();
        s.backup_to(&bak).unwrap();
    }
    let r = Store::open(&bak).unwrap();
    assert_eq!(r.integrity_check().unwrap(), "ok");
    assert_eq!(r.parties(&PartyFilter::default()).unwrap().len(), 1);
    drop(r);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn f17_concurrent_save_conflict() {
    let (s, c, _p, _v) = setup();
    let a = s.party(c.id).unwrap().unwrap();
    let b = a.clone();
    let mut a2 = a.clone();
    a2.city = "Bolzano".into();
    s.save_party(&a2).unwrap();
    let mut b2 = b;
    b2.city = "Verona".into();
    assert!(matches!(s.save_party(&b2), Err(StoreError::Conflict(_))));
    assert_eq!(s.party(c.id).unwrap().unwrap().city, "Bolzano");
}

#[test]
fn f18_cancelled_source_cannot_be_transformed() {
    let (s, c, p, v) = setup();
    let q = new_doc(&s, DocKind::Quote, &c, d(2026, 1, 1), vec![line(&p, dec!(1), &v)]);
    s.cancel_document(q.id).unwrap();
    assert!(s.transform(&[q.id], DocKind::Invoice).is_err());
    // nessun documento creato
    assert_eq!(s.documents(&DocFilter { kinds: vec![DocKind::Invoice], ..Default::default() }).unwrap().len(), 0);
}

#[test]
fn delete_only_last_fiscal_number() {
    let (s, c, p, v) = setup();
    let a = new_doc(&s, DocKind::Invoice, &c, d(2026, 1, 1), vec![line(&p, dec!(1), &v)]);
    s.issue_document(a.id).unwrap();
    let b = new_doc(&s, DocKind::Invoice, &c, d(2026, 1, 2), vec![line(&p, dec!(1), &v)]);
    s.issue_document(b.id).unwrap();
    assert!(s.delete_document(a.id).is_err());
    s.delete_document(b.id).unwrap();
    s.delete_document(a.id).unwrap();
}

#[test]
fn credit_note_from_invoice_restocks() {
    let (s, c, p, v) = setup();
    let inv = new_doc(&s, DocKind::Invoice, &c, d(2026, 1, 1), vec![line(&p, dec!(3), &v)]);
    let inv = s.issue_document(inv.id).unwrap();
    assert_eq!(s.stock_of(p.id).unwrap(), dec!(-3));
    let nc = s.transform(&[inv.id], DocKind::CreditNote).unwrap();
    assert_eq!(nc.related.iter().filter(|r| r.kind == "FATTURA").count(), 1);
    s.issue_document(nc.id).unwrap();
    assert_eq!(s.stock_of(p.id).unwrap(), dec!(0));
}

#[test]
fn rollback_on_failure_leaves_no_partial_data() {
    let (s, c, p, v) = setup();
    let a = new_doc(&s, DocKind::Invoice, &c, d(2026, 1, 10), vec![line(&p, dec!(1), &v)]);
    s.issue_document(a.id).unwrap();
    let mut b = Document::new(DocKind::Invoice);
    b.date = d(2026, 1, 11);
    b.party_id = Some(c.id);
    b.number = Some(1); // numero già usato
    b.lines.push(line(&p, dec!(1), &v));
    let b = s.save_document(&b);
    assert!(b.is_err());
    // nessun movimento di magazzino orfano
    assert_eq!(s.stock_moves(Some(p.id), 100).unwrap().len(), 1);
}

#[test]
fn reminders_group_overdue_dues_and_raise_the_level() {
    let (s, c, _p, v) = setup();
    let mut c = c;
    c.email = "amministrazione@cliente-uno.example".into();
    let c = s.save_party(&c).unwrap();
    let pay = s.payment_methods().unwrap().into_iter().find(|p| p.name.starts_with("RiBa")).unwrap();
    let mut doc = Document::new(DocKind::Invoice);
    doc.date = d(2026, 1, 15);
    doc.party_id = Some(c.id);
    doc.payment_id = Some(pay.id);
    doc.lines.push(DocLine { description: "Servizio".into(), qty: dec!(1), unit_price: dec!(81.97), vat: v.clone(), ..Default::default() });
    let doc = s.issue_document(s.save_document(&doc).unwrap().id).unwrap();
    let today = chrono::Local::now().date_naive();
    // entro la tolleranza nulla da sollecitare
    assert!(s.reminder_candidates(d(2026, 3, 2), 5, 7).unwrap().is_empty());
    let list = s.reminder_candidates(today, 5, 7).unwrap();
    assert_eq!(list.len(), 1);
    let r = &list[0];
    assert_eq!((r.dues.len(), r.overdue_total, r.next_level, r.email.as_str()), (2, dec!(100.00), 1, "amministrazione@cliente-uno.example"));
    assert!(r.last_reminder.is_none() && !r.recently_reminded);
    // un invio fallito non conta; uno riuscito alza il livello
    let mut e = MailLogEntry { kind: MailKind::Reminder, party_id: Some(c.id), level: 1, recipients: r.email.clone(), subject: "Promemoria".into(), status: MailStatus::Failed, due_ids: r.dues.iter().map(|d| d.id).collect(), ..Default::default() };
    s.log_mail(&e).unwrap();
    assert_eq!(s.reminder_candidates(today, 5, 7).unwrap()[0].next_level, 1);
    e.status = MailStatus::Sent;
    s.log_mail(&e).unwrap();
    let r = &s.reminder_candidates(today, 5, 7).unwrap()[0];
    assert_eq!(r.next_level, 2);
    assert!(r.recently_reminded);
    assert_eq!(r.last_reminder.map(|x| x.1), Some(1));
    let log = s.mail_log(Some(c.id), None, 10).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].due_ids.len(), 2);
    // incassata la prima rata resta solo la seconda
    s.pay_due(doc.due_dates[0].id, today, doc.due_dates[0].amount, "Banca", false).unwrap();
    let r = &s.reminder_candidates(today, 5, 7).unwrap()[0];
    assert_eq!(r.dues.len(), 1);
    // invio del documento registrato e ritrovato
    s.log_mail(&MailLogEntry { kind: MailKind::Document, party_id: Some(c.id), document_id: Some(doc.id), recipients: "x@y.it".into(), subject: "Fattura".into(), ..Default::default() }).unwrap();
    assert_eq!(s.document_last_mail(doc.id).unwrap().unwrap().recipients, "x@y.it");
}

#[test]
fn bank_batch_lifecycle() {
    let (s, c, _p, v) = setup();
    let pay = s.payment_methods().unwrap().into_iter().find(|p| p.name.starts_with("RiBa")).unwrap();
    let mut doc = Document::new(DocKind::Invoice);
    doc.date = d(2026, 1, 15);
    doc.party_id = Some(c.id);
    doc.payment_id = Some(pay.id);
    doc.lines.push(DocLine { description: "Servizio".into(), qty: dec!(1), unit_price: dec!(81.97), vat: v.clone(), ..Default::default() });
    let doc = s.issue_document(s.save_document(&doc).unwrap().id).unwrap();
    // la SDD seminata esiste; nessun bonifico o SDD da presentare
    assert!(s.payment_methods().unwrap().iter().any(|p| p.mp_code == "MP19"));
    assert!(s.bank_pending(BankKind::Sdd).unwrap().is_empty());
    let pending = s.bank_pending(BankKind::Riba).unwrap();
    assert_eq!(pending.len(), 2);
    let items: Vec<_> = pending.iter().map(|d| (d.id, d.residual(), format!("R{}", d.id))).collect();
    let n = s.next_batch_number(BankKind::Riba, 2026).unwrap();
    assert_eq!(n, 1);
    // numero sbagliato rifiutato
    assert!(s.create_batch(BankKind::Riba, 2026, 5, d(2026, 2, 28), "x.txt", "...", &items).is_err());
    let b = s.create_batch(BankKind::Riba, 2026, n, d(2026, 2, 28), "riba.txt", "contenuto", &items).unwrap();
    assert_eq!((b.count, b.total, b.open_items), (2, dec!(100.00), 2));
    assert!(s.bank_pending(BankKind::Riba).unwrap().is_empty());
    assert!(s.due_list(true, None).unwrap().iter().all(|r| r.batch_id == Some(b.id)));
    // la stessa scadenza non entra in una seconda distinta
    assert!(s.create_batch(BankKind::Riba, 2026, 2, d(2026, 2, 28), "y.txt", "...", &items[..1]).is_err());
    // incasso della distinta
    assert_eq!(s.settle_batch(b.id, d(2026, 3, 31), "Banca").unwrap(), 2);
    assert!(s.due_list(true, None).unwrap().is_empty());
    assert_eq!(s.batches(None).unwrap()[0].open_items, 0);
    // annullo impossibile con incassi registrati
    assert!(s.cancel_batch(b.id).is_err());
    // la prima ricevuta torna insoluta: incasso annullato, di nuovo da presentare
    let (_, its) = s.batch(b.id).unwrap();
    s.mark_batch_item_unpaid(its[0].id, "insoluto").unwrap();
    let again = s.bank_pending(BankKind::Riba).unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].id, doc.due_dates[0].id);
    assert_eq!(s.batches(None).unwrap()[0].unpaid_items, 1);
    // nuova distinta con la sola ricevuta insoluta, poi annullata
    let b2 = s.create_batch(BankKind::Riba, 2026, 2, d(2026, 4, 30), "riba2.txt", "...", &[(again[0].id, again[0].residual(), "R".into())]).unwrap();
    assert!(s.bank_pending(BankKind::Riba).unwrap().is_empty());
    s.cancel_batch(b2.id).unwrap();
    assert_eq!(s.bank_pending(BankKind::Riba).unwrap().len(), 1);
    assert_eq!(s.batches(Some(BankKind::Riba)).unwrap().len(), 2);
    // impostazioni
    s.set_bank_settings(&BankSettings { sia: "A1B2C".into(), ..Default::default() }).unwrap();
    assert_eq!(s.bank_settings().unwrap().sia, "A1B2C");
}

#[test]
fn vat_registers_and_liquidation() {
    use openfatt_core::vat::{Period, Periodicity, Register};
    let (s, c, _p, v) = setup();
    let svc = |price| DocLine { description: "Consulenza".into(), qty: dec!(1), unit_price: price, vat: v.clone(), ..Default::default() };
    let issue = |mut doc: Document| s.issue_document(s.save_document(&doc.clone()).map(|d| { doc = d; doc.id }).unwrap()).unwrap();
    let mk = |kind: DocKind, date: NaiveDate, price, f: &dyn Fn(&mut Document)| {
        let mut d = Document::new(kind);
        d.date = date;
        d.party_id = Some(c.id);
        d.lines = vec![svc(price)];
        f(&mut d);
        issue(d)
    };
    // marzo 2026: fattura 1000 + 22%, nota di credito 100, scissione dei pagamenti, IVA differita
    mk(DocKind::Invoice, d(2026, 3, 2), dec!(1000), &|_| {});
    mk(DocKind::CreditNote, d(2026, 3, 5), dec!(100), &|_| {});
    mk(DocKind::Invoice, d(2026, 3, 6), dec!(500), &|d| d.vat_liability = "S".into());
    let def = mk(DocKind::Invoice, d(2026, 3, 9), dec!(200), &|d| d.vat_liability = "D".into());
    // integrazione reverse charge TD17 (servizi da fornitore estero) e un'autofattura fuori liquidazione
    mk(DocKind::SelfInvoice, d(2026, 3, 10), dec!(300), &|d| d.fe_type = "TD17".into());
    mk(DocKind::SelfInvoice, d(2026, 3, 11), dec!(50), &|d| d.fe_type = "TD21".into());
    // acquisto registrato a marzo
    let mut pi = Document::new(DocKind::PurchaseInvoice);
    pi.date = d(2026, 3, 12);
    pi.party_id = Some(c.id);
    pi.external_number = "F-77".into();
    pi.lines = vec![svc(dec!(400))];
    issue(pi);

    let (from, to) = (d(2026, 3, 1), d(2026, 3, 31));
    let sales = s.vat_register(Register::Sales, from, to).unwrap();
    assert_eq!(sales.len(), 5); // TD21 escluso
    assert!(sales.iter().any(|r| r.taxable() == dec!(-100) && r.tax() == dec!(-22)));
    let purchases = s.vat_register(Register::Purchases, from, to).unwrap();
    assert_eq!(purchases.len(), 2); // acquisto + copia dell'integrazione
    assert!(purchases.iter().any(|r| r.external_number == "F-77"));
    let m = Period::new(2026, 3, Periodicity::Monthly);
    let t = s.vat_period_totals(m).unwrap();
    assert_eq!(t.sales_taxable, dec!(1600)); // 1000 - 100 + 500 + 200
    assert_eq!(t.purchases_taxable, dec!(700)); // 400 + 300 integrazione
    assert_eq!(t.vat_due, dec!(264)); // 220 - 22 + 66 (scissione e differita escluse)
    assert_eq!(t.split_payment, dec!(110));
    assert_eq!(t.deferred_pending, dec!(44));
    assert_eq!(t.vat_deducted, dec!(154)); // 88 + 66
    // incasso di metà della fattura a IVA differita ad aprile: metà dell'IVA diventa esigibile
    let due = s.due_list(true, None).unwrap().into_iter().find(|r| r.document_id == def.id).unwrap();
    s.pay_due(due.id, d(2026, 4, 15), dec!(122), "Banca", false).unwrap();
    let dv = s.deferred_vat(d(2026, 4, 1), d(2026, 4, 30)).unwrap();
    assert_eq!(dv.len(), 1);
    assert_eq!(dv[0].tax, dec!(22));
    assert_eq!(s.vat_period_totals(Period::new(2026, 4, Periodicity::Monthly)).unwrap().vat_due, dec!(22));
    // dopo un anno il residuo diventa esigibile comunque
    let rest = s.deferred_vat(d(2027, 3, 1), d(2027, 3, 31)).unwrap();
    assert_eq!(rest[0].tax, dec!(22));
    // liquidazioni dell'anno con riporti e dati manuali
    let mut i = s.vat_inputs(m).unwrap();
    i.tax_credits = dec!(10);
    s.set_vat_inputs(m, &i).unwrap();
    let y = s.vat_liquidations(2026, 4).unwrap();
    assert_eq!(y.len(), 4);
    let mar = &y[2].1;
    assert_eq!(mar.vat_payable, dec!(110)); // 264 - 154
    assert_eq!((mar.to_pay, mar.pay_now), (dec!(100), false)); // 110 - 10: non oltre 100 euro
    assert_eq!(y[3].1.prev_debt, dec!(100));
    assert_eq!(y[3].1.to_pay, dec!(122));
    assert!(y[3].1.pay_now);
    let mut st = s.vat_settings().unwrap();
    assert_eq!(st.declarant_role, 1);
    st.periodicity = Periodicity::Quarterly;
    s.set_vat_settings(&st).unwrap();
    let q = s.vat_liquidations(2026, 1).unwrap();
    assert!(q[0].0.quarterly);
    assert_eq!(q[0].1.interest, dec!(1.10)); // 1% di 110
}

#[test]
fn bulk_updates_are_all_or_nothing() {
    let (s, c, p, _v) = setup();
    let c2 = s.save_party(&Party { name: "Cliente Due".into(), ..Default::default() }).unwrap();
    let pay = s.payment_methods().unwrap()[0].id;
    assert_eq!(s.update_parties(&[c.id, c2.id], |x| x.default_payment_id = Some(pay)).unwrap(), 2);
    assert!(s.parties(&PartyFilter::default()).unwrap().iter().all(|x| x.default_payment_id == Some(pay)));
    let p2 = s.save_product(&Product { code: "A2".into(), description: "Articolo due".into(), ..Default::default() }).unwrap();
    assert_eq!(s.update_products(&[p.id, p2.id], |x| x.category = "Ufficio".into()).unwrap(), 2);
    assert!(s.products("", false).unwrap().iter().all(|x| x.category == "Ufficio"));
    // un codice duplicato fa fallire tutto: nessun articolo cambiato
    assert!(s.update_products(&[p.id, p2.id], |x| {
        x.category = "Altro".into();
        x.code = "STESSO".into();
    })
    .is_err());
    assert!(s.products("", false).unwrap().iter().all(|x| x.category == "Ufficio"));
}

// ---- documento salvato in bozza, preventivi fornitori, stato ordini, aliquote "sp", autofatture

fn fornitore(s: &Store, country: &str) -> Party {
    s.save_party(&Party { role: PartyRole::Supplier, name: format!("Fornitore {}", country), vat_number: "01234567897".into(), country: country.into(), vat_country: country.into(), ..Default::default() }).unwrap()
}

#[test]
fn bozza_ha_numero_e_scadenze_per_le_riba() {
    let s = Store::open_in_memory().unwrap();
    let c = s.save_party(&Party { role: PartyRole::Customer, name: "Cliente".into(), vat_number: "01234567897".into(), iban: "IT60X0542811101000000123456".into(), ..Default::default() }).unwrap();
    let v = s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap();
    let mut pay = PaymentMethod { name: "Ri.Ba. 30gg".into(), mp_code: "MP12".into(), installments: vec![Installment { days: 30, percent: dec!(100), ..Default::default() }], active: true, ..Default::default() };
    pay.id = s.save_payment_method(&pay).unwrap();
    let mut d = Document::new(DocKind::Invoice);
    d.party_id = Some(c.id);
    d.party = c.clone();
    d.payment_id = Some(pay.id);
    d.payment = Some(pay.clone());
    d.lines.push(DocLine { description: "Servizio".into(), unit_price: dec!(100), vat: VatRef::from_code(&v), ..Default::default() });
    let d = s.save_document(&d).unwrap();
    assert_eq!(d.status, DocStatus::Draft);
    assert_eq!(d.number, Some(1), "la bozza prende il numero");
    assert_eq!(d.due_dates.len(), 1, "la bozza ha le scadenze");
    assert!(s.bank_pending(BankKind::Riba).unwrap().iter().any(|r| r.document_id == d.id && r.draft));
    // le scadenze delle bozze non entrano nei calcoli ufficiali
    assert!(s.due_list(true, None).unwrap().is_empty());
    // risalvare non cambia il numero e non duplica le scadenze
    let again = s.save_document(&d).unwrap();
    assert_eq!(again.number, Some(1));
    assert_eq!(again.due_dates.len(), 1);
    // in distinta il documento si blocca, ma si emette conservando la scadenza
    let due = again.due_dates[0].id;
    let b = s.create_batch(BankKind::Riba, 2026, 1, again.due_dates[0].date, "r.txt", "x", &[(due, dec!(122), "R1".into())]).unwrap();
    assert!(s.document_in_batch(again.id).unwrap());
    assert!(s.save_document(&again).is_err());
    let issued = s.issue_document(again.id).unwrap();
    assert_eq!(issued.status, DocStatus::Issued);
    assert_eq!(issued.number, Some(1));
    assert_eq!(issued.due_dates[0].id, due, "la scadenza in distinta non viene rigenerata");
    assert_eq!(s.batch(b.id).unwrap().1.len(), 1);
}

#[test]
fn numero_cambia_con_anno_o_serie_in_bozza() {
    let s = Store::open_in_memory().unwrap();
    let mk = |y: i32| {
        let mut d = Document::new(DocKind::Quote);
        d.date = NaiveDate::from_ymd_opt(y, 3, 1).unwrap();
        d
    };
    let a = s.save_document(&mk(2026)).unwrap();
    let b = s.save_document(&mk(2026)).unwrap();
    assert_eq!((a.number, b.number), (Some(1), Some(2)));
    let mut c = b.clone();
    c.date = NaiveDate::from_ymd_opt(2027, 1, 2).unwrap();
    let c = s.save_document(&c).unwrap();
    assert_eq!(c.number, Some(1));
}

#[test]
fn preventivo_fornitore_diventa_ordine_e_stato_ordine() {
    let s = Store::open_in_memory().unwrap();
    let f = fornitore(&s, "IT");
    let v = s.vat_codes().unwrap().into_iter().find(|v| v.code == "22").unwrap();
    let mut q = Document::new(DocKind::SupplierQuote);
    q.party_id = Some(f.id);
    q.party = f.clone();
    q.lines.push(DocLine { description: "Sedia".into(), qty: dec!(10), unit_price: dec!(20), vat: VatRef::from_code(&v), ..Default::default() });
    let q = s.save_document(&q).unwrap();
    assert!(q.kind.is_purchase());
    let mut o = s.transform(&[q.id], DocKind::SupplierOrder).unwrap();
    assert_eq!(o.kind, DocKind::SupplierOrder);
    assert_eq!(o.order_status, OrderStatus::ToConfirm);
    o.order_status = OrderStatus::Confirmed;
    o.expected_date = NaiveDate::from_ymd_opt(2026, 12, 1);
    o.internal_notes = "Sollecitare".into();
    let o = s.save_document(&o).unwrap();
    let list = s.documents(&DocFilter { kinds: vec![DocKind::SupplierOrder], ..Default::default() }).unwrap();
    assert_eq!(list[0].order_status, OrderStatus::Confirmed);
    assert_eq!(list[0].expected_date, NaiveDate::from_ymd_opt(2026, 12, 1));
    assert_eq!(list[0].comment, "Sollecitare");
    // arrivo parziale: 4 su 10
    let mut a = s.transform(&[o.id], DocKind::GoodsReceipt).unwrap();
    a.lines.iter_mut().filter(|l| l.kind == LineKind::Item).for_each(|l| l.qty = dec!(4));
    let a = s.save_document(&a).unwrap();
    let p = s.order_progress(&[DocKind::SupplierOrder]).unwrap();
    let pr = &p[&o.id];
    assert!(pr.any && !pr.all);
    assert_eq!(pr.targets.len(), 1);
    assert_eq!(pr.targets[0].0, a.id);
    assert!(pr.targets[0].1.starts_with("Arrivo"));
    assert_eq!(list[0].order_status.effective(pr.any, pr.all), OrderStatus::Partial);
    // arrivo del resto: concluso, con due documenti collegati
    s.transform(&[o.id], DocKind::GoodsReceipt).unwrap();
    let p = s.order_progress(&[DocKind::SupplierOrder]).unwrap();
    assert!(p[&o.id].all);
    assert_eq!(p[&o.id].targets.len(), 2);
    assert_eq!(list[0].order_status.effective(p[&o.id].any, p[&o.id].all), OrderStatus::Concluded);
}

#[test]
fn aliquota_sp_mette_il_documento_in_scissione() {
    let s = Store::open_in_memory().unwrap();
    let v = s.vat_codes().unwrap().into_iter().find(|v| v.code == "22sp").expect("22sp presente");
    assert!(v.split_payment);
    let pa = s.save_party(&Party { role: PartyRole::Customer, name: "Comune".into(), vat_number: "01234567897".into(), is_pa: true, ..Default::default() }).unwrap();
    let mut d = Document::new(DocKind::Invoice);
    d.party_id = Some(pa.id);
    d.party = pa;
    d.lines.push(DocLine { description: "Fornitura".into(), unit_price: dec!(100), vat: VatRef::from_code(&v), ..Default::default() });
    let t = openfatt_core::calc::compute(&d).unwrap();
    assert_eq!(t.total, dec!(122));
    assert_eq!(t.net_to_pay, dec!(100), "l'IVA la versa la PA");
    let d = s.save_document(&d).unwrap();
    assert_eq!(d.vat_liability, "S");
    assert_eq!(d.due_dates.iter().map(|x| x.amount).sum::<Decimal>(), dec!(100));
}

#[test]
fn piu_fornitori_per_articolo() {
    let s = Store::open_in_memory().unwrap();
    let a = fornitore(&s, "IT");
    let b = s.save_party(&Party { role: PartyRole::Supplier, name: "Altro".into(), vat_number: "12345678903".into(), ..Default::default() }).unwrap();
    let p = s.save_product(&Product { description: "Sedia".into(), cost: dec!(50), supplier_id: Some(a.id), supplier_code: "A-1".into(), supplier_price: dec!(40), other_suppliers: vec![ProductSupplier { supplier_id: b.id, code: "B-9".into(), price: dec!(38), notes: "minimo 5".into() }, ProductSupplier::default()], ..Default::default() }).unwrap();
    assert_eq!(p.other_suppliers.len(), 1, "le righe vuote si scartano");
    let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
    assert_eq!(s.quote(Some(&a), &p, dec!(1), today, true).unwrap().price, dec!(40));
    assert_eq!(s.quote(Some(&b), &p, dec!(1), today, true).unwrap().price, dec!(38));
    assert_eq!(p.supplier_terms(b.id).unwrap().0, "B-9");
    let c = s.save_party(&Party { role: PartyRole::Supplier, name: "Terzo".into(), vat_number: "02345678907".into(), ..Default::default() }).unwrap();
    assert_eq!(s.quote(Some(&c), &p, dec!(1), today, true).unwrap().price, dec!(50), "altri fornitori: costo");
}

#[test]
fn autofattura_da_fattura_di_acquisto() {
    let s = Store::open_in_memory().unwrap();
    let f = fornitore(&s, "DE");
    let n = s.vat_codes().unwrap().into_iter().find(|v| v.code == "N6.9").unwrap();
    let mut d = Document::new(DocKind::PurchaseInvoice);
    d.party_id = Some(f.id);
    d.party = f.clone();
    d.external_number = "INV-77".into();
    d.date = NaiveDate::from_ymd_opt(2026, 10, 10).unwrap();
    d.lines.push(DocLine { description: "Consulenza".into(), unit_price: dec!(1000), vat: VatRef::from_code(&n), ..Default::default() });
    let d = s.save_document(&d).unwrap();
    let a = s.transform(&[d.id], DocKind::SelfInvoice).unwrap();
    assert_eq!(a.fe_type, "TD17", "servizi da fornitore UE");
    assert_eq!(a.related[0].kind, "FATTURA");
    assert_eq!(a.related[0].number, "INV-77");
    assert_eq!(a.lines[0].vat.rate, dec!(22));
    assert!(a.payment_id.is_none());
    assert!(a.due_dates.is_empty());
    assert!(a.causale.contains("INV-77"));
    assert_eq!(openfatt_core::calc::compute(&a).unwrap().total, dec!(1220));
}

#[test]
fn prezzi_di_un_listino_per_molti_articoli() {
    use openfatt_core::pricing::{PriceList, PriceListKind};
    let s = Store::open_in_memory().unwrap();
    let a = s.save_product(&Product { description: "A".into(), price: dec!(100), ..Default::default() }).unwrap();
    let b = s.save_product(&Product { description: "B".into(), price: dec!(50), ..Default::default() }).unwrap();
    let l = s.save_price_list(&PriceList { name: "Vip".into(), kind: PriceListKind::Derived, percent: dec!(-10), ..Default::default() }).unwrap();
    s.set_price_list_item(l.id, b.id, dec!(0), Some(dec!(40))).unwrap();
    let m = s.list_prices_for(l.id, &[a.clone(), b.clone()]).unwrap();
    assert_eq!(m[&a.id], dec!(90));
    assert_eq!(m[&b.id], dec!(40));
    let o = s.product_list_overrides(b.id).unwrap();
    assert_eq!(o[&l.id], dec!(40));
    assert!(s.product_list_overrides(a.id).unwrap().is_empty());
    assert_eq!(s.base_list_name(), "Prezzo base");
}
