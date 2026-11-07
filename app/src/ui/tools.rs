//! Archivio: backup, apertura, importazioni ed esportazioni, registro operazioni.

use chrono::Datelike;
use eframe::egui::{self, RichText};
use openfatt_core::model::{DocStatus, PartyRole};
use openfatt_interop::ImportedDocument;

use super::widgets::{GRAY, ORANGE, RED};
use super::{Ctx, Nav};

#[derive(Default)]
pub struct State {
    preview_docs: Option<Vec<ImportedDocument>>,
    preview_parties: Option<(Vec<openfatt_core::model::Party>, Vec<String>)>,
    preview_products: Option<(Vec<(openfatt_core::model::Product, String)>, Vec<String>)>,
    csv_role: Option<PartyRole>,
    export_year: Option<i32>,
    show_audit: bool,
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    super::title(ui, "Archivio e strumenti");
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.group(|ui| {
            ui.label(RichText::new("Archivio").strong());
            ui.label(format!("File: {}", cx.store.path().display()));
            ui.horizontal_wrapped(|ui| {
                if ui.button("\u{E1DE} Backup...").clicked() {
                    let name = format!("Backup {} {}.gadb", cx.store.path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), chrono::Local::now().format("%Y-%m-%d %H%M"));
                    if let Some(p) = rfd::FileDialog::new().add_filter("Archivio", &["gadb"]).set_file_name(name).save_file() {
                        cx.msgs.check(cx.store.backup_to(&p), "Backup completato: il file è un archivio completo e apribile");
                    }
                }
                if ui.button("\u{E182} Verifica integrità").clicked() {
                    match cx.store.integrity_check() {
                        Ok(s) if s == "ok" => cx.msgs.ok("Archivio integro"),
                        Ok(s) => cx.msgs.err(format!("Problemi rilevati: {}", s)),
                        Err(e) => cx.msgs.err(e.to_string()),
                    }
                }
                if ui.button("\u{E256} Apri un altro archivio...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Archivio", &["gadb"]).pick_file() {
                        cx.nav.push(Nav::OpenArchive(p));
                    }
                }
                if ui.button("\u{E4F6} Chiudi archivio").clicked() {
                    cx.nav.push(Nav::CloseArchive);
                }
            });
            ui.label(RichText::new("Per ripristinare un backup basta aprirlo con 'Apri un altro archivio'.").small().color(GRAY));
        });

        ui.add_space(8.0);
        ui.group(|ui| {
            ui.label(RichText::new("Importazione da altri programmi").strong());
            ui.label("Formato Easyfatt-XML (documenti con clienti, righe e scadenze) e CSV per anagrafiche e articoli.");
            ui.horizontal_wrapped(|ui| {
                if ui.button("\u{E20C} Documenti da Easyfatt-XML...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Easyfatt-XML", &["xml", "DefXml", "defxml"]).pick_file() {
                        match std::fs::read(&p) {
                            Ok(b) => {
                                let xml = openfatt_fatturapa::parse::decode_xml_bytes(&b);
                                let vats = cx.store.vat_codes().unwrap_or_default();
                                match openfatt_interop::parse_documents(&xml, &vats) {
                                    Ok(d) => st.preview_docs = Some(d),
                                    Err(e) => cx.msgs.err(e),
                                }
                            }
                            Err(e) => cx.msgs.err(e.to_string()),
                        }
                    }
                }
                for (label, role) in [("Clienti da CSV...", PartyRole::Customer), ("Fornitori da CSV...", PartyRole::Supplier)] {
                    if ui.button(format!("\u{E20C} {}", label)).clicked() {
                        if let Some(p) = rfd::FileDialog::new().add_filter("CSV", &["csv", "txt"]).pick_file() {
                            match std::fs::read(&p) {
                                Ok(b) => {
                                    let text = openfatt_fatturapa::parse::decode_xml_bytes(&b);
                                    let r = openfatt_interop::csv::parties_from_csv(&text, role);
                                    st.csv_role = Some(role);
                                    st.preview_parties = Some((r.items, r.warnings));
                                }
                                Err(e) => cx.msgs.err(e.to_string()),
                            }
                        }
                    }
                }
                if ui.button("\u{E20C} Articoli da CSV...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("CSV", &["csv", "txt"]).pick_file() {
                        match std::fs::read(&p) {
                            Ok(b) => {
                                let r = openfatt_interop::csv::products_from_csv(&openfatt_fatturapa::parse::decode_xml_bytes(&b));
                                st.preview_products = Some((r.items, r.warnings));
                            }
                            Err(e) => cx.msgs.err(e.to_string()),
                        }
                    }
                }
            });
        });

        ui.add_space(8.0);
        ui.group(|ui| {
            ui.label(RichText::new("Esportazione").strong());
            ui.horizontal(|ui| {
                let y = st.export_year.get_or_insert(chrono::Local::now().year());
                ui.label("Anno");
                let mut ys = y.to_string();
                if ui.add(egui::TextEdit::singleline(&mut ys).desired_width(60.0)).changed() {
                    if let Ok(v) = ys.parse() {
                        *y = v;
                    }
                }
                if ui.button("\u{E4C0} Documenti emessi in Easyfatt-XML...").clicked() {
                    let filter = openfatt_store::DocFilter { year: Some(*y), status: Some(DocStatus::Issued), ..Default::default() };
                    let mut docs = Vec::new();
                    for s in cx.store.documents(&filter).unwrap_or_default() {
                        if let Ok(d) = cx.store.document(s.id) {
                            if let Ok(t) = openfatt_core::calc::compute(&d) {
                                docs.push((d, t));
                            }
                        }
                    }
                    let company = cx.store.company().unwrap_or_default();
                    let xml = openfatt_interop::export_documents(&company, &docs);
                    if let Some(p) = rfd::FileDialog::new().set_file_name(format!("Documenti {}.xml", y)).save_file() {
                        cx.msgs.check(std::fs::write(&p, xml), &format!("{} documenti esportati", docs.len()));
                    }
                }
                if ui.button("\u{E4C0} Fatture elettroniche dell'anno in una cartella...").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        let mut n = 0;
                        for dirn in [openfatt_store::Direction::Out, openfatt_store::Direction::In] {
                            for e in cx.store.einvoices(dirn, "", Some(*y)).unwrap_or_default() {
                                let sub = dir.join(if dirn == openfatt_store::Direction::Out { "Emesse" } else { "Ricevute" });
                                let _ = std::fs::create_dir_all(&sub);
                                let data = e.original.clone().unwrap_or(e.xml.clone());
                                if std::fs::write(sub.join(&e.file_name), data).is_ok() {
                                    n += 1;
                                }
                            }
                        }
                        cx.msgs.ok(format!("{} file esportati in {}", n, dir.display()));
                    }
                }
            });
        });

        ui.add_space(8.0);
        ui.group(|ui| {
            ui.checkbox(&mut st.show_audit, RichText::new("Registro delle operazioni").strong());
            if st.show_audit {
                for (ts, entity, id, action, details) in cx.store.audit_log(300).unwrap_or_default() {
                    ui.label(RichText::new(format!("{}  {} #{}  {}  {}", ts, entity, id, action, details)).small());
                }
            }
        });
    });

    preview_windows(ui.ctx(), cx, st);
}

fn preview_windows(ctx: &egui::Context, cx: &mut Ctx, st: &mut State) {
    let mut close = false;
    if let Some(docs) = &st.preview_docs {
        egui::Window::new("Anteprima importazione Easyfatt-XML").default_size([760.0, 520.0]).show(ctx, |ui| {
            ui.label(format!("{} documenti nel file. I soggetti sono riconosciuti per codice o partita IVA; quelli nuovi vengono creati.", docs.len()));
            egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                egui::Grid::new("imp").striped(true).num_columns(5).show(ui, |ui| {
                    for h in ["Documento", "Data", "Soggetto", "Totale", "Avvisi"] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for d in docs {
                        ui.label(format!("{} {}", d.doc.kind.label(), d.doc.number.map(|n| n.to_string()).unwrap_or_else(|| d.doc.external_number.clone())));
                        ui.label(d.doc.date.format("%d/%m/%Y").to_string());
                        ui.label(d.doc.party.display_name());
                        ui.label(d.recomputed_total.map(super::widgets::eur).unwrap_or_else(|| "-".into()));
                        if d.warnings.is_empty() {
                            ui.label("");
                        } else {
                            ui.label(RichText::new(d.warnings.join("; ")).small().color(ORANGE));
                        }
                        ui.end_row();
                    }
                });
            });
            ui.horizontal(|ui| {
                if ui.button("Importa").clicked() {
                    let mut ok = 0;
                    let mut errors = Vec::new();
                    for d in docs {
                        let mut doc = d.doc.clone();
                        let party = match find_or_create_party(cx, &doc.party) {
                            Ok(p) => p,
                            Err(e) => {
                                errors.push(e);
                                continue;
                            }
                        };
                        doc.party_id = Some(party.id);
                        doc.party = party;
                        for l in doc.lines.iter_mut() {
                            if !l.code.is_empty() {
                                if let Ok(Some(p)) = cx.store.product_by_code(&l.code) {
                                    l.product_id = Some(p.id);
                                }
                            }
                        }
                        match cx.store.import_document(&doc) {
                            Ok(_) => ok += 1,
                            Err(e) => errors.push(format!("{} {}: {}", doc.kind.label(), doc.number_label(), e)),
                        }
                    }
                    if errors.is_empty() {
                        cx.msgs.ok(format!("{} documenti importati", ok));
                        cx.nav.push(Nav::Refresh);
                    } else {
                        cx.msgs.err(format!("{} documenti importati, {} errori:\n{}", ok, errors.len(), errors.join("\n")));
                    }
                    close = true;
                }
                if ui.button("Annulla").clicked() {
                    close = true;
                }
            });
        });
    }
    if close {
        st.preview_docs = None;
    }

    let mut close = false;
    if let Some((items, warnings)) = &st.preview_parties {
        egui::Window::new("Anteprima importazione anagrafiche").default_size([640.0, 460.0]).show(ctx, |ui| {
            ui.label(format!("{} soggetti da importare", items.len()));
            for w in warnings {
                ui.label(RichText::new(w).small().color(RED));
            }
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                for p in items.iter().take(500) {
                    ui.label(format!("{}  {}  {}  {}", p.code, p.display_name(), p.vat_number, p.city));
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Importa").clicked() {
                    let mut ok = 0;
                    let mut skipped = 0;
                    for p in items {
                        if !p.vat_number.is_empty() || !p.tax_code.is_empty() {
                            if let Ok(Some(_)) = cx.store.find_party_by_tax_id(&p.vat_number, &p.tax_code) {
                                skipped += 1;
                                continue;
                            }
                        }
                        if cx.store.save_party(p).is_ok() {
                            ok += 1;
                        }
                    }
                    cx.msgs.ok(format!("{} soggetti importati, {} già presenti", ok, skipped));
                    close = true;
                }
                if ui.button("Annulla").clicked() {
                    close = true;
                }
            });
        });
    }
    if close {
        st.preview_parties = None;
    }

    let mut close = false;
    if let Some((items, warnings)) = &st.preview_products {
        egui::Window::new("Anteprima importazione articoli").default_size([640.0, 460.0]).show(ctx, |ui| {
            ui.label(format!("{} articoli da importare (i codici già presenti vengono aggiornati)", items.len()));
            for w in warnings {
                ui.label(RichText::new(w).small().color(RED));
            }
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                for (p, vat) in items.iter().take(500) {
                    ui.label(format!("{}  {}  € {}  IVA {}", p.code, p.description, super::widgets::eur(p.price), vat));
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Importa").clicked() {
                    let vats = cx.store.vat_codes().unwrap_or_default();
                    let mut ok = 0;
                    let mut errors = Vec::new();
                    for (p, vat) in items {
                        let mut p = p.clone();
                        let v = vat.trim().trim_end_matches('%');
                        p.vat_id = vats
                            .iter()
                            .find(|x| x.code.eq_ignore_ascii_case(v) || openfatt_core::money::parse_dec(v).map(|r| r == x.rate && x.natura.is_none()).unwrap_or(false))
                            .map(|x| x.id)
                            .or_else(|| vats.iter().find(|x| x.code == "22").map(|x| x.id));
                        if !p.code.is_empty() {
                            if let Ok(Some(existing)) = cx.store.product_by_code(&p.code) {
                                p.id = existing.id;
                                p.version = existing.version;
                            }
                        }
                        match cx.store.save_product(&p) {
                            Ok(_) => ok += 1,
                            Err(e) => errors.push(format!("{}: {}", p.code, e)),
                        }
                    }
                    if errors.is_empty() {
                        cx.msgs.ok(format!("{} articoli importati", ok));
                    } else {
                        cx.msgs.err(format!("{} importati, errori:\n{}", ok, errors.join("\n")));
                    }
                    close = true;
                }
                if ui.button("Annulla").clicked() {
                    close = true;
                }
            });
        });
    }
    if close {
        st.preview_products = None;
    }
}

fn find_or_create_party(cx: &mut Ctx, p: &openfatt_core::model::Party) -> Result<openfatt_core::model::Party, String> {
    if !p.vat_number.is_empty() || !p.tax_code.is_empty() {
        if let Ok(Some(found)) = cx.store.find_party_by_tax_id(&p.vat_number, &p.tax_code) {
            return Ok(found);
        }
    }
    if !p.code.is_empty() {
        if let Some(found) = cx.store.parties(&openfatt_store::PartyFilter { search: p.code.clone(), include_inactive: true, ..Default::default() }).unwrap_or_default().into_iter().find(|x| x.code == p.code) {
            return Ok(found);
        }
    }
    cx.store.save_party(p).map_err(|e| e.to_string())
}
