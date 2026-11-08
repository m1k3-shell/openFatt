//! Editor grafico dei modelli di stampa liberi e dei moduli prestampati.
//!
//! La pagina è disegnata in scala; i campi si spostano trascinandoli e si ridimensionano
//! dall'angolo in basso a destra. Per i moduli prestampati si carica la scansione del
//! modulo come sfondo e si posizionano i campi sopra le caselle; la correzione
//! dell'allineamento compensa i margini della stampante.

use base64::Engine;
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use openfatt_core::model::DocKind;
use openfatt_pdf::freeform::{ColKey, FieldKind, FreeColumn, FreeField, FreeLayout};

use super::theme::{self, icon};
use super::widgets::GRAY;
use super::Ctx;
use crate::printing;

#[derive(Clone, Copy, PartialEq)]
enum Sel {
    Field(usize),
    Table,
}

#[derive(Default)]
pub struct EditorState {
    /// (posizione nell'elenco o None se nuovo, modello in modifica)
    editing: Option<(Option<usize>, FreeLayout)>,
    sel: Option<Sel>,
    bg: Option<(usize, egui::TextureHandle)>,
    confirm_delete: Option<usize>,
}

const KINDS: [DocKind; 13] = [
    DocKind::Invoice,
    DocKind::AccompanyingInvoice,
    DocKind::CreditNote,
    DocKind::DebitNote,
    DocKind::Fee,
    DocKind::Quote,
    DocKind::CustomerOrder,
    DocKind::DeliveryNote,
    DocKind::Proforma,
    DocKind::SupplierQuote,
    DocKind::SupplierOrder,
    DocKind::GoodsReceipt,
    DocKind::StockTransfer,
];

impl EditorState {
    /// Apre l'editor su un modello nuovo (usato dalle catture dello schermo).
    pub fn start_new(&mut self, preprinted: bool) {
        let mut l = FreeLayout::standard_a4(if preprinted { "Modulo prestampato" } else { "Modello libero" });
        l.preprinted = preprinted;
        l.kinds = vec!["FAT".into()];
        self.editing = Some((None, l));
        self.sel = Some(Sel::Field(6));
    }
}

/// Sezione «Modelli liberi e moduli prestampati» delle impostazioni di stampa.
pub fn section(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut EditorState) {
    let mut list = printing::layouts(cx.store);
    theme::card(ui, "Modelli liberi e moduli prestampati", |_| {}, |ui| {
        ui.label(theme::muted("Impaginazione a piacere con il mouse, oppure stampa dei soli dati sui moduli prestampati (carta intestata, bollettari, DDT a ricalco). Il modello si assegna ai tipi di documento: per gli altri resta il modello scelto sopra."));
        ui.add_space(4.0);
        if list.is_empty() {
            ui.label(RichText::new("Nessun modello libero.").color(GRAY));
        }
        for (i, l) in list.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(theme::medium(&l.name));
                let kinds: Vec<&str> = l.kinds.iter().filter_map(|k| DocKind::from_code(k)).map(|k| k.label()).collect();
                ui.label(theme::muted(if kinds.is_empty() { "non assegnato".to_string() } else { kinds.join(", ") }));
                if l.preprinted {
                    theme::pill(ui, "Modulo prestampato", theme::Tone::Neutral);
                }
                if ui.small_button("Modifica").clicked() {
                    st.editing = Some((Some(i), l.clone()));
                    st.sel = None;
                    st.bg = None;
                }
                if ui.small_button(icon::COPY).on_hover_text("Duplica").clicked() {
                    let mut c = l.clone();
                    c.name = format!("{} (copia)", c.name);
                    c.kinds.clear();
                    st.editing = Some((None, c));
                    st.bg = None;
                }
                if ui.small_button(icon::TRASH).on_hover_text("Elimina").clicked() {
                    st.confirm_delete = Some(i);
                }
            });
        }
        ui.horizontal(|ui| {
            if theme::button(ui, icon::PLUS, "Nuovo modello").clicked() {
                st.editing = Some((None, FreeLayout::standard_a4("Modello libero")));
                st.sel = None;
                st.bg = None;
            }
            if theme::button(ui, icon::PLUS, "Nuovo modulo prestampato").clicked() {
                let mut l = FreeLayout::standard_a4("Modulo prestampato");
                l.preprinted = true;
                l.table.header = false;
                st.editing = Some((None, l));
                st.sel = None;
                st.bg = None;
            }
        });
    });
    if let Some(i) = st.confirm_delete {
        match super::widgets::confirm(ui.ctx(), "Elimina modello", "Eliminare il modello di stampa? I documenti torneranno al modello predefinito.") {
            Some(true) => {
                if i < list.len() {
                    list.remove(i);
                    cx.msgs.check(printing::save_layouts(cx.store, &list), "Modello eliminato");
                }
                st.confirm_delete = None;
            }
            Some(false) => st.confirm_delete = None,
            None => {}
        }
    }
    editor_window(ui.ctx(), cx, st, &mut list);
}

fn editor_window(ctx: &egui::Context, cx: &mut Ctx, st: &mut EditorState, list: &mut Vec<FreeLayout>) {
    let Some((pos, l)) = &mut st.editing else { return };
    let mut close = false;
    let mut open = true;
    egui::Window::new(format!("{}  {}", icon::LAYOUT, l.name)).open(&mut open).fixed_size(ctx.screen_rect().size() * egui::vec2(0.94, 0.86)).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).collapsible(false).show(ctx, |ui| {
        // ---- barra superiore
        ui.horizontal_wrapped(|ui| {
            ui.label("Nome");
            ui.add(egui::TextEdit::singleline(&mut l.name).desired_width(180.0));
            ui.label("Foglio");
            egui::ComboBox::from_id_salt("le_page").selected_text(format!("{} x {} mm", l.page_w, l.page_h)).show_ui(ui, |ui| {
                for (n, w, h) in [("A4", 210.0, 297.0), ("A5 verticale", 148.0, 210.0), ("A5 orizzontale", 210.0, 148.0), ("Lettera", 216.0, 279.0)] {
                    if ui.selectable_label(l.page_w == w && l.page_h == h, n).clicked() {
                        l.page_w = w;
                        l.page_h = h;
                    }
                }
            });
            ui.add(egui::DragValue::new(&mut l.page_w).range(50.0..=420.0).suffix(" mm").speed(0.5));
            ui.add(egui::DragValue::new(&mut l.page_h).range(50.0..=600.0).suffix(" mm").speed(0.5));
            ui.separator();
            ui.checkbox(&mut l.preprinted, "Modulo prestampato (stampa solo i dati)");
            if l.preprinted {
                ui.label("Correzione stampante");
                ui.add(egui::DragValue::new(&mut l.offset_x).range(-20.0..=20.0).suffix(" mm →").speed(0.1));
                ui.add(egui::DragValue::new(&mut l.offset_y).range(-20.0..=20.0).suffix(" mm ↓").speed(0.1));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Usato per");
            for k in KINDS {
                let mut on = l.applies_to(k.code());
                if ui.checkbox(&mut on, k.label()).changed() {
                    l.kinds.retain(|x| x != k.code());
                    if on {
                        l.kinds.push(k.code().to_string());
                    }
                }
            }
        });
        ui.horizontal(|ui| {
            egui::menu::menu_custom_button(ui, theme::button_widget(icon::PLUS, "Aggiungi campo"), |ui| {
                for k in FieldKind::ALL {
                    if ui.button(k.label()).clicked() {
                        ui.close_menu();
                        let (w, h) = match k {
                            FieldKind::Line => (60.0, 0.0),
                            FieldKind::Logo => (40.0, 18.0),
                            FieldKind::PartyBlock | FieldKind::CompanyBlock | FieldKind::InfoBlock | FieldKind::TotalsBlock | FieldKind::VatSummary => (80.0, 24.0),
                            _ => (50.0, 6.0),
                        };
                        let mut f = FreeField::new(k, 20.0, 20.0, w, h);
                        if k == FieldKind::Text {
                            f.text = "Testo".into();
                        }
                        if k == FieldKind::Info {
                            f.text = "Pagamento".into();
                        }
                        l.fields.push(f);
                        st.sel = Some(Sel::Field(l.fields.len() - 1));
                    }
                }
            });
            if theme::button(ui, icon::IMAGE, if l.background.is_empty() { "Scansione del modulo..." } else { "Cambia scansione..." }).on_hover_text("Immagine PNG o JPEG del modulo, a pagina intera: fa da sfondo per posizionare i campi").clicked() {
                if let Some(f) = rfd::FileDialog::new().add_filter("Immagine", &["png", "jpg", "jpeg"]).pick_file() {
                    match std::fs::read(&f) {
                        Ok(b) if b.len() > 3 * 1024 * 1024 => cx.msgs.err("Immagine troppo grande (massimo 3 MB): salvarla a 150 dpi"),
                        Ok(b) if openfatt_pdf::Image::from_bytes(&b).is_err() => cx.msgs.err("Formato non riconosciuto: usare PNG o JPEG"),
                        Ok(b) => {
                            l.background = base64::engine::general_purpose::STANDARD.encode(&b);
                            st.bg = None;
                        }
                        Err(e) => cx.msgs.err(e.to_string()),
                    }
                }
            }
            if !l.background.is_empty() {
                if ui.button("Togli la scansione").clicked() {
                    l.background.clear();
                    st.bg = None;
                }
                ui.checkbox(&mut l.print_background, "stampa anche la scansione (prova su carta bianca)");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary(ui, icon::CHECK, "Salva").clicked() {
                    // un tipo di documento ha un solo modello libero
                    let me = pos.unwrap_or(usize::MAX);
                    for (i, other) in list.iter_mut().enumerate() {
                        if i != me {
                            other.kinds.retain(|k| !l.kinds.contains(k));
                        }
                    }
                    match pos {
                        Some(i) if *i < list.len() => list[*i] = l.clone(),
                        _ => {
                            list.push(l.clone());
                            *pos = Some(list.len() - 1);
                        }
                    }
                    cx.msgs.check(printing::save_layouts(cx.store, list), "Modello salvato");
                }
                if theme::button(ui, icon::FILE_PDF, "Anteprima PDF").clicked() {
                    match preview_pdf(cx, l) {
                        Ok(pdf) => {
                            if let Err(e) = crate::services::open_bytes("anteprima_modello.pdf", &pdf) {
                                cx.msgs.err(e);
                            }
                        }
                        Err(e) => cx.msgs.err(e),
                    }
                }
                if theme::button(ui, icon::X, "Chiudi").clicked() {
                    close = true;
                }
            });
        });
        ui.separator();
        // ---- area di lavoro: pagina e proprietà
        let avail = ui.available_size();
        ui.horizontal_top(|ui| {
            let canvas_w = (avail.x - 330.0).max(300.0);
            let scale = ((avail.y - 10.0) / l.page_h).min(canvas_w / l.page_w).max(0.5);
            let (resp, painter) = ui.allocate_painter(Vec2::new(l.page_w * scale, l.page_h * scale), Sense::click());
            let page = resp.rect;
            let to_px = |x: f32, y: f32| Pos2::new(page.left() + x * scale, page.top() + y * scale);
            painter.rect_filled(page, 0, Color32::WHITE);
            painter.rect_stroke(page, 0, Stroke::new(1.0_f32, theme::BORDER), egui::StrokeKind::Outside);
            // sfondo
            if !l.background.is_empty() {
                if st.bg.as_ref().map(|(n, _)| *n != l.background.len()).unwrap_or(true) {
                    st.bg = base64::engine::general_purpose::STANDARD.decode(&l.background).ok().and_then(|b| super::products::load_texture_full(ui.ctx(), &b)).map(|t| (l.background.len(), t));
                }
                if let Some((_, t)) = &st.bg {
                    painter.image(t.id(), page, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::from_white_alpha(150));
                }
            }
            if resp.clicked() {
                st.sel = None;
            }
            // area delle righe
            {
                let t = &mut l.table;
                let r = Rect::from_min_size(to_px(t.x, t.y), Vec2::new(t.w * scale, t.h * scale));
                let sel = st.sel == Some(Sel::Table);
                painter.rect_filled(r, 2, Color32::from_rgba_unmultiplied(0x1d, 0x5f, 0xbf, 18));
                painter.rect_stroke(r, 2, Stroke::new(if sel { 2.0_f32 } else { 1.0 }, theme::ACCENT), egui::StrokeKind::Inside);
                for c in &t.columns {
                    let x = r.left() + c.x * scale;
                    painter.line_segment([Pos2::new(x, r.top()), Pos2::new(x, r.bottom())], Stroke::new(0.5_f32, theme::ACCENT));
                    painter.text(Pos2::new(x + 2.0, r.top() + 2.0), egui::Align2::LEFT_TOP, c.key.label(), egui::FontId::proportional(10.0), theme::ACCENT);
                }
                painter.text(r.center(), egui::Align2::CENTER_CENTER, "Righe del documento", egui::FontId::proportional(13.0), theme::ACCENT);
                let id = ui.id().with("table_area");
                let body = ui.interact(r, id, Sense::click_and_drag());
                if body.clicked() || body.drag_started() {
                    st.sel = Some(Sel::Table);
                }
                if body.dragged() {
                    let d = body.drag_delta() / scale;
                    t.x = (t.x + d.x).clamp(0.0, l.page_w - t.w);
                    t.y = (t.y + d.y).clamp(0.0, l.page_h - t.h);
                }
                let handle = Rect::from_center_size(r.right_bottom(), Vec2::splat(10.0));
                painter.rect_filled(handle, 1, theme::ACCENT);
                let hr = ui.interact(handle, id.with("h"), Sense::drag());
                if hr.dragged() {
                    let d = hr.drag_delta() / scale;
                    t.w = (t.w + d.x).clamp(20.0, l.page_w - t.x);
                    t.h = (t.h + d.y).clamp(10.0, l.page_h - t.y);
                    st.sel = Some(Sel::Table);
                }
            }
            // campi
            let n = l.fields.len();
            for i in 0..n {
                let f = &mut l.fields[i];
                let h = if f.kind == FieldKind::Line { 1.0 } else { f.h };
                let r = Rect::from_min_size(to_px(f.x, f.y), Vec2::new(f.w * scale, (h * scale).max(4.0)));
                let sel = st.sel == Some(Sel::Field(i));
                let hidden = l.preprinted && !f.on_preprinted;
                let color = if hidden { Color32::from_gray(170) } else if f.kind.is_decoration() { Color32::from_rgb(0x5e, 0x63, 0x6b) } else { Color32::from_rgb(0x2e, 0x7d, 0x32) };
                painter.rect_filled(r, 2, color.gamma_multiply(0.08));
                painter.rect_stroke(r, 2, Stroke::new(if sel { 2.0_f32 } else { 1.0 }, if sel { theme::ACCENT } else { color }), egui::StrokeKind::Inside);
                let label = match f.kind {
                    FieldKind::Text => f.text.lines().next().unwrap_or("").to_string(),
                    FieldKind::Info => format!("{}: ...", f.text),
                    k => k.label().to_string(),
                };
                let fs = (f.size * scale * 0.35).clamp(8.0, 16.0);
                painter.with_clip_rect(r).text(r.left_top() + Vec2::new(2.0, 1.0), egui::Align2::LEFT_TOP, label, egui::FontId::proportional(fs), color);
                let id = ui.id().with(("field", i));
                let body = ui.interact(r, id, Sense::click_and_drag());
                if body.clicked() || body.drag_started() {
                    st.sel = Some(Sel::Field(i));
                }
                if body.dragged() {
                    let d = body.drag_delta() / scale;
                    f.x = (f.x + d.x).clamp(0.0, (l.page_w - f.w).max(0.0));
                    f.y = (f.y + d.y).clamp(0.0, (l.page_h - h).max(0.0));
                }
                if sel {
                    let handle = Rect::from_center_size(r.right_bottom(), Vec2::splat(9.0));
                    painter.rect_filled(handle, 1, theme::ACCENT);
                    let hr = ui.interact(handle, id.with("h"), Sense::drag());
                    if hr.dragged() {
                        let d = hr.drag_delta() / scale;
                        f.w = (f.w + d.x).max(3.0);
                        if f.kind != FieldKind::Line {
                            f.h = (f.h + d.y).max(2.0);
                        }
                    }
                }
            }
            // tasti freccia: spostamento fine del campo selezionato
            if let Some(Sel::Field(i)) = st.sel {
                let step = if ui.input(|i| i.modifiers.shift) { 2.0 } else { 0.5 };
                let (dx, dy) = ui.input(|inp| {
                    (
                        if inp.key_pressed(egui::Key::ArrowRight) { step } else if inp.key_pressed(egui::Key::ArrowLeft) { -step } else { 0.0 },
                        if inp.key_pressed(egui::Key::ArrowDown) { step } else if inp.key_pressed(egui::Key::ArrowUp) { -step } else { 0.0 },
                    )
                });
                if let Some(f) = l.fields.get_mut(i) {
                    if !ui.ctx().wants_keyboard_input() {
                        f.x += dx;
                        f.y += dy;
                    }
                }
            }
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.set_width(300.0);
                egui::ScrollArea::vertical().id_salt("le_props").show(ui, |ui| properties(ui, l, &mut st.sel));
            });
        });
    });
    if close || !open {
        st.editing = None;
        st.sel = None;
    }
}

fn properties(ui: &mut egui::Ui, l: &mut FreeLayout, sel: &mut Option<Sel>) {
    match *sel {
        None => {
            ui.label(theme::medium("Proprietà"));
            ui.label(theme::muted("Fare clic su un campo per modificarlo. Trascinare per spostare, l'angolo in basso a destra per ridimensionare; con i tasti freccia si sposta di mezzo millimetro (Maiusc: 2 mm)."));
            ui.add_space(6.0);
            ui.label(theme::muted("Verde: dati del documento. Grigio: testi fissi e grafica, che su un modulo prestampato non si stampano."));
        }
        Some(Sel::Table) => {
            let t = &mut l.table;
            ui.label(theme::medium("Righe del documento"));
            egui::Grid::new("le_table").num_columns(2).show(ui, |ui| {
                for (lbl, v) in [("Sinistra", &mut t.x), ("Alto", &mut t.y), ("Larghezza", &mut t.w), ("Altezza", &mut t.h), ("Altezza riga", &mut t.row_h), ("Carattere", &mut t.size)] {
                    ui.label(lbl);
                    ui.add(egui::DragValue::new(v).speed(0.25).range(0.0..=600.0));
                    ui.end_row();
                }
            });
            ui.checkbox(&mut t.header, "Intestazione delle colonne");
            ui.add_space(4.0);
            ui.label(theme::medium("Colonne (mm dalla sinistra dell'area)"));
            let mut remove = None;
            for (i, c) in t.columns.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt(("le_col", i)).width(90.0).selected_text(c.key.label()).show_ui(ui, |ui| {
                        for k in ColKey::ALL {
                            ui.selectable_value(&mut c.key, k, k.label());
                        }
                    });
                    ui.add(egui::DragValue::new(&mut c.x).speed(0.25).range(0.0..=400.0).prefix("da "));
                    ui.add(egui::DragValue::new(&mut c.w).speed(0.25).range(2.0..=400.0).prefix("l "));
                    if ui.small_button(icon::TRASH).clicked() {
                        remove = Some(i);
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(96.0);
                    ui.radio_value(&mut c.align, 0, "sx");
                    ui.radio_value(&mut c.align, 1, "centro");
                    ui.radio_value(&mut c.align, 2, "dx");
                });
            }
            if let Some(i) = remove {
                t.columns.remove(i);
            }
            if ui.button(format!("{} Colonna", icon::PLUS)).clicked() {
                let x = t.columns.iter().map(|c| c.x + c.w).fold(0.0f32, f32::max) + 1.0;
                t.columns.push(FreeColumn { key: ColKey::Description, x: x.min(t.w - 10.0), w: 10.0, align: 0 });
            }
        }
        Some(Sel::Field(i)) => {
            let Some(f) = l.fields.get_mut(i) else {
                *sel = None;
                return;
            };
            ui.label(theme::medium("Campo"));
            egui::ComboBox::from_id_salt("le_kind").width(240.0).selected_text(f.kind.label()).show_ui(ui, |ui| {
                for k in FieldKind::ALL {
                    ui.selectable_value(&mut f.kind, k, k.label());
                }
            });
            match f.kind {
                FieldKind::Text => {
                    ui.label("Testo");
                    ui.add(egui::TextEdit::multiline(&mut f.text).desired_rows(3).desired_width(280.0));
                }
                FieldKind::Info => {
                    ui.label("Dato della testata (etichetta)");
                    egui::ComboBox::from_id_salt("le_info").width(240.0).selected_text(f.text.clone()).show_ui(ui, |ui| {
                        for k in ["Pagamento", "IBAN", "Doc. fornitore", "Causale trasp.", "Trasporto a cura", "Vettore", "Colli", "Peso lordo", "Aspetto beni", "Porto", "Inizio trasporto", "Magazzino"] {
                            ui.selectable_value(&mut f.text, k.to_string(), k);
                        }
                    });
                    ui.add(egui::TextEdit::singleline(&mut f.text).desired_width(240.0));
                }
                FieldKind::DocNumber | FieldKind::DocDate | FieldKind::PageNumber => {
                    ui.label("Testo prima del valore (es. \"N. \")");
                    ui.add(egui::TextEdit::singleline(&mut f.text).desired_width(240.0));
                }
                _ => {}
            }
            egui::Grid::new("le_field").num_columns(2).show(ui, |ui| {
                for (lbl, v) in [("Sinistra (mm)", &mut f.x), ("Alto (mm)", &mut f.y), ("Larghezza", &mut f.w), ("Altezza", &mut f.h), ("Carattere (pt)", &mut f.size)] {
                    ui.label(lbl);
                    ui.add(egui::DragValue::new(v).speed(0.25).range(0.0..=600.0));
                    ui.end_row();
                }
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut f.bold, "grassetto");
                ui.radio_value(&mut f.align, 0, "sx");
                ui.radio_value(&mut f.align, 1, "centro");
                ui.radio_value(&mut f.align, 2, "dx");
            });
            ui.checkbox(&mut f.on_preprinted, "Stampa anche sul modulo prestampato");
            ui.checkbox(&mut f.last_page_only, "Solo nell'ultima pagina");
            ui.add_space(6.0);
            let (mut dup, mut del) = (false, false);
            ui.horizontal(|ui| {
                dup = ui.button(format!("{} Duplica", icon::COPY)).clicked();
                del = ui.button(format!("{} Elimina", icon::TRASH)).clicked();
            });
            if dup {
                let mut c = f.clone();
                c.y += 6.0;
                l.fields.push(c);
                *sel = Some(Sel::Field(l.fields.len() - 1));
            } else if del {
                l.fields.remove(i);
                *sel = None;
            }
        }
    }
}

/// Anteprima con l'ultimo documento di un tipo assegnato al modello, o con dati di esempio.
fn preview_pdf(cx: &mut Ctx, l: &FreeLayout) -> Result<Vec<u8>, String> {
    let kinds: Vec<DocKind> = l.kinds.iter().filter_map(|k| DocKind::from_code(k)).collect();
    let kinds = if kinds.is_empty() { vec![DocKind::Invoice] } else { kinds };
    let last = cx.store.documents(&openfatt_store::DocFilter { kinds, ..Default::default() }).map_err(|e| e.to_string())?.into_iter().next();
    match last {
        Some(d) => {
            let doc = cx.store.document(d.id).map_err(|e| e.to_string())?;
            printing::free_pdf(cx.store, l, &doc)
        }
        None => Ok(openfatt_pdf::freeform::render(l, &[&openfatt_pdf::freeform::sample_doc()], printing::logo_image(cx.store).as_ref())),
    }
}
