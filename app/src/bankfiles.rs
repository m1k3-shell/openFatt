//! Preparazione delle distinte bancarie: raccolta dei dati da anagrafiche e documenti,
//! raggruppamento delle scadenze, generazione del file (crate openfatt-bank), stampa.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate, Weekday};
use openfatt_bank::riba::{self, RibaItem};
use openfatt_bank::sepa::{self, SddItem, SddScheme, SctItem, SeqType};
use openfatt_core::model::Party;
use openfatt_store::{BankBatch, BankKind, BatchItem, DueRow, Store};
use rust_decimal::Decimal;

/// Distinta pronta da salvare.
pub struct Prepared {
    pub kind: BankKind,
    pub year: i32,
    pub number: i64,
    pub date: NaiveDate,
    pub file_name: String,
    pub content: String,
    /// (scadenza, importo, riferimento) per l'archivio.
    pub items: Vec<(i64, Decimal, String)>,
    /// Voci del file dopo il raggruppamento.
    pub entries: usize,
    pub total: Decimal,
    pub warnings: Vec<String>,
    /// Clienti il cui primo addebito SDD viene presentato con questa distinta.
    pub first_sdd: Vec<i64>,
}

pub fn company(store: &Store) -> Result<openfatt_bank::Company, String> {
    let c = store.company().map_err(|e| e.to_string())?;
    let b = store.bank_settings().map_err(|e| e.to_string())?;
    Ok(openfatt_bank::Company {
        name: c.display_name(),
        address: format!("{} {}", c.address.trim(), c.civic.trim()).trim().to_string(),
        zip: c.zip.clone(),
        city: c.city.clone(),
        province: c.province.clone(),
        tax_id: if c.vat_number.trim().is_empty() { c.tax_code.trim().to_string() } else { c.vat_number.trim().to_string() },
        iban: c.iban.clone(),
        bic: c.bic.clone(),
        sia: b.sia.trim().to_string(),
        cuc: b.cuc.trim().to_string(),
        creditor_id: b.creditor_id.trim().to_string(),
    })
}

/// Primo giorno lavorativo (lunedì-venerdì) a partire da `d`.
pub fn business_day(mut d: NaiveDate) -> NaiveDate {
    while matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
        d = d.succ_opt().unwrap();
    }
    d
}

struct Group<'a> {
    party: Option<Party>,
    name: String,
    date: NaiveDate,
    dues: Vec<&'a DueRow>,
}

impl Group<'_> {
    fn total(&self) -> Decimal {
        self.dues.iter().map(|d| d.residual()).sum()
    }
    fn labels(&self, store: &Store, external: bool) -> String {
        let mut v: Vec<String> = Vec::new();
        for d in &self.dues {
            let mut l = d.doc_label.clone();
            if external {
                // fatture di acquisto: si indica il numero del fornitore
                if let Ok(doc) = store.document(d.document_id) {
                    if !doc.external_number.trim().is_empty() {
                        l = format!("fattura n. {} del {}", doc.external_number.trim(), doc.date.format("%d/%m/%Y"));
                    }
                }
            }
            if !v.contains(&l) {
                v.push(l);
            }
        }
        v.join(", ")
    }
}

/// Più scadenze dello stesso soggetto con la stessa data diventano una sola voce.
fn group<'a>(store: &Store, dues: &'a [DueRow]) -> Vec<Group<'a>> {
    let mut map: BTreeMap<(String, NaiveDate), Group<'a>> = BTreeMap::new();
    for d in dues {
        let key = (d.party_id.map(|p| p.to_string()).unwrap_or_else(|| format!("n:{}", d.party_name)), d.date);
        map.entry(key)
            .or_insert_with(|| Group { party: d.party_id.and_then(|id| store.party(id).ok().flatten()), name: d.party_name.clone(), date: d.date, dues: Vec::new() })
            .dues
            .push(d);
    }
    let mut v: Vec<Group> = map.into_values().collect();
    v.sort_by(|a, b| a.date.cmp(&b.date).then(a.name.cmp(&b.name)));
    v
}

/// Genera il file per le scadenze scelte. `execution` è la data di esecuzione dei bonifici.
pub fn prepare(store: &Store, kind: BankKind, dues: &[DueRow], execution: NaiveDate) -> Result<Prepared, Vec<String>> {
    let c = company(store).map_err(|e| vec![e])?;
    let now = chrono::Local::now().naive_local();
    let today = now.date();
    let year = today.year();
    let number = store.next_batch_number(kind, year).map_err(|e| vec![e.to_string()])?;
    let groups = group(store, dues);
    let drafts = dues.iter().filter(|d| d.draft).count();
    let tag = match kind {
        BankKind::Riba => "RB",
        BankKind::Sdd => "DD",
        BankKind::Sct => "BO",
    };
    let msg_id = format!("GA{}{}{:04}{}", tag, year, number, now.format("%m%d%H%M%S"));
    let mut warnings = Vec::new();
    if drafts > 0 {
        warnings.push(format!("{} scadenze vengono da documenti ancora in bozza: ricordarsi di emetterli e trasmetterli", drafts));
    }
    let mut items = Vec::new();
    let mut first_sdd = Vec::new();
    let reference = |i: usize| format!("{}{}-{}-{}", tag, year, number, i + 1);
    for (i, g) in groups.iter().enumerate() {
        for d in &g.dues {
            items.push((d.id, d.residual(), reference(i)));
        }
    }
    let total: Decimal = groups.iter().map(|g| g.total()).sum();
    let (content, date, file_name) = match kind {
        BankKind::Riba => {
            let past: Vec<String> = groups.iter().filter(|g| g.date <= today).map(|g| format!("{}: Ri.Ba. con scadenza {} già passata (la banca non la accetta: incassare in altro modo o sollecitare)", g.name, g.date.format("%d/%m/%Y"))).collect();
            if !past.is_empty() {
                return Err(past);
            }
            let list: Vec<RibaItem> = groups
                .iter()
                .map(|g| {
                    let p = g.party.clone().unwrap_or_default();
                    RibaItem {
                        due: g.date,
                        amount: g.total(),
                        debtor_name: if p.display_name().is_empty() { g.name.clone() } else { p.display_name() },
                        debtor_tax_id: if p.vat_number.trim().is_empty() { p.tax_code.trim().to_string() } else { p.vat_number.trim().to_string() },
                        debtor_address: format!("{} {}", p.address.trim(), p.civic.trim()).trim().to_string(),
                        debtor_zip: p.zip.clone(),
                        debtor_city: p.city.clone(),
                        debtor_province: p.province.clone(),
                        debtor_iban: p.iban.clone(),
                        debtor_abi: p.bank_abi.clone(),
                        debtor_cab: p.bank_cab.clone(),
                        debtor_bank: p.bank_name.clone(),
                        customer_code: if p.code.trim().is_empty() { p.id.to_string() } else { p.code.trim().to_string() },
                        description: format!("PER {}", g.labels(store, false)),
                    }
                })
                .collect();
            let support = riba::support_name(now, &c.sia);
            let content = riba::riba_file(&c, today, &support, &list)?;
            (content, groups.first().map(|g| g.date).unwrap_or(today), format!("RiBa {}-{}.txt", year, number))
        }
        BankKind::Sdd => {
            let earliest = business_day(today.succ_opt().unwrap());
            let mut list = Vec::new();
            for (i, g) in groups.iter().enumerate() {
                let p = g.party.clone().unwrap_or_default();
                let collection = if g.date < earliest {
                    warnings.push(format!("{}: scadenza {} già passata, addebito il {}", g.name, g.date.format("%d/%m/%Y"), earliest.format("%d/%m/%Y")));
                    earliest
                } else {
                    business_day(g.date)
                };
                let mandate_date = p.sdd_mandate_date.unwrap_or_else(|| {
                    warnings.push(format!("{}: data del mandato non indicata, usata la data convenzionale 02/01/2012", g.name));
                    NaiveDate::from_ymd_opt(2012, 1, 2).unwrap()
                });
                // un cliente presentato per la prima volta: FRST sulla prima voce, RCUR sulle altre
                let first = !p.sdd_first_done && p.id != 0 && !first_sdd.contains(&p.id);
                if first {
                    first_sdd.push(p.id);
                }
                list.push(SddItem {
                    name: if p.display_name().is_empty() { g.name.clone() } else { p.display_name() },
                    iban: p.iban.clone(),
                    amount: g.total(),
                    collection,
                    mandate_id: p.sdd_mandate_id.clone(),
                    mandate_date,
                    scheme: if p.sdd_scheme == "B2B" { SddScheme::B2b } else { SddScheme::Core },
                    seq: if first { SeqType::First } else { SeqType::Recurring },
                    end_to_end: reference(i),
                    remittance: format!("Pagamento {}", g.labels(store, false)),
                });
            }
            let date = list.iter().map(|i| i.collection).min().unwrap_or(today);
            (sepa::sdd_cbi(&c, &msg_id, now, &list)?, date, format!("SDD {}-{}.xml", year, number))
        }
        BankKind::Sct => {
            let list: Vec<SctItem> = groups
                .iter()
                .enumerate()
                .map(|(i, g)| {
                    let p = g.party.clone().unwrap_or_default();
                    SctItem {
                        name: if p.display_name().is_empty() { g.name.clone() } else { p.display_name() },
                        iban: p.iban.clone(),
                        amount: g.total(),
                        end_to_end: reference(i),
                        remittance: format!("Saldo {}", g.labels(store, true)),
                    }
                })
                .collect();
            if execution < today {
                return Err(vec!["La data di esecuzione dei bonifici è già passata".into()]);
            }
            (sepa::sct_cbi(&c, &msg_id, now, execution, &list)?, execution, format!("Bonifici {}-{}.xml", year, number))
        }
    };
    Ok(Prepared { kind, year, number, date, file_name, content, items, entries: groups.len(), total, warnings, first_sdd })
}

/// Registra la distinta in archivio e aggiorna i mandati SDD presentati per la prima volta.
pub fn issue(store: &Store, p: &Prepared) -> Result<BankBatch, String> {
    let b = store.create_batch(p.kind, p.year, p.number, p.date, &p.file_name, &p.content, &p.items).map_err(|e| e.to_string())?;
    for id in &p.first_sdd {
        if let Ok(Some(mut party)) = store.party(*id) {
            party.sdd_first_done = true;
            let _ = store.save_party(&party);
        }
    }
    Ok(b)
}

/// Stampa della distinta senza registrarla: per controllarla o consegnarla in banca.
pub fn preview_pdf(store: &Store, kind: BankKind, dues: &[DueRow]) -> Vec<u8> {
    let now = chrono::Local::now().naive_local();
    let total: Decimal = dues.iter().map(|d| d.residual()).sum();
    let b = BankBatch {
        id: 0,
        kind,
        year: now.date().year(),
        number: store.next_batch_number(kind, now.date().year()).unwrap_or(1),
        created_at: now,
        date: dues.first().map(|d| d.date).unwrap_or(now.date()),
        file_name: "non ancora esportato".into(),
        content: String::new(),
        total,
        count: dues.len() as i64,
        status: openfatt_store::BatchStatus::Issued,
        note: String::new(),
        open_items: dues.len() as i64,
        unpaid_items: 0,
    };
    let items: Vec<BatchItem> = dues.iter().map(|d| BatchItem { id: 0, due: d.clone(), amount: d.residual(), reference: String::new(), unpaid: false }).collect();
    batch_pdf(store, &b, &items)
}

/// Stampa della distinta (da allegare o consegnare in banca).
pub fn batch_pdf(store: &Store, b: &BankBatch, items: &[BatchItem]) -> Vec<u8> {
    use openfatt_core::money::fmt_eur;
    use openfatt_pdf::{PrintDoc, PrintLine};
    let c = store.company().unwrap_or_default();
    let settings = store.bank_settings().unwrap_or_default();
    let mut lines = Vec::new();
    for i in items {
        let party = i.due.party_id.and_then(|id| store.party(id).ok().flatten());
        let bank = party
            .as_ref()
            .map(|p| {
                if !p.iban.trim().is_empty() {
                    format!("IBAN {}", p.iban.trim())
                } else if !p.bank_abi.is_empty() {
                    format!("ABI {} CAB {}", p.bank_abi, p.bank_cab)
                } else {
                    String::new()
                }
            })
            .unwrap_or_default();
        lines.push(PrintLine {
            code: i.due.date.format("%d/%m/%Y").to_string(),
            description: format!("{} - {}{}{}", i.due.party_name, i.due.doc_label, if bank.is_empty() { String::new() } else { format!(" - {}", bank) }, if i.unpaid { " - INSOLUTO" } else { "" }),
            total: fmt_eur(i.amount),
            ..Default::default()
        });
    }
    let mut info = vec![("Voci".to_string(), b.count.to_string()), ("File".to_string(), b.file_name.clone())];
    match b.kind {
        BankKind::Riba => info.push(("Codice SIA".into(), settings.sia.clone())),
        BankKind::Sdd => info.push(("Identificativo creditore".into(), settings.creditor_id.clone())),
        BankKind::Sct => info.push(("Data di esecuzione".into(), b.date.format("%d/%m/%Y").to_string())),
    }
    let doc = PrintDoc {
        title: format!("Distinta {}", b.kind.label()),
        number: format!("{}/{}", b.number, b.year),
        date: b.created_at.format("%d/%m/%Y").to_string(),
        issuer: vec![c.display_name(), format!("{} {}", c.address, c.civic), format!("{} {} {}", c.zip, c.city, c.province), format!("P.IVA {}", c.vat_number)],
        recipient_title: "Banca".into(),
        recipient: vec![c.bank_name.clone(), format!("IBAN {}", c.iban)],
        info,
        lines,
        totals: vec![("Totale distinta".into(), format!("€ {}", fmt_eur(b.total)), true)],
        show_prices: true,
        ..Default::default()
    };
    openfatt_pdf::render(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stampa_distinta_senza_registrarla() {
        let s = openfatt_store::Store::open_in_memory().unwrap();
        crate::demo::populate(&s).unwrap();
        let pending = s.bank_pending(BankKind::Riba).unwrap();
        assert!(!pending.is_empty());
        let pdf = preview_pdf(&s, BankKind::Riba, &pending);
        assert!(pdf.starts_with(b"%PDF"));
        assert_eq!(s.bank_pending(BankKind::Riba).unwrap().len(), pending.len(), "la stampa non consuma le scadenze");
    }

    #[test]
    fn distinte_dall_archivio_dimostrativo() {
        let s = Store::open_in_memory().unwrap();
        crate::demo::populate(&s).unwrap();
        let today = chrono::Local::now().date_naive();
        for kind in BankKind::ALL {
            let all = s.bank_pending(kind).unwrap();
            // le Ri.Ba. già scadute non si presentano
            if kind == BankKind::Riba {
                assert!(prepare(&s, kind, &all, today).err().unwrap()[0].contains("già passata"));
            }
            let pending: Vec<_> = all.into_iter().filter(|d| kind != BankKind::Riba || d.date > today).collect();
            assert!(!pending.is_empty(), "nessuna scadenza {:?} nei dati dimostrativi", kind);
            let p = prepare(&s, kind, &pending, business_day(today.succ_opt().unwrap())).unwrap_or_else(|e| panic!("{:?}: {:?}", kind, e));
            assert!(p.total > Decimal::ZERO);
            let b = issue(&s, &p).unwrap();
            assert!(s.bank_pending(kind).unwrap().iter().all(|d| kind == BankKind::Riba && d.date <= today));
            let (b, items) = s.batch(b.id).unwrap();
            assert!(batch_pdf(&s, &b, &items).starts_with(b"%PDF"));
            // file XML scritti per la validazione con tools/valida-cbi.ps1
            if kind != BankKind::Riba {
                let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/xsd-samples/cbi");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(format!("demo-{}.xml", kind.code().to_lowercase())), &p.content).unwrap();
            } else {
                assert!(p.content.lines().all(|l| l.len() == 120));
            }
            if kind == BankKind::Sdd {
                // il primo addebito ha reso ricorrenti i successivi
                assert!(p.content.contains("<SeqTp>FRST</SeqTp>"));
                let party = s.party(p.first_sdd[0]).unwrap().unwrap();
                assert!(party.sdd_first_done);
            }
        }
    }
}
