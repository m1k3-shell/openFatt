//! Importazione diretta di un archivio di Easyfatt (interoperabilità sui propri dati).
//!
//! L'archivio è letto in sola lettura dal lettore ausiliario `openfatt-easyfatt`, che produce
//! un `Dump` con le tabelle; qui le righe diventano anagrafiche, articoli, documenti con
//! scadenze e pagamenti, movimenti di magazzino e prima nota di openFatt. Le classi IVA
//! sono le stesse del formato pubblico Easyfatt-XML.

use std::collections::HashMap;
use std::str::FromStr;

use chrono::NaiveDate;
use openfatt_fbread::{Dump, TableDump};
use rust_decimal::Decimal;

use openfatt_core::calc;
use openfatt_core::model::*;

use crate::{Agent, Result, Store, StoreError};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EasyfattImport {
    pub vat_codes: usize,
    pub payments: usize,
    pub agents: usize,
    pub parties: usize,
    pub parties_existing: usize,
    pub products: usize,
    pub products_existing: usize,
    pub documents: usize,
    pub documents_skipped: usize,
    pub dues: usize,
    pub cash_entries: usize,
    pub stock_moves: usize,
    pub warnings: Vec<String>,
}

type Row = Vec<Option<String>>;

fn rows<'a>(d: &'a Dump, t: &str) -> (&'a [Row], Option<&'a TableDump>) {
    match d.tables.get(t) {
        Some(t) => (&t.rows[..], Some(t)),
        None => (&[], None),
    }
}

fn s<'a>(t: Option<&TableDump>, r: &'a Row, c: &str) -> &'a str {
    t.map(|t| t.get(r, c)).unwrap_or("").trim()
}

fn dec(t: Option<&TableDump>, r: &Row, c: &str) -> Decimal {
    let v = s(t, r, c);
    Decimal::from_str(v).or_else(|_| Decimal::from_scientific(v)).map(|d| d.round_dp(8).normalize()).unwrap_or_default()
}

fn int(t: Option<&TableDump>, r: &Row, c: &str) -> Option<i64> {
    s(t, r, c).parse().ok()
}

fn flag(t: Option<&TableDump>, r: &Row, c: &str) -> bool {
    matches!(s(t, r, c), "1" | "-1" | "T" | "True")
}

fn date(t: Option<&TableDump>, r: &Row, c: &str) -> Option<NaiveDate> {
    let v = s(t, r, c);
    NaiveDate::parse_from_str(v.get(..10)?, "%Y-%m-%d").ok()
}

/// Natura dell'operazione dalla classe IVA di Easyfatt.
pub fn natura_of_class(class: &str) -> Option<&'static str> {
    Some(match class {
        "NonImponibile.1" => "N3.1",
        "NonImponibile.2" => "N3.2",
        "NonImponibile.3" => "N3.3",
        "NonImponibile.4" => "N3.4",
        "NonImponibile.5" => "N3.5",
        "NonImponibile" | "NonImponibile.6" => "N3.6",
        "Esente" => "N4",
        "EsclusoArt15" => "N1",
        "NonSoggetto.1" => "N2.1",
        "NonSoggetto.2" | "NonSoggetto.2.TotOperaz" | "NonSoggetto" => "N2.2",
        "VendRevCharge.1" => "N6.1",
        "VendRevCharge.2" => "N6.2",
        "VendRevCharge.3" => "N6.3",
        "VendRevCharge.4" => "N6.4",
        "VendRevCharge.5" => "N6.5",
        "VendRevCharge.6" => "N6.6",
        "VendRevCharge.7" => "N6.7",
        "VendRevCharge.8" => "N6.8",
        c if c.starts_with("VendRevCharge") => "N6.9",
        "Margine" | "RegimeMargine" => "N5",
        _ => return None,
    })
}

/// Modalità di pagamento FatturaPA dalla categoria di Easyfatt.
fn mp_of(categ: &str) -> &'static str {
    match categ.to_lowercase().as_str() {
        "contanti" => "MP01",
        "assegno" => "MP02",
        "assegnocirc" => "MP03",
        "riba" => "MP12",
        "rid" => "MP09",
        "sdd" => "MP19",
        "bancomat" | "cartacredito" | "carta" | "paypal" => "MP08",
        "mav" => "MP13",
        "compensazione" => "MP22",
        _ => "MP05",
    }
}

/// Rate "[30|50|T]" = giorni, percentuale, fine mese.
fn installments(text: &str) -> Vec<Installment> {
    text.split(['[', ']', '\r', '\n'])
        .filter(|p| p.contains('|'))
        .filter_map(|p| {
            let f: Vec<&str> = p.split('|').collect();
            Some(Installment { days: f.first()?.trim().parse().ok()?, percent: Decimal::from_str(f.get(1)?.trim()).ok()?, end_of_month: f.get(2).map(|x| x.trim() == "T").unwrap_or(false), extra_days: 0 })
        })
        .collect()
}

fn split_address(a: &str) -> (String, String) {
    match a.rsplit_once(',') {
        Some((street, civic)) if civic.trim().chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && civic.trim().len() <= 10 => (street.trim().to_string(), civic.trim().to_string()),
        _ => (a.trim().to_string(), String::new()),
    }
}

fn country_code(name: &str) -> String {
    let n = name.trim();
    if n.is_empty() || n.eq_ignore_ascii_case("italia") || n.eq_ignore_ascii_case("italy") {
        return "IT".into();
    }
    if n.len() == 2 {
        return n.to_uppercase();
    }
    match n.to_lowercase().as_str() {
        "francia" => "FR",
        "germania" => "DE",
        "spagna" => "ES",
        "austria" => "AT",
        "svizzera" => "CH",
        "san marino" => "SM",
        "regno unito" | "gran bretagna" => "GB",
        "stati uniti" | "usa" => "US",
        "belgio" => "BE",
        "paesi bassi" | "olanda" => "NL",
        "slovenia" => "SI",
        "croazia" => "HR",
        "polonia" => "PL",
        "romania" => "RO",
        "portogallo" => "PT",
        "grecia" => "GR",
        _ => "IT",
    }
    .into()
}

fn find_iban(text: &str) -> String {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase();
    for i in 0..compact.len().saturating_sub(26) {
        let cand = &compact[i..i + 27];
        if cand.starts_with("IT") && openfatt_core::fiscal::iban_valido(cand) {
            return cand.to_string();
        }
    }
    String::new()
}

impl Store {
    /// Importa il contenuto di un archivio Easyfatt letto dal lettore ausiliario.
    pub fn import_easyfatt(&self, dump: &Dump) -> Result<EasyfattImport> {
        let mut rep = EasyfattImport::default();
        let tx = self.begin()?;

        // ---- azienda (solo se ancora da configurare)
        let (cfg_rows, cfg) = rows(dump, "TConfig");
        if let Some(r) = cfg_rows.first() {
            let mut c = self.company()?;
            if c.name.trim().is_empty() && c.last_name.trim().is_empty() {
                let (street, civic) = split_address(s(cfg, r, "Az_Indirizzo"));
                c.name = s(cfg, r, "Az_Nome").into();
                c.address = street;
                c.civic = civic;
                c.zip = s(cfg, r, "Az_Cap").into();
                c.city = s(cfg, r, "Az_Citta").into();
                c.province = s(cfg, r, "Az_Prov").into();
                c.vat_number = s(cfg, r, "Az_PartitaIva").trim_start_matches("IT").into();
                c.tax_code = s(cfg, r, "Az_CodiceFiscale").into();
                c.phone = s(cfg, r, "Az_Tel").into();
                c.email = s(cfg, r, "Az_Email").into();
                c.pec = s(cfg, r, "Az_Pec").into();
                c.website = s(cfg, r, "Az_HomePage").into();
                self.set_company(&c)?;
                rep.warnings.push("Dati dell'azienda importati: controllare regime fiscale, REA e banca in Impostazioni".into());
            }
        }

        // ---- aliquote
        let mut vats = self.vat_codes()?;
        let mut vat_map: HashMap<String, (VatRef, bool, bool)> = HashMap::new(); // codice -> (iva, scissione, reverse charge acquisti)
        let (iva_rows, iva) = rows(dump, "TIva");
        for r in iva_rows {
            let code = s(iva, r, "CodIva").to_string();
            let rate = dec(iva, r, "PercIva");
            let class = s(iva, r, "ClasseIva");
            let desc = s(iva, r, "DescIva");
            let split = class == "SplitPayment";
            let acq_rc = class == "AcqRevCharge";
            if dec(iva, r, "PercIvaIndetr") > Decimal::ZERO {
                rep.warnings.push(format!("Codice IVA {} ({}): la detraibilità parziale non è gestita, l'IVA risulterà detraibile", code, desc));
            }
            let natura = if acq_rc { Some("N6.9") } else if rate.is_zero() { natura_of_class(class).or(Some("N2.2")) } else { None };
            let rate = if acq_rc { Decimal::ZERO } else { rate };
            let found = vats.iter().find(|v| v.rate == rate && v.natura.as_deref() == natura).cloned();
            let v = match found {
                Some(v) => v,
                None => {
                    let nv = VatCode { id: 0, code: code.clone(), description: desc.to_string(), rate, natura: natura.map(|n| n.to_string()), legal_ref: if rate.is_zero() { desc.to_string() } else { String::new() }, active: true };
                    let id = match self.save_vat_code(&nv) {
                        Ok(id) => id,
                        Err(_) => self.save_vat_code(&VatCode { code: format!("EF-{}", code), ..nv.clone() })?,
                    };
                    rep.vat_codes += 1;
                    let v = VatCode { id, ..nv };
                    vats.push(v.clone());
                    v
                }
            };
            vat_map.insert(code, (VatRef::from_code(&v), split, acq_rc));
        }

        // ---- pagamenti
        let mut pays = self.payment_methods()?;
        let (pag_rows, pag) = rows(dump, "TPagamenti");
        for r in pag_rows {
            let name = s(pag, r, "NomePagamento").to_string();
            if name.is_empty() || pays.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
                continue;
            }
            let mut inst = installments(s(pag, r, "Rate"));
            if inst.is_empty() {
                inst.push(Installment { days: 0, percent: Decimal::ONE_HUNDRED, end_of_month: false, extra_days: 0 });
            }
            // rate come 33+33+33: l'ultima completa il 100%
            let sum: Decimal = inst.iter().map(|i| i.percent).sum();
            if sum != Decimal::ONE_HUNDRED {
                if let Some(last) = inst.last_mut() {
                    last.percent += Decimal::ONE_HUNDRED - sum;
                }
            }
            let p = PaymentMethod { id: 0, name: name.clone(), mp_code: mp_of(s(pag, r, "CategPagamento")).into(), installments: inst, show_iban: s(pag, r, "CategPagamento").eq_ignore_ascii_case("bonifico"), active: true };
            let id = self.save_payment_method(&p)?;
            pays.push(PaymentMethod { id, ..p });
            rep.payments += 1;
        }
        let pay_id = |name: &str| pays.iter().find(|p| p.name.eq_ignore_ascii_case(name.trim())).map(|p| p.id);

        // ---- agenti
        let mut agents = self.agents(true)?;
        let (ag_rows, ag) = rows(dump, "TAgenti");
        for r in ag_rows {
            let name = s(ag, r, "Agente").to_string();
            if name.is_empty() || agents.iter().any(|a| a.name.eq_ignore_ascii_case(&name)) {
                continue;
            }
            let a = self.save_agent(&Agent { name, rate: Decimal::ZERO, active: !flag(ag, r, "Nascosto"), ..Default::default() })?;
            agents.push(a);
            rep.agents += 1;
        }
        if rep.agents > 0 {
            rep.warnings.push("Agenti importati senza percentuale di provvigione: indicarla in Analisi > Agenti".into());
        }
        let agent_id = |name: &str| agents.iter().find(|a| a.name.eq_ignore_ascii_case(name.trim())).map(|a| a.id);

        // ---- clienti e fornitori
        let mut party_map: HashMap<i64, Party> = HashMap::new();
        let (an_rows, an) = rows(dump, "TAnagrafica");
        for r in an_rows {
            let Some(old) = int(an, r, "IDAnagr") else { continue };
            let vat = s(an, r, "PartitaIva").trim_start_matches("IT").to_string();
            let cf = s(an, r, "CodiceFiscale").to_uppercase();
            if !vat.is_empty() || !cf.is_empty() {
                if let Some(p) = self.find_party_by_tax_id(&vat, &cf)? {
                    party_map.insert(old, p);
                    rep.parties_existing += 1;
                    continue;
                }
            }
            let (customer, supplier) = (flag(an, r, "Cliente"), flag(an, r, "Fornitore"));
            let (street, civic) = split_address(s(an, r, "Indirizzo"));
            let ufficio = s(an, r, "FE_CodUfficio").to_uppercase();
            let mut p = Party {
                code: s(an, r, "CodAnagr").into(),
                role: match (customer, supplier) {
                    (true, true) => PartyRole::Both,
                    (false, true) => PartyRole::Supplier,
                    _ => PartyRole::Customer,
                },
                name: s(an, r, "Nome").into(),
                vat_number: vat,
                tax_code: cf,
                address: street,
                civic,
                zip: s(an, r, "Cap").into(),
                city: s(an, r, "Citta").into(),
                province: s(an, r, "Prov").to_uppercase(),
                country: country_code(s(an, r, "Nazione")),
                phone: s(an, r, "Tel").into(),
                mobile: s(an, r, "Cell").into(),
                email: s(an, r, "Email").into(),
                pec: s(an, r, "Pec").into(),
                sdi_code: ufficio.clone(),
                is_pa: ufficio.len() == 6,
                default_payment_id: pay_id(s(an, r, "PagamentoDefault")),
                default_vat_id: vat_map.get(s(an, r, "CodIvaDefault")).and_then(|v| v.0.vat_id),
                default_discount: s(an, r, "ScontiDefault").replace('%', ""),
                iban: find_iban(s(an, r, "CoordBancarieDefault")),
                agent_id: agent_id(s(an, r, "AgenteDefault")),
                loyalty_card: s(an, r, "Fidelity_CodTessera").into(),
                notes: s(an, r, "Note").into(),
                ..Default::default()
            };
            if p.iban.is_empty() && !s(an, r, "CoordBancarieDefault").is_empty() {
                p.notes = format!("{}\nCoordinate bancarie: {}", p.notes, s(an, r, "CoordBancarieDefault")).trim().to_string();
            }
            let referente = s(an, r, "Referente");
            if !referente.is_empty() {
                p.contacts.push(Contact { name: referente.into(), role: "Referente".into(), ..Default::default() });
            }
            let intent = s(an, r, "DichIntentoDefault");
            if !intent.is_empty() {
                let d = date(an, r, "DataDichIntentoDefault");
                p.intents.push(IntentDeclaration { protocol: intent.into(), date: d, year: d.map(|d| chrono::Datelike::year(&d)).unwrap_or(0), ..Default::default() });
            }
            if p.name.trim().is_empty() {
                p.name = format!("Soggetto {}", old);
            }
            let saved = match self.save_party(&p) {
                Ok(x) => x,
                Err(StoreError::Invalid(e)) if e.contains("codice") || e.contains("Codice") => self.save_party(&Party { code: String::new(), ..p.clone() })?,
                Err(e) => return Err(e),
            };
            party_map.insert(old, saved);
            rep.parties += 1;
        }

        // ---- articoli
        let mut prod_map: HashMap<i64, Product> = HashMap::new();
        let (ar_rows, ar) = rows(dump, "TArticoli");
        for r in ar_rows {
            let Some(old) = int(ar, r, "IDArticolo") else { continue };
            let code = s(ar, r, "CodArticolo").to_string();
            if let Some(p) = if code.is_empty() { None } else { self.product_by_code(&code)? } {
                prod_map.insert(old, p);
                rep.products_existing += 1;
                continue;
            }
            let service = !s(ar, r, "Tipologia").starts_with("ArtMagazz") && !s(ar, r, "Tipologia").is_empty();
            let cat = s(ar, r, "NomeCategoria");
            let sub = s(ar, r, "NomeSottocategoria");
            let extra: Vec<Decimal> = (2..=9).map(|i| dec(ar, r, &format!("PrezzoNetto{}", i))).collect();
            let last = extra.iter().rposition(|x| !x.is_zero()).map(|i| i + 1).unwrap_or(0);
            let cost = [dec(ar, r, "PrezzoNettoForn"), dec(ar, r, "PrezzoUltimoCarico"), dec(ar, r, "PrezzoMedioCarico")].into_iter().find(|x| !x.is_zero()).unwrap_or_default();
            let p = Product {
                code: if code.is_empty() { format!("EF{}", old) } else { code },
                description: s(ar, r, "Desc").lines().next().unwrap_or("").chars().take(1000).collect(),
                unit: if s(ar, r, "Udm").is_empty() { if service { String::new() } else { "pz".into() } } else { s(ar, r, "Udm").into() },
                category: if sub.is_empty() { cat.to_string() } else { format!("{} > {}", cat, sub) },
                price: dec(ar, r, "PrezzoNetto1"),
                extra_prices: extra[..last].to_vec(),
                vat_id: vat_map.get(s(ar, r, "CodIva")).and_then(|v| v.0.vat_id),
                cost,
                barcode: s(ar, r, "CodBarre").into(),
                is_service: service,
                manage_stock: !service && flag(ar, r, "GestMagazzino"),
                supplier_id: int(ar, r, "IDFornitore").and_then(|f| party_map.get(&f)).map(|p| p.id),
                supplier_code: s(ar, r, "CodArticoloForn").into(),
                notes: s(ar, r, "Note").into(),
                eco_fee: dec(ar, r, "EcoContribNetto"),
                online: flag(ar, r, "PubblicaSuWeb"),
                ..Default::default()
            };
            let saved = match self.save_product(&p) {
                Ok(x) => x,
                Err(_) => self.save_product(&Product { barcode: String::new(), ..p.clone() })?,
            };
            prod_map.insert(old, saved);
            rep.products += 1;
        }
        // distinte base
        let (db_rows, dbt) = rows(dump, "TDiba");
        let mut boms: HashMap<i64, Vec<Component>> = HashMap::new();
        for r in db_rows {
            if let (Some(a), Some(c)) = (int(dbt, r, "IDArticolo"), int(dbt, r, "IDComponente")) {
                if let Some(cp) = prod_map.get(&c) {
                    boms.entry(a).or_default().push(Component { product_id: cp.id, qty: dec(dbt, r, "Qta"), note: String::new() });
                }
            }
        }
        for (a, comps) in boms {
            if let Some(p) = prod_map.get(&a).cloned() {
                let p = self.save_product(&Product { kind: ProductKind::Assembled, components: comps, ..p })?;
                prod_map.insert(a, p);
            }
        }

        // ---- documenti
        let (dt_rows, dt) = rows(dump, "TDocTestate");
        let (dr_rows, dr) = rows(dump, "TDocRighe");
        let (pn_rows, pn) = rows(dump, "TPrimaNota");
        let (mm_rows, mm) = rows(dump, "TMovMagazz");
        let mut lines_by_doc: HashMap<i64, Vec<&Row>> = HashMap::new();
        for r in dr_rows {
            if let Some(d) = int(dr, r, "IDDoc") {
                lines_by_doc.entry(d).or_default().push(r);
            }
        }
        for v in lines_by_doc.values_mut() {
            v.sort_by_key(|r| int(dr, r, "IDDocRiga").unwrap_or(0));
        }
        let mut dues_by_doc: HashMap<i64, Vec<&Row>> = HashMap::new();
        for r in pn_rows {
            if let Some(d) = int(pn, r, "IDDoc") {
                dues_by_doc.entry(d).or_default().push(r);
            }
        }
        let mut line_doc: HashMap<i64, i64> = HashMap::new(); // IDDocRiga -> IDDoc
        for r in dr_rows {
            if let (Some(l), Some(d)) = (int(dr, r, "IDDocRiga"), int(dr, r, "IDDoc")) {
                line_doc.insert(l, d);
            }
        }
        let mut moves_by_doc: HashMap<i64, Vec<&Row>> = HashMap::new();
        let mut free_moves: Vec<&Row> = Vec::new();
        for r in mm_rows {
            match int(mm, r, "IDDocRiga").and_then(|l| line_doc.get(&l)) {
                Some(d) => moves_by_doc.entry(*d).or_default().push(r),
                None => free_moves.push(r),
            }
        }
        let mut docs: Vec<&Row> = dt_rows.iter().collect();
        docs.sort_by_key(|r| (date(dt, r, "Data"), int(dt, r, "Num")));
        let mut doc_map: HashMap<i64, i64> = HashMap::new();
        let mut mismatch = 0usize;
        for r in docs {
            let Some(old) = int(dt, r, "IDDoc") else { continue };
            let tipo = s(dt, r, "TipoDoc");
            let (kind, note) = match tipo {
                "U" => (Some(DocKind::PurchaseInvoice), ""),
                "V" => (Some(DocKind::PurchaseCreditNote), ""),
                "W" => (None, "spese fuori campo IVA: importate in prima nota"),
                "T" => (None, "registrazione dei corrispettivi: non importata"),
                x => openfatt_interop::easyfatt_xml::kind_from_code(x),
            };
            let date_doc = date(dt, r, "Data").unwrap_or_else(|| chrono::Local::now().date_naive());
            let Some(kind) = kind else {
                // spese fuori campo: solo i movimenti di prima nota
                if tipo == "W" {
                    for p in dues_by_doc.get(&old).cloned().unwrap_or_default() {
                        if !flag(pn, p, "Saldato") {
                            continue;
                        }
                        let e = CashEntry {
                            date: date(pn, p, "DataPagam").unwrap_or(date_doc),
                            description: if s(dt, r, "DescDoc").is_empty() { "Spesa".into() } else { s(dt, r, "DescDoc").into() },
                            account: if s(pn, p, "Risorsa").is_empty() { "Cassa".into() } else { s(pn, p, "Risorsa").into() },
                            amount: -dec(pn, p, "Importo").abs(),
                            party_id: int(dt, r, "IDAnagr").and_then(|a| party_map.get(&a)).map(|p| p.id),
                            category: "Spese".into(),
                            ..Default::default()
                        };
                        self.add_cash_entry(&e)?;
                        rep.cash_entries += 1;
                    }
                } else if !note.is_empty() {
                    rep.documents_skipped += 1;
                }
                continue;
            };
            if !note.is_empty() && !rep.warnings.iter().any(|w| w.contains(note)) {
                rep.warnings.push(note.to_string());
            }
            let mut d = Document::new(kind);
            d.date = date_doc;
            d.number = int(dt, r, "Num");
            d.series = s(dt, r, "Numeraz").trim_start_matches('/').to_string();
            d.status = DocStatus::Issued;
            if kind.is_purchase() {
                d.external_number = s(dt, r, "NumDoc").into();
            }
            if let Some(p) = int(dt, r, "IDAnagr").and_then(|a| party_map.get(&a)) {
                d.party_id = Some(p.id);
                d.party = p.clone();
            }
            // dati del soggetto come stampati sul documento
            if !s(dt, r, "Anagr_Nome").is_empty() {
                let (street, civic) = split_address(s(dt, r, "Anagr_Indirizzo"));
                d.party.name = s(dt, r, "Anagr_Nome").into();
                d.party.first_name.clear();
                d.party.last_name.clear();
                d.party.address = street;
                d.party.civic = civic;
                d.party.zip = s(dt, r, "Anagr_Cap").into();
                d.party.city = s(dt, r, "Anagr_Citta").into();
                d.party.province = s(dt, r, "Anagr_Prov").to_uppercase();
                d.party.vat_number = s(dt, r, "Anagr_PartitaIva").trim_start_matches("IT").into();
                d.party.tax_code = s(dt, r, "Anagr_CodiceFiscale").into();
            }
            let dest: Vec<&str> = ["Anagr_DestNome", "Anagr_DestIndirizzo", "Anagr_DestCap", "Anagr_DestCitta", "Anagr_DestProv"].iter().map(|c| s(dt, r, c)).filter(|x| !x.is_empty()).collect();
            d.destination = dest.join("\n");
            d.payment_id = pay_id(s(dt, r, "Pagamento"));
            d.payment = d.payment_id.and_then(|id| pays.iter().find(|p| p.id == id).cloned());
            d.prices_include_vat = flag(dt, r, "UsaPrezzoIvato");
            d.agent_id = agent_id(s(dt, r, "Agente"));
            d.notes = s(dt, r, "Note").into();
            d.internal_notes = s(dt, r, "NoteInterne").into();
            d.fe_type = s(dt, r, "FE_TipoDoc").into();
            d.causale = s(dt, r, "FE_Causale").into();
            d.stamp.mode = StampMode::No;
            d.legacy_fe = kind.is_fiscal();
            if kind.has_transport() {
                d.transport = Some(Transport {
                    reason: s(dt, r, "CausaleTrasporto").into(),
                    carrier: s(dt, r, "Vettore").into(),
                    appearance: s(dt, r, "AspettoBeni").into(),
                    packages: s(dt, r, "NumColli").trim().parse().ok(),
                    port: s(dt, r, "Porto").into(),
                    ..Default::default()
                });
            }
            for (num, data, cig, cup, comm, tipo) in [("FE_Src_Num", "FE_Src_Data", "FE_Src_Cig", "FE_Src_Cup", "FE_Src_Commessa", "FE_Src_TipoDoc"), ("FE_Src2_Num", "FE_Src2_Data", "FE_Src2_Cig", "FE_Src2_Cup", "FE_Src2_Commessa", "FE_Src2_TipoDoc")] {
                if !s(dt, r, num).is_empty() {
                    let kind = match s(dt, r, tipo).to_lowercase().as_str() {
                        k if k.contains("contr") => "CONTRATTO",
                        k if k.contains("conv") => "CONVENZIONE",
                        _ => "ORDINE",
                    };
                    d.related.push(RelatedDoc { kind: kind.into(), number: s(dt, r, num).into(), date: date(dt, r, data), cig: s(dt, r, cig).into(), cup: s(dt, r, cup).into(), commessa: s(dt, r, comm).into() });
                }
            }
            let perc_ra = dec(dt, r, "PercRitAcconto");
            if !perc_ra.is_zero() {
                let base = dec(dt, r, "PercRitAcconto2");
                d.withholdings.push(Withholding { rate: perc_ra, base_percent: if base.is_zero() { Decimal::ONE_HUNDRED } else { base }, ..Default::default() });
            }
            let cassa = !s(dt, r, "ContrPrev_Nome").is_empty() && !dec(dt, r, "ContrPrev_Perc").is_zero();
            if cassa {
                let name = s(dt, r, "ContrPrev_Nome").to_lowercase();
                let vat = vat_map.get(s(dt, r, "ContrPrev_CodIva")).map(|v| v.0.clone()).unwrap_or_else(|| vats.iter().find(|v| v.rate == Decimal::from(22) && v.natura.is_none()).map(VatRef::from_code).unwrap_or_default());
                d.pension_fund = Some(PensionFund { kind: if name.contains("inps") || name.contains("rivalsa") { "TC22".into() } else { "TC22".into() }, rate: dec(dt, r, "ContrPrev_Perc"), vat, subject_to_withholding: flag(dt, r, "ContrPrev_SoggettoRA") });
                if !rep.warnings.iter().any(|w| w.starts_with("Contributo cassa previdenziale")) {
                    rep.warnings.push("Contributo cassa previdenziale importato come TC22 (INPS): se la cassa è un'altra cambiarla nei documenti da emettere".into());
                }
            }
            let mut any_split = false;
            let mut rc_purchase = false;
            for l in lines_by_doc.get(&old).cloned().unwrap_or_default() {
                let code_iva = s(dr, l, "CodIva");
                let desc = s(dr, l, "Desc").to_string();
                if code_iva.is_empty() && dec(dr, l, "PrezzoNetto").is_zero() && dec(dr, l, "Qta").is_zero() {
                    if !desc.is_empty() {
                        d.lines.push(DocLine { kind: LineKind::Text, description: desc, ..Default::default() });
                    }
                    continue;
                }
                let (vat, split, rc) = vat_map.get(code_iva).cloned().unwrap_or((VatRef { vat_id: None, rate: Decimal::from(22), natura: None, legal_ref: String::new() }, false, false));
                any_split |= split;
                rc_purchase |= rc;
                let product = int(dr, l, "IDArticoloScaricato").and_then(|a| prod_map.get(&a));
                d.lines.push(DocLine {
                    product_id: product.map(|p| p.id),
                    code: s(dr, l, "CodArticolo").into(),
                    description: desc,
                    qty: dec(dr, l, "Qta"),
                    unit: s(dr, l, "Udm").into(),
                    unit_price: if d.prices_include_vat { dec(dr, l, "PrezzoIvato") } else { dec(dr, l, "PrezzoNetto") },
                    discounts: s(dr, l, "Sconti").replace('%', "").replace(' ', ""),
                    vat,
                    withholding: flag(dr, l, "RitAcconto"),
                    unit_cost: dec(dr, l, "PrezzoAcquisto"),
                    lot: s(dr, l, "Lotto").into(),
                    lot_expiry: date(dr, l, "DataScadenza"),
                    eco_fee: dec(dr, l, "EcoContribNetto"),
                    commission: Some(dec(dr, l, "PercProvv")).filter(|x| !x.is_zero()),
                    // il contributo cassa non si calcola sulle spese escluse art. 15
                    pension_fund: cassa && vat_map.get(code_iva).map(|v| v.0.natura.as_deref() != Some("N1")).unwrap_or(true),
                    ..Default::default()
                });
            }
            // spese del documento (trasporto, incasso...)
            let spese = if d.prices_include_vat { dec(dt, r, "Spese_ImportoIvato") } else { dec(dt, r, "Spese_ImportoNetto") };
            if !spese.is_zero() {
                let (vat, _, _) = vat_map.get(s(dt, r, "Spese_CodIva")).cloned().unwrap_or((VatRef { vat_id: None, rate: Decimal::from(22), natura: None, legal_ref: String::new() }, false, false));
                d.lines.push(DocLine { description: if s(dt, r, "Spese_Nome").is_empty() { "Spese".into() } else { s(dt, r, "Spese_Nome").into() }, unit_price: spese, vat, ..Default::default() });
            }
            if any_split {
                d.vat_liability = "S".into();
            }
            if rc_purchase {
                rep.warnings.push(format!("{} {}: acquisto in reverse charge importato senza integrazione dell'IVA (registrare l'autofattura se il periodo è ancora da liquidare)", kind.label(), d.number_label()));
            }
            if kind == DocKind::RetailSale {
                let total = calc::compute(&d).map(|t| t.total).unwrap_or_default();
                d.retail = Some(RetailInfo { payments: vec![RetailPayment { method: RetailPayMethod::Cash, amount: total }], rt_status: RtStatus::Manual, rt_message: "importato da Easyfatt".into(), ..Default::default() });
            }
            if d.lines.is_empty() {
                d.status = DocStatus::Draft;
                d.number = None;
            }
            // confronto con il totale calcolato da Easyfatt
            let tot_ef = dec(dt, r, "TotDoc");
            if let Ok(t) = calc::compute(&d) {
                if !rc_purchase && !tot_ef.is_zero() && (t.total - tot_ef).abs() > Decimal::new(2, 2) {
                    mismatch += 1;
                    if mismatch <= 20 {
                        rep.warnings.push(format!("{} {} del {}: totale ricalcolato € {} invece di € {}", kind.label(), d.number_label(), d.date.format("%d/%m/%Y"), t.total, tot_ef));
                    }
                }
            }
            // scadenze ed esiti dei pagamenti di Easyfatt
            let dues: Vec<&Row> = dues_by_doc.get(&old).cloned().unwrap_or_default();
            d.due_dates = dues
                .iter()
                .map(|p| {
                    let amount = dec(pn, p, "Importo").abs();
                    let paid = flag(pn, p, "Saldato");
                    DueDate { date: date(pn, p, "DataScad").unwrap_or(d.date), amount, mp_code: d.payment.as_ref().map(|x| x.mp_code.clone()).unwrap_or_else(|| "MP05".into()), paid: if paid { amount } else { Decimal::ZERO }, paid_date: if paid { date(pn, p, "DataPagam") } else { None }, ..Default::default() }
                })
                .collect();
            // numerazioni che in Easyfatt sono separate (note di credito, rapporti d'intervento,
            // preventivi fornitore): in caso di numero già usato si usa una serie con la lettera del tipo
            let first = self.import_document(&d);
            let first = match first {
                Err(StoreError::Invalid(e)) if e.contains("Numero già utilizzato") => {
                    let mut d2 = d.clone();
                    d2.series = if d2.series.is_empty() { tipo.to_string() } else { format!("{}{}", d2.series, tipo) };
                    let r = self.import_document(&d2);
                    if r.is_ok() && !rep.warnings.iter().any(|w| w.contains(&format!("serie \"{}\"", d2.series))) {
                        rep.warnings.push(format!("{} con numerazione propria in Easyfatt: importati con serie \"{}\"", kind.plural(), d2.series));
                    }
                    r
                }
                other => other,
            };
            let saved = match first {
                Ok(x) => x,
                Err(e) => {
                    rep.documents_skipped += 1;
                    rep.warnings.push(format!("{} {} del {} non importato: {}", kind.label(), d.number_label(), d.date.format("%d/%m/%Y"), e));
                    continue;
                }
            };
            rep.dues += saved.due_dates.len();
            // incassi e pagamenti registrati
            for p in dues.iter().filter(|p| flag(pn, p, "Saldato")) {
                let incoming = !kind.is_purchase() != kind.is_credit();
                let amount = dec(pn, p, "Importo").abs();
                self.add_cash_entry(&CashEntry {
                    id: 0,
                    date: date(pn, p, "DataPagam").unwrap_or(saved.date),
                    description: format!("{} {} - {}", if incoming { "Incasso" } else { "Pagamento" }, saved.title(), saved.party.display_name()),
                    account: if s(pn, p, "Risorsa").is_empty() { "Banca".into() } else { s(pn, p, "Risorsa").into() },
                    amount: if incoming { amount } else { -amount },
                    document_id: Some(saved.id),
                    due_date_id: None,
                    party_id: saved.party_id,
                    category: if incoming { "Incassi clienti".into() } else { "Pagamenti fornitori".into() },
                })?;
                rep.cash_entries += 1;
            }
            // movimenti di magazzino come registrati da Easyfatt
            if let Some(ms) = moves_by_doc.get(&old) {
                self.conn().execute("DELETE FROM stock_moves WHERE document_id = ?1", [saved.id])?;
                for m in ms {
                    if let Some(p) = int(mm, m, "IDArticolo").and_then(|a| prod_map.get(&a)) {
                        let qty = dec(mm, m, "QtaCaricata") - dec(mm, m, "QtaScaricata");
                        if qty.is_zero() {
                            continue;
                        }
                        self.insert_stock_move(&StockMove { id: 0, date: date(mm, m, "Data").unwrap_or(saved.date), product_id: p.id, qty, warehouse: if s(mm, m, "Magazz").is_empty() { crate::DEFAULT_WAREHOUSE.into() } else { s(mm, m, "Magazz").into() }, unit_value: dec(mm, m, "PrezzoNetto"), reason: saved.title(), document_id: Some(saved.id), line_id: None, lot: s(mm, m, "Lotto").into(), variant: String::new(), expiry: date(mm, m, "DataScadenza"), serial: String::new() })?;
                        rep.stock_moves += 1;
                    }
                }
            }
            doc_map.insert(old, saved.id);
            rep.documents += 1;
        }
        if mismatch > 20 {
            rep.warnings.push(format!("... e altri {} documenti con totale diverso", mismatch - 20));
        }
        // movimenti di magazzino non legati a documenti (carichi iniziali, rettifiche)
        for m in free_moves {
            if let Some(p) = int(mm, m, "IDArticolo").and_then(|a| prod_map.get(&a)) {
                let qty = dec(mm, m, "QtaCaricata") - dec(mm, m, "QtaScaricata");
                if qty.is_zero() {
                    continue;
                }
                self.insert_stock_move(&StockMove { id: 0, date: date(mm, m, "Data").unwrap_or_else(|| chrono::Local::now().date_naive()), product_id: p.id, qty, warehouse: if s(mm, m, "Magazz").is_empty() { crate::DEFAULT_WAREHOUSE.into() } else { s(mm, m, "Magazz").into() }, unit_value: dec(mm, m, "PrezzoNetto"), reason: if s(mm, m, "Note").is_empty() { "Movimento importato da Easyfatt".into() } else { s(mm, m, "Note").into() }, document_id: None, line_id: None, lot: s(mm, m, "Lotto").into(), variant: String::new(), expiry: date(mm, m, "DataScadenza"), serial: String::new() })?;
                rep.stock_moves += 1;
            }
        }
        self.audit("archive", 0, "import_easyfatt", &format!("{} soggetti, {} articoli, {} documenti", rep.parties, rep.products, rep.documents))?;
        tx.commit()?;
        Ok(rep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(natura_of_class("NonImponibile.5"), Some("N3.5"));
        assert_eq!(natura_of_class("VendRevCharge.3"), Some("N6.3"));
        assert_eq!(natura_of_class("Imponibile"), None);
        let i = installments("[30|50|T]\r\n[60|50|T]\r\n");
        assert_eq!(i.len(), 2);
        assert!(i[0].end_of_month && i[1].days == 60);
        assert_eq!(split_address("Via Nazionale, 61"), ("Via Nazionale".to_string(), "61".to_string()));
        assert_eq!(split_address("Piazza Duomo"), ("Piazza Duomo".to_string(), String::new()));
        assert_eq!(country_code("Italia"), "IT");
        assert_eq!(find_iban("Banca X IBAN IT60 X054 2811 1010 0000 0123 456"), "IT60X0542811101000000123456");
    }

    /// Importazione reale da un archivio dimostrativo di Easyfatt letto con il lettore
    /// ausiliario (file JSON indicato con OPENFATT_EASYFATT_DUMP; non incluso nella repository).
    #[test]
    fn import_real_dump_if_available() {
        let Some(p) = std::env::var_os("OPENFATT_EASYFATT_DUMP") else { return };
        let dump: Dump = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        let s = Store::open_in_memory().unwrap();
        let r = s.import_easyfatt(&dump).unwrap();
        println!("{:#?}", r);
        assert!(r.parties > 10 && r.products > 10 && r.documents > 100);
        assert!(s.integrity_check().unwrap() == "ok");
        // seconda importazione: soggetti e articoli già presenti
        let r2 = s.import_easyfatt(&dump).unwrap();
        assert_eq!(r2.parties, 0);
        assert_eq!(r2.products, 0);
    }
}
