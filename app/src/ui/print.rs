//! Impostazioni di stampa (modello, logo, colonne, testi per tipo) e finestra delle etichette.

use eframe::egui::{self, RichText};
use openfatt_core::model::{DocKind, DocStatus};
use openfatt_pdf::labels;

use super::theme::{self, icon};
use super::{Ctx, Msgs};
use crate::printing::{self, KindPrint, LabelItem, PrintSettings};
use crate::services;

// ----------------------------------------------------------------------
// Impostazioni
// ----------------------------------------------------------------------

#[derive(Default)]
pub struct SettingsState {
    pub(crate) s: Option<PrintSettings>,
    kind: Option<DocKind>,
    pub(crate) layouts: super::layout_editor::EditorState,
}

const KINDS: [DocKind; 9] = [DocKind::Invoice, DocKind::AccompanyingInvoice, DocKind::CreditNote, DocKind::Quote, DocKind::CustomerOrder, DocKind::DeliveryNote, DocKind::Proforma, DocKind::Fee, DocKind::SupplierOrder];

pub fn settings_tab(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut SettingsState) {
    let s = st.s.get_or_insert_with(|| printing::settings(cx.store));
    let mut save = false;
    ui.label("Aspetto dei documenti stampati e inviati per email (anche il PDF di cortesia allegato alle fatture elettroniche).");
    ui.add_space(8.0);
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        theme::card(ui, "Modello", |_| {}, |ui| {
            for (id, name, desc) in [
                ("classico", "Classico", "riquadri grigi, come i moduli tradizionali"),
                ("moderno", "Moderno", "colore aziendale su titolo, righe e totale"),
                ("essenziale", "Essenziale", "solo linee sottili, nessun fondo colorato"),
            ] {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut s.template, id.to_string(), theme::medium(name));
                    ui.label(theme::muted(desc));
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Colore aziendale");
                let mut rgb = printing::parse_hex(&s.accent).map(|(r, g, b)| [r, g, b]).unwrap_or([0x1d, 0x5f, 0xbf]);
                if egui::widgets::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
                    s.accent = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                }
                ui.label(theme::muted(&s.accent));
            });
            ui.add_space(6.0);
            ui.label(theme::medium("Colonne e numerazione"));
            ui.checkbox(&mut s.show_codes, "codice articolo");
            ui.checkbox(&mut s.show_discount, "sconto %");
            ui.checkbox(&mut s.show_vat, "aliquota IVA della riga");
            ui.checkbox(&mut s.page_numbers, "numero di pagina in fondo a ogni pagina");
        });
        let ui = &mut cols[1];
        theme::card(ui, "Logo", |_| {}, |ui| {
            let logo = cx.store.logo().ok().flatten();
            match logo.as_ref().map(|b| openfatt_pdf::Image::from_bytes(b)) {
                Some(Ok(img)) => {
                    ui.label(format!("{} {} x {} pixel, {} kB", icon::IMAGE, img.width, img.height, logo.as_ref().unwrap().len() / 1024));
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(theme::NEGATIVE));
                }
                None => {
                    ui.label(theme::muted("Nessun logo. Si consiglia un PNG con sfondo trasparente, largo almeno 600 pixel."));
                }
            }
            ui.checkbox(&mut s.show_logo, "stampa il logo in testata");
            ui.horizontal(|ui| {
                if theme::button(ui, icon::UPLOAD_SIMPLE, "Scegli immagine...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Immagini", &["png", "jpg", "jpeg"]).pick_file() {
                        match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| openfatt_pdf::Image::from_bytes(&b).map(|_| b)) {
                            Ok(b) => {
                                cx.msgs.check(cx.store.set_logo(Some(&b)), "Logo salvato nell'archivio");
                            }
                            Err(e) => cx.msgs.err(e),
                        }
                    }
                }
                if logo.is_some() && theme::ghost(ui, true, icon::TRASH, "Togli").clicked() {
                    cx.msgs.check(cx.store.set_logo(None), "Logo rimosso");
                }
            });
        });
        ui.add_space(10.0);
        theme::card(ui, "Etichette", |_| {}, |ui| {
            ui.horizontal(|ui| {
                ui.label("Formato predefinito");
                egui::ComboBox::from_id_salt("lbl_fmt").width(300.0).selected_text(labels::format_by_id(&s.label_format).name).show_ui(ui, |ui| {
                    for f in labels::formats() {
                        ui.selectable_value(&mut s.label_format, f.id.to_string(), f.name);
                    }
                });
            });
            ui.checkbox(&mut s.label_vat_included, "prezzo IVA inclusa sulle etichette degli articoli");
            ui.checkbox(&mut s.label_border, "bordo intorno a ogni etichetta (per fogli non pretagliati)");
        });
    });
    ui.add_space(10.0);
    let kind = st.kind.get_or_insert(DocKind::Invoice);
    theme::card(ui, "Testi per tipo di documento", |_| {}, |ui| {
        ui.horizontal_wrapped(|ui| {
            for k in KINDS {
                ui.selectable_value(kind, k, k.label());
            }
        });
        ui.add_space(6.0);
        let k = s.kinds.entry(kind.code().to_string()).or_insert_with(KindPrint::default);
        egui::Grid::new("kind_print").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Titolo stampato");
            ui.add(egui::TextEdit::singleline(&mut k.title).desired_width(260.0).hint_text(kind.label()));
            ui.end_row();
            ui.label("Prezzi");
            ui.horizontal(|ui| {
                let def = *kind != DocKind::DeliveryNote;
                let mut v = k.prices.unwrap_or(def);
                if ui.checkbox(&mut v, if *kind == DocKind::DeliveryNote { "DDT valorizzato (con prezzi e totali)" } else { "mostra prezzi e totali" }).changed() {
                    k.prices = if v == def { None } else { Some(v) };
                }
            });
            ui.end_row();
            ui.label("Testo in fondo");
            ui.add(egui::TextEdit::multiline(&mut k.note).desired_width(520.0).desired_rows(3).hint_text("es. condizioni di vendita, validità dell'offerta, coordinate per il pagamento"));
            ui.end_row();
        });
    });
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if theme::primary(ui, icon::FLOPPY_DISK, "Salva").clicked() {
            save = true;
        }
        if theme::button(ui, icon::EYE, "Anteprima").on_hover_text("Stampa di prova con l'ultimo documento del tipo scelto (salva prima le impostazioni)").clicked() {
            save = true;
            let k = st.kind.unwrap_or(DocKind::Invoice);
            let list = cx.store.documents(&openfatt_store::DocFilter { kinds: vec![k], ..Default::default() }).unwrap_or_default();
            match list.iter().find(|d| d.status == DocStatus::Issued).or(list.first()) {
                Some(d) => {
                    let _ = cx.store.set_setting("print", st.s.as_ref().unwrap());
                    match cx.store.document(d.id).map_err(|e| e.to_string()).and_then(|doc| services::document_pdf(cx.store, &doc)) {
                        Ok(pdf) => {
                            if let Err(e) = services::open_bytes(&format!("Anteprima {}.pdf", k.label()), &pdf) {
                                cx.msgs.err(e);
                            }
                        }
                        Err(e) => cx.msgs.err(e),
                    }
                }
                None => cx.msgs.warn(format!("Nessun documento di tipo {} per l'anteprima", k.label())),
            }
        }
    });
    if save {
        let s = st.s.as_mut().unwrap();
        s.kinds.retain(|_, v| *v != KindPrint::default());
        cx.msgs.check(cx.store.set_setting("print", &*s), "Impostazioni di stampa salvate");
    }
    ui.add_space(10.0);
    super::layout_editor::section(ui, cx, &mut st.layouts);
}

// ----------------------------------------------------------------------
// Etichette
// ----------------------------------------------------------------------

#[derive(Default)]
pub struct LabelDialog {
    pub items: Vec<LabelItem>,
    format: String,
    skip: u32,
    border: bool,
}

impl LabelDialog {
    pub fn open(&mut self, store: &openfatt_store::Store, items: Vec<LabelItem>) {
        let s = printing::settings(store);
        let shipping = items.iter().all(|i| matches!(i.content, labels::LabelContent::Shipping { .. }));
        self.format = if shipping && !s.label_format.contains("2x4") && !s.label_format.contains("100x150") { "a4-2x4".into() } else { s.label_format };
        self.border = s.label_border;
        self.skip = 0;
        self.items = items;
    }

    pub fn show(&mut self, ctx: &egui::Context, msgs: &mut Msgs) {
        if self.items.is_empty() {
            return;
        }
        let mut close = false;
        let fmt = labels::format_by_id(&self.format);
        egui::Window::new(format!("{}  Etichette", icon::TAG))
            .id(egui::Id::new("labels_dialog"))
            .collapsible(false)
            .resizable(true)
            .default_width(620.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("lbl_opts").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label("Formato");
                    egui::ComboBox::from_id_salt("lbl_fmt_d").width(340.0).selected_text(fmt.name).show_ui(ui, |ui| {
                        for f in labels::formats() {
                            ui.selectable_value(&mut self.format, f.id.to_string(), f.name);
                        }
                    });
                    ui.end_row();
                    if !fmt.is_roll() {
                        ui.label("Salta");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.skip).range(0..=fmt.per_page() as u32 - 1));
                            ui.label(theme::muted(format!("etichette già usate del primo foglio (su {})", fmt.per_page())));
                        });
                        ui.end_row();
                    }
                    ui.label("");
                    ui.checkbox(&mut self.border, "bordo intorno alle etichette");
                    ui.end_row();
                });
                ui.add_space(6.0);
                theme::rule(ui);
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    egui::Grid::new("lbl_items").num_columns(2).spacing([12.0, 6.0]).striped(true).show(ui, |ui| {
                        ui.label(theme::medium("Etichetta").size(12.0).color(theme::MUTED));
                        ui.label(theme::medium("Copie").size(12.0).color(theme::MUTED));
                        ui.end_row();
                        for it in &mut self.items {
                            ui.allocate_ui_with_layout(egui::vec2(440.0, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.set_width(440.0);
                                ui.add(egui::Label::new(&it.label).truncate());
                            });
                            ui.add(egui::DragValue::new(&mut it.qty).range(0..=999));
                            ui.end_row();
                        }
                    });
                });
                let total: u32 = self.items.iter().map(|i| i.qty).sum();
                let sheets = if fmt.is_roll() { total as usize } else { (total as usize + self.skip as usize).div_ceil(fmt.per_page()) };
                ui.add_space(6.0);
                ui.label(theme::muted(format!("{} etichette, {} {}", total, sheets, if fmt.is_roll() { "pagine per l'etichettatrice" } else if sheets == 1 { "foglio" } else { "fogli" })));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if theme::button(ui, icon::X, "Chiudi").clicked() {
                        close = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::primary_enabled(ui, total > 0, icon::PRINTER, "Crea PDF").on_hover_text("Il PDF si apre per la stampa; stampare al 100% (senza adattare alla pagina)").clicked() {
                            let pdf = printing::labels_pdf(&self.items, &self.format, self.skip as usize, self.border);
                            if let Err(e) = services::open_bytes("Etichette.pdf", &pdf) {
                                msgs.err(e);
                            }
                        }
                    });
                });
            });
        if close {
            self.items.clear();
        }
    }
}
