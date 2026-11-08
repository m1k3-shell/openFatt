//! Fatture elettroniche: invio, notifiche, ricezione e registrazione.

use chrono::Datelike;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use openfatt_store::{Direction, EInvoiceStatus};

use super::widgets::{self, eur, status_color, GRAY, ORANGE};
use super::{Ctx, Nav};
use crate::services;

pub struct State {
    search: String,
    year: Option<i32>,
    selected: Option<i64>,
    viewing: Option<i64>,
    confirm_delete: Option<i64>,
    pfx_prompt: Option<(i64, String)>,
}

impl Default for State {
    fn default() -> Self {
        State { search: String::new(), year: Some(chrono::Local::now().year()), selected: None, viewing: None, confirm_delete: None, pfx_prompt: None }
    }
}

fn save_xml(cx: &mut Ctx, id: i64) {
    if let Ok(e) = cx.store.einvoice(id) {
        let (name, data) = match &e.original {
            Some(o) => (e.file_name.clone(), o.clone()),
            None => (e.file_name.clone(), e.xml.clone()),
        };
        if let Some(p) = rfd::FileDialog::new().set_file_name(&name).save_file() {
            cx.msgs.check(std::fs::write(&p, data), "File salvato");
        }
    }
}

fn view_window(ctx: &egui::Context, cx: &mut Ctx, st: &mut State) {
    let Some(id) = st.viewing else { return };
    let Ok(e) = cx.store.einvoice(id) else {
        st.viewing = None;
        return;
    };
    let mut open = true;
    egui::Window::new(format!("Fattura elettronica {}", e.file_name)).open(&mut open).default_size([760.0, 560.0]).show(ctx, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(e.status.label()).color(status_color(e.status.code())).strong());
            if !e.sdi_id.is_empty() {
                ui.label(format!("Identificativo SdI {}", e.sdi_id));
            }
            ui.label(RichText::new(format!("SHA-256 {}", &e.hash[..16])).small().color(GRAY));
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("\u{E702} Visualizza PDF").clicked() {
                match services::einvoice_pdf(&e.xml).and_then(|pdf| services::open_bytes(&format!("{}.pdf", e.file_name), &pdf)) {
                    Ok(_) => {}
                    Err(x) => cx.msgs.err(x),
                }
            }
            if ui.button("\u{E248} Salva file").clicked() {
                save_xml(cx, e.id);
            }
            if ui.button("\u{E23A} Apri XML").clicked() {
                cx.msgs.check(services::open_bytes(&e.file_name.replace(".p7m", ""), &e.xml), "");
            }
            if let Some(doc) = e.document_id {
                if ui.button("Apri documento").clicked() {
                    cx.nav.push(Nav::OpenDocument(doc));
                }
            }
        });
        ui.separator();
        match openfatt_fatturapa::parse_bytes(&e.xml) {
            Ok(f) => {
                let pd = openfatt_pdf::from_einvoice(&f, 0);
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    ui.label(RichText::new(format!("{} n. {} del {}", pd.title, pd.number, pd.date)).strong());
                    ui.columns(2, |c| {
                        c[0].label(RichText::new("Cedente / prestatore").small().color(GRAY));
                        for s in &pd.issuer {
                            c[0].label(s);
                        }
                        c[1].label(RichText::new(&pd.recipient_title).small().color(GRAY));
                        for s in &pd.recipient {
                            c[1].label(s);
                        }
                    });
                    egui::Grid::new("fe_lines").striped(true).num_columns(5).show(ui, |ui| {
                        for h in ["Descrizione", "Q.tà", "Prezzo", "Importo", "IVA"] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        for l in &pd.lines {
                            ui.label(widgets::truncate(&l.description, 70));
                            ui.label(&l.qty);
                            ui.label(&l.price);
                            ui.label(&l.total);
                            ui.label(&l.vat);
                            ui.end_row();
                        }
                    });
                    for (k, v, strong) in &pd.totals {
                        let t = format!("{}: {}", k, v);
                        ui.label(if *strong { RichText::new(t).strong() } else { RichText::new(t) });
                    }
                    for (d, m, a) in &pd.payments {
                        ui.label(format!("Scadenza {} {} € {}", d, m, a));
                    }
                    let issues = openfatt_fatturapa::validate(&f);
                    if !issues.is_empty() {
                        ui.separator();
                        ui.label(RichText::new("Controlli formali").strong());
                        for i in issues {
                            ui.label(RichText::new(i.to_string()).small().color(if i.severity == openfatt_fatturapa::Severity::Error { widgets::RED } else { ORANGE }));
                        }
                    }
                    let atts = &f.bodies[0].allegati;
                    for a in atts {
                        if ui.link(format!("\u{E39A} {}", a.nome)).clicked() {
                            cx.msgs.check(services::open_bytes(&a.nome, &a.dati), "");
                        }
                    }
                });
            }
            Err(x) => {
                ui.label(RichText::new(x.0).color(widgets::RED));
            }
        }
        ui.separator();
        ui.label(RichText::new("Cronologia notifiche").strong());
        let events = cx.store.einvoice_events(e.id).unwrap_or_default();
        if events.is_empty() {
            ui.label(RichText::new("Nessuna notifica").color(GRAY));
        }
        for ev in events {
            ui.label(format!("{}  [{}]  {}", ev.received_at, ev.kind, ev.description));
        }
        if !e.last_message.is_empty() {
            ui.label(RichText::new(&e.last_message).small().color(GRAY));
        }
    });
    if !open {
        st.viewing = None;
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, dir: Direction) {
    super::title(ui, if dir == Direction::Out { "Fatture elettroniche inviate" } else { "Fatture elettroniche ricevute" });
    view_window(ui.ctx(), cx, st);

    let mut done = false;
    if let Some((id, pw)) = &mut st.pfx_prompt {
        egui::Window::new("Password del certificato").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
            ui.label("Password del file .pfx (non viene memorizzata):");
            let r = ui.add(egui::TextEdit::singleline(pw).password(true).desired_width(240.0));
            ui.horizontal(|ui| {
                if ui.button("Firma").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                    super::signature::sign_now(cx, *id, pw);
                    done = true;
                }
                if ui.button("Annulla").clicked() {
                    done = true;
                }
            });
        });
    }
    if done {
        st.pfx_prompt = None;
    }

    if let Some(id) = st.confirm_delete {
        match widgets::confirm(ui.ctx(), "Elimina", "Eliminare la fattura elettronica dall'archivio?") {
            Some(true) => {
                cx.msgs.check(cx.store.delete_einvoice(id), "Eliminata");
                st.confirm_delete = None;
                st.selected = None;
            }
            Some(false) => st.confirm_delete = None,
            None => {}
        }
    }

    let list = cx.store.einvoices(dir, &st.search, st.year).unwrap_or_default();
    let sel = st.selected.and_then(|id| list.iter().find(|e| e.id == id)).cloned();

    ui.horizontal_wrapped(|ui| {
        if ui.button("\u{E218} Ricevi dalla PEC").on_hover_text("Legge la casella PEC: notifiche dello SdI e fatture dei fornitori").clicked() {
            cx.nav.push(Nav::PecFetch);
        }
        if ui.button("\u{E256} Importa file...").on_hover_text("Fatture .xml/.p7m scaricate dal portale Fatture e Corrispettivi, o notifiche SdI").clicked() {
            if let Some(files) = rfd::FileDialog::new().add_filter("Fatture e notifiche", &["xml", "p7m", "XML", "P7M"]).pick_files() {
                let mut sum = services::InboundSummary::default();
                for f in files {
                    match std::fs::read(&f) {
                        Ok(b) => services::import_file(cx.store, &f.file_name().unwrap_or_default().to_string_lossy(), &b, &mut sum),
                        Err(e) => sum.errors.push(e.to_string()),
                    }
                }
                if sum.errors.is_empty() {
                    cx.msgs.ok(sum.text());
                } else {
                    cx.msgs.err(sum.text());
                }
            }
        }
        if let Some(e) = &sel {
            if ui.button("\u{E220} Visualizza").clicked() {
                st.viewing = Some(e.id);
            }
            if ui.button("\u{E248} Salva file").clicked() {
                save_xml(cx, e.id);
            }
            if let Some(o) = &e.original {
                if ui.button("\u{E606} Verifica firma").clicked() {
                    super::signature::verify_dialog(cx, o);
                }
            }
            if dir == Direction::Out {
                if e.status == EInvoiceStatus::ToSend {
                    if !e.is_signed() {
                        if ui.button("\u{EBAC} Firma").on_hover_text("Firma digitale CAdES con il certificato scelto in Opzioni > Firma digitale").clicked() {
                            match services::sign_config(cx.store).source {
                                openfatt_sign::SignSource::None => {
                                    cx.msgs.warn("Scegliere il certificato in Opzioni > Firma digitale, oppure firmare con un programma esterno e usare 'Carica file firmato'")
                                }
                                openfatt_sign::SignSource::Pfx { .. } => st.pfx_prompt = Some((e.id, String::new())),
                                openfatt_sign::SignSource::Store { .. } => super::signature::sign_now(cx, e.id, ""),
                            }
                        }
                        if ui.button("\u{E256} Carica file firmato").on_hover_text("File .p7m firmato con un programma esterno partendo dall'XML salvato da qui").clicked() {
                            if let Some(p) = rfd::FileDialog::new().add_filter("File firmato", &["p7m", "P7M"]).pick_file() {
                                match std::fs::read(&p).map_err(|x| x.to_string()).and_then(|b| services::attach_external_signature(cx.store, e.id, &b)) {
                                    Ok(note) => cx.msgs.ok(note),
                                    Err(x) => cx.msgs.err(x),
                                }
                            }
                        }
                    } else if ui.button("Rimuovi firma").clicked() {
                        cx.msgs.check(cx.store.remove_signature(e.id), "Firma rimossa");
                    }
                }
                let can_send = matches!(e.status, EInvoiceStatus::ToSend);
                if ui.add_enabled(can_send, egui::Button::new("\u{E398} Invia allo SdI via PEC")).clicked() {
                    cx.nav.push(Nav::PecSend(e.id));
                }
                if e.status == EInvoiceStatus::ToSend && ui.button("\u{E182} Segna come inviata").on_hover_text("Se il file è stato caricato manualmente sul portale Fatture e Corrispettivi").clicked() {
                    cx.msgs.check(cx.store.mark_einvoice_sent(e.id, "Caricata manualmente sul portale"), "Segnata come inviata");
                }
                if e.status == EInvoiceStatus::Rejected {
                    if let Some(doc) = e.document_id {
                        if ui.button("\u{E3B4} Correggi documento").on_hover_text("Correggere e rigenerare: la fattura scartata va reinviata entro 5 giorni con stesso numero e data").clicked() {
                            cx.nav.push(Nav::OpenDocument(doc));
                        }
                        if ui.button("\u{E398} Rigenera").clicked() {
                            let mut issues = None;
                            super::documents::generate_einvoices(cx, &[doc], &mut issues);
                        }
                    }
                }
                if matches!(e.status, EInvoiceStatus::ToSend | EInvoiceStatus::Rejected) && ui.button("\u{E4A6} Elimina").clicked() {
                    st.confirm_delete = Some(e.id);
                }
            } else {
                if e.status == EInvoiceStatus::Received && ui.button("\u{E182} Registra come acquisto").clicked() {
                    if let Some(ids) = cx.msgs.check(cx.store.register_incoming(e.id), "Fattura registrata negli acquisti") {
                        if let Some(first) = ids.first() {
                            cx.nav.push(Nav::OpenDocument(*first));
                        }
                    }
                }
                if let Some(doc) = e.document_id {
                    if ui.button("Apri registrazione").clicked() {
                        cx.nav.push(Nav::OpenDocument(doc));
                    }
                }
                if ui.button("\u{E4A6} Elimina").clicked() {
                    st.confirm_delete = Some(e.id);
                }
            }
        }
    });
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut st.search).hint_text("\u{E30C} soggetto, numero, nome file, P.IVA").desired_width(280.0));
        egui::ComboBox::from_id_salt("fe_year").selected_text(st.year.map(|y| y.to_string()).unwrap_or_else(|| "Tutti gli anni".into())).show_ui(ui, |ui| {
            ui.selectable_value(&mut st.year, None, "Tutti gli anni");
            let cur = chrono::Local::now().year();
            for y in (cur - 6..=cur).rev() {
                ui.selectable_value(&mut st.year, Some(y), y.to_string());
            }
        });
    });
    if dir == Direction::Out {
        let pending = cx.store.documents_without_einvoice().unwrap_or_default().len();
        if pending > 0 {
            ui.label(RichText::new(format!("{} documenti emessi non hanno ancora una fattura elettronica: generarla da 'Fatture e note'.", pending)).color(ORANGE));
        }
    }

    let avail = ui.available_height();
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .max_scroll_height(avail)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(84.0))
        .column(Column::initial(70.0))
        .column(Column::initial(90.0))
        .column(Column::initial(260.0).at_least(100.0))
        .column(Column::initial(100.0))
        .column(Column::initial(200.0))
        .column(Column::remainder())
        .header(22.0, |mut h| {
            for t in ["Data", "Tipo", "Numero", if dir == Direction::Out { "Cliente" } else { "Fornitore" }, "Totale", "Stato", "File"] {
                h.col(|ui| {
                    ui.strong(t);
                });
            }
        })
        .body(|body| {
            body.rows(22.0, list.len(), |mut row| {
                let e = &list[row.index()];
                let is_sel = st.selected == Some(e.id);
                let date = chrono::NaiveDate::parse_from_str(&e.doc_date, "%Y-%m-%d").map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or(e.doc_date.clone());
                row.col(|ui| {
                    let r = ui.selectable_label(is_sel, date);
                    if r.clicked() {
                        st.selected = Some(e.id);
                    }
                    if r.double_clicked() {
                        st.viewing = Some(e.id);
                    }
                });
                row.col(|ui| {
                    ui.label(&e.doc_type);
                });
                row.col(|ui| {
                    let r = ui.selectable_label(is_sel, &e.doc_number);
                    if r.clicked() {
                        st.selected = Some(e.id);
                    }
                    if r.double_clicked() {
                        st.viewing = Some(e.id);
                    }
                });
                row.col(|ui| {
                    let r = ui.selectable_label(is_sel, &e.party_name);
                    if r.clicked() {
                        st.selected = Some(e.id);
                    }
                    if r.double_clicked() {
                        st.viewing = Some(e.id);
                    }
                });
                row.col(|ui| {
                    ui.label(eur(e.total));
                });
                row.col(|ui| {
                    ui.label(RichText::new(e.status.label()).color(status_color(e.status.code())));
                    if e.direction == Direction::Out && e.status == EInvoiceStatus::ToSend {
                        if e.is_signed() {
                            ui.label(RichText::new("\u{EBAC} firmata").small().color(widgets::GREEN));
                        } else if e.is_pa() {
                            ui.label(RichText::new("da firmare").small().color(widgets::RED));
                        }
                    }
                });
                row.col(|ui| {
                    ui.label(RichText::new(&e.file_name).small().color(GRAY));
                });
            });
        });
}
