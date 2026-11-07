//! Conservazione digitale delle fatture elettroniche e ordini NSO della Pubblica Amministrazione.

use chrono::Datelike;
use eframe::egui::{self, RichText};

use super::theme::{self, icon, Tone};
use super::widgets::GRAY;
use super::{Ctx, Nav};

pub struct State {
    year: i32,
    note: String,
}

impl Default for State {
    fn default() -> Self {
        State { year: chrono::Local::now().date_naive().year() - 1, note: String::new() }
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    theme::title_row(ui, &["Fattura elettronica"], "Conservazione e ordini NSO", None, |_| {});
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        theme::card(ui, "Conservazione digitale", |_| {}, |ui| {
            ui.label(RichText::new("La conservazione a norma la fa un conservatore. Il servizio gratuito dell'Agenzia delle Entrate (Fatture e Corrispettivi, previa adesione) conserva le fatture passate dallo SdI; in alternativa si consegna il pacchetto annuale al conservatore scelto.").small().color(GRAY));
            ui.label(RichText::new("Il pacchetto contiene le fatture emesse e ricevute (anche firmate), le ricevute dello SdI, l'indice con i metadati e le impronte SHA-256.").small().color(GRAY));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Anno");
                ui.add(egui::DragValue::new(&mut st.year).range(2019..=2100));
                if theme::primary(ui, icon::ARCHIVE, "Prepara il pacchetto...").clicked() {
                    match cx.store.conservation_package(st.year) {
                        Ok(p) => {
                            if let Some(path) = rfd::FileDialog::new().add_filter("ZIP", &["zip"]).set_file_name(&p.file_name).save_file() {
                                match std::fs::write(&path, &p.zip) {
                                    Ok(_) => cx.msgs.ok(format!("Pacchetto con {} fatture salvato in {}", p.count, path.display())),
                                    Err(e) => cx.msgs.err(e.to_string()),
                                }
                            }
                        }
                        Err(e) => cx.msgs.err(e.to_string()),
                    }
                }
            });
            ui.add_space(6.0);
            for r in cx.store.conservation_records().unwrap_or_default().iter().take(12) {
                ui.horizontal(|ui| {
                    ui.label(format!("{} · {} fatture · {}", r.year, r.count, &r.created_at[..10.min(r.created_at.len())]));
                    if r.status == "CONSEGNATO" {
                        theme::pill(ui, "Consegnato", Tone::Positive);
                        ui.label(RichText::new(&r.note).small().color(GRAY));
                    } else {
                        theme::pill(ui, "Da consegnare", Tone::Warning);
                        ui.add(egui::TextEdit::singleline(&mut st.note).desired_width(120.0).hint_text("rif. ricevuta"));
                        if ui.small_button("Consegnato").clicked() {
                            cx.msgs.check(cx.store.mark_conservation_delivered(r.id, &st.note), "Pacchetto segnato come consegnato");
                            st.note.clear();
                        }
                    }
                })
                .response
                .on_hover_text(format!("{}\nSHA-256 {}", r.file_name, r.hash));
            }
        });
        let ui = &mut cols[1];
        theme::card(ui, "Ordini NSO della Pubblica Amministrazione", |_| {}, |ui| {
            ui.label(RichText::new("Gli ordini elettronici degli enti del Servizio sanitario arrivano dal Nodo di Smistamento degli Ordini tramite un intermediario PEPPOL, in formato UBL. Importato il file XML, l'ordine diventa un ordine cliente; la fattura che lo evade riporta la tripletta ID, data e #punto ordinante# in DatiOrdineAcquisto, come richiesto per il pagamento.").small().color(GRAY));
            ui.add_space(6.0);
            if theme::primary(ui, icon::FILE_ARROW_DOWN, "Importa un ordine NSO (XML)...").clicked() {
                if let Some(files) = rfd::FileDialog::new().add_filter("Ordine UBL", &["xml"]).pick_files() {
                    let mut last = None;
                    for f in files {
                        let r = std::fs::read(&f).map_err(|e| e.to_string()).and_then(|b| openfatt_interop::nso::parse_order(&String::from_utf8_lossy(&b)));
                        match r {
                            Ok(o) => {
                                if let Some(d) = cx.msgs.check(cx.store.import_nso_order(&o), "") {
                                    cx.msgs.ok(format!("Ordine NSO {} del {} importato ({} righe)", o.id, o.issue_date.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default(), o.lines.len()));
                                    last = Some(d.id);
                                }
                            }
                            Err(e) => cx.msgs.err(format!("{}: {}", f.display(), e)),
                        }
                    }
                    if let Some(id) = last {
                        cx.nav.push(Nav::OpenDocument(id));
                    }
                }
            }
            ui.add_space(4.0);
            ui.label(RichText::new("Per un ordine ricevuto fuori dal NSO (email, telefono) l'ente può chiedere \"#NO#\" nel campo Commessa/Convenzione dei riferimenti dell'ordine.").small().color(GRAY));
        });
    });
}
