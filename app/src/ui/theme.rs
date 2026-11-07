//! Aspetto dell'applicazione: caratteri, colori, pulsanti e componenti comuni.
//!
//! Stile "registro": la cornice non ha colore, l'azione principale è nera, il blu
//! (`ACCENT`) va solo sul dato principale e il rosso/verde solo sugli stati.
//! Carattere Geist (SIL OFL 1.1) con le cifre tabulari rese predefinite, così gli
//! importi restano incolonnati; icone Phosphor (MIT) tramite `egui-phosphor`.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Response, RichText, Sense, Stroke, TextStyle, Ui, Vec2,
};
pub use egui_phosphor::regular as icon;
use openfatt_store::EInvoiceStatus;

pub const GROUND: Color32 = Color32::from_rgb(0xf2, 0xf4, 0xf7);
pub const SURFACE: Color32 = Color32::WHITE;
pub const RAISED: Color32 = Color32::from_rgb(0xf6, 0xf7, 0xf9);
pub const ACTIVE: Color32 = Color32::from_rgb(0xe4, 0xe7, 0xec);
pub const BORDER: Color32 = Color32::from_rgb(0xe3, 0xe6, 0xea);
pub const RULE: Color32 = Color32::from_rgb(0xee, 0xf0, 0xf3);
pub const CONTROL: Color32 = Color32::from_rgb(0xcf, 0xd4, 0xdb);
pub const INK: Color32 = Color32::from_rgb(0x0a, 0x0a, 0x0a);
pub const INK_SOFT: Color32 = Color32::from_rgb(0x33, 0x37, 0x3d);
pub const MUTED: Color32 = Color32::from_rgb(0x5e, 0x63, 0x6b);
pub const PRIMARY: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
pub const ACCENT: Color32 = Color32::from_rgb(0x1d, 0x5f, 0xbf);
pub const POSITIVE: Color32 = Color32::from_rgb(0x2e, 0x7d, 0x32);
pub const NEGATIVE: Color32 = Color32::from_rgb(0xc4, 0x24, 0x2a);
pub const WARNING: Color32 = Color32::from_rgb(0x92, 0x40, 0x0e);
pub const TRACK: Color32 = Color32::from_rgb(0xdf, 0xe3, 0xe8);
pub const CHART_MUTED: Color32 = Color32::from_rgb(0xc3, 0xc8, 0xcf);

const GEIST_REGULAR: &[u8] = include_bytes!("../../assets/fonts/Geist-Regular.ttf");
const GEIST_MEDIUM: &[u8] = include_bytes!("../../assets/fonts/Geist-Medium.ttf");
const GEIST_BOLD: &[u8] = include_bytes!("../../assets/fonts/Geist-Bold.ttf");

pub fn medium_family() -> FontFamily {
    FontFamily::Name("medium".into())
}
pub fn bold_family() -> FontFamily {
    FontFamily::Name("bold".into())
}
/// Testo a peso medio (titoli di schede, voci di menu, pulsanti).
pub fn medium(t: impl Into<String>) -> RichText {
    RichText::new(t).family(medium_family()).color(INK)
}
/// Testo in grassetto (titoli di schermata, importi principali).
pub fn bold(t: impl Into<String>) -> RichText {
    RichText::new(t).family(bold_family()).color(INK)
}
pub fn muted(t: impl Into<String>) -> RichText {
    RichText::new(t).color(MUTED)
}

/// Caratteri, colori e misure dell'intera applicazione.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("geist".into(), FontData::from_static(GEIST_REGULAR).into());
    fonts.font_data.insert("geist-medium".into(), FontData::from_static(GEIST_MEDIUM).into());
    fonts.font_data.insert("geist-bold".into(), FontData::from_static(GEIST_BOLD).into());
    fonts.font_data.insert("phosphor".into(), egui_phosphor::Variant::Regular.font_data().into());
    let defaults = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let fam = |first: &str| -> Vec<String> {
        let mut v = vec![first.to_string(), "phosphor".to_string()];
        v.extend(defaults.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, fam("geist"));
    fonts.families.insert(medium_family(), fam("geist-medium"));
    fonts.families.insert(bold_family(), fam("geist-bold"));
    if let Some(m) = fonts.families.get_mut(&FontFamily::Monospace) {
        m.push("phosphor".into());
    }
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, medium_family())),
        (TextStyle::Heading, FontId::new(24.0, bold_family())),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();
    let sp = &mut style.spacing;
    sp.item_spacing = Vec2::new(8.0, 8.0);
    sp.button_padding = Vec2::new(12.0, 6.0);
    sp.interact_size = Vec2::new(40.0, 32.0);
    sp.window_margin = Margin::same(16);
    sp.menu_margin = Margin::same(6);
    sp.combo_height = 320.0;
    sp.icon_width = 16.0;
    sp.icon_width_inner = 10.0;
    sp.icon_spacing = 6.0;
    sp.scroll.bar_width = 8.0;

    let mut v = egui::Visuals::light();
    v.panel_fill = GROUND;
    v.window_fill = SURFACE;
    v.extreme_bg_color = SURFACE;
    v.faint_bg_color = RAISED;
    v.code_bg_color = RAISED;
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = WARNING;
    v.error_fg_color = NEGATIVE;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(28) };
    v.popup_shadow = egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(24) };
    v.selection.bg_fill = Color32::from_rgb(0xe1, 0xe7, 0xf0);
    v.selection.stroke = Stroke::new(1.0_f32, INK);
    v.text_cursor.stroke = Stroke::new(2.0_f32, INK);
    v.striped = false;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    let r = CornerRadius::same(6);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = SURFACE;
    w.noninteractive.weak_bg_fill = SURFACE;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, INK);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = SURFACE;
    w.inactive.weak_bg_fill = SURFACE;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, CONTROL);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, INK);
    w.inactive.corner_radius = r;
    w.inactive.expansion = 0.0;
    w.hovered.bg_fill = RAISED;
    w.hovered.weak_bg_fill = RAISED;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xb4, 0xbb, 0xc5));
    w.hovered.fg_stroke = Stroke::new(1.5_f32, INK);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = ACTIVE;
    w.active.weak_bg_fill = ACTIVE;
    w.active.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xa9, 0xb1, 0xbc));
    w.active.fg_stroke = Stroke::new(1.5_f32, INK);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;
    w.open.bg_fill = ACTIVE;
    w.open.weak_bg_fill = ACTIVE;
    w.open.bg_stroke = Stroke::new(1.0_f32, CONTROL);
    w.open.fg_stroke = Stroke::new(1.0_f32, INK);
    w.open.corner_radius = r;
    style.visuals = v;
    // le etichette non intercettano il clic: nelle tabelle si clicca tutta la riga
    // (il testo resta copiabile dai campi di modifica)
    style.interaction.selectable_labels = false;
    // tema chiaro sempre: all'avvio egui non conosce ancora il tema di sistema e
    // applicherebbe lo stile a quello scuro di riserva
    ctx.set_theme(egui::ThemePreference::Light);
    let style = std::sync::Arc::new(style);
    ctx.set_style_of(egui::Theme::Light, style.clone());
    ctx.set_style_of(egui::Theme::Dark, style);
}

fn label(icon: &str, text: &str) -> String {
    if icon.is_empty() {
        text.to_string()
    } else if text.is_empty() {
        icon.to_string()
    } else {
        format!("{}  {}", icon, text)
    }
}

/// Imposta i colori dei pulsanti dentro `ui` (riposo, passaggio, pressione), senza bordo.
pub fn flat_visuals(ui: &mut Ui, rest: Color32, hover: Color32, press: Color32) {
    let w = &mut ui.visuals_mut().widgets;
    for (st, fill) in [(&mut w.inactive, rest), (&mut w.hovered, hover), (&mut w.active, press), (&mut w.open, press)] {
        st.weak_bg_fill = fill;
        st.bg_fill = fill;
        st.bg_stroke = Stroke::NONE;
        st.expansion = 0.0;
    }
}

/// Pulsante dell'azione principale della schermata (nero). Uno per schermata.
pub fn primary(ui: &mut Ui, icon: &str, text: &str) -> Response {
    primary_enabled(ui, true, icon, text)
}
pub fn primary_enabled(ui: &mut Ui, enabled: bool, icon: &str, text: &str) -> Response {
    if !enabled {
        return ui.add_enabled(false, button_widget(icon, text));
    }
    ui.scope(|ui| {
        flat_visuals(ui, PRIMARY, Color32::from_rgb(0x2e, 0x2e, 0x2e), Color32::BLACK);
        ui.add(egui::Button::new(RichText::new(label(icon, text)).family(medium_family()).color(Color32::WHITE)).min_size(Vec2::new(0.0, 36.0)))
    })
    .inner
}

/// Pulsante normale (bianco con bordo), alto 36.
pub fn button_widget(icon: &str, text: &str) -> egui::Button<'static> {
    egui::Button::new(medium(label(icon, text))).min_size(Vec2::new(0.0, 36.0))
}
pub fn button(ui: &mut Ui, icon: &str, text: &str) -> Response {
    ui.add(button_widget(icon, text))
}
pub fn button_enabled(ui: &mut Ui, enabled: bool, icon: &str, text: &str) -> Response {
    ui.add_enabled(enabled, button_widget(icon, text))
}

/// Pulsante senza cornice (barre di selezione, azioni secondarie).
pub fn ghost(ui: &mut Ui, enabled: bool, icon: &str, text: &str) -> Response {
    ghost_colored(ui, enabled, icon, text, INK)
}
pub fn ghost_colored(ui: &mut Ui, enabled: bool, icon: &str, text: &str, color: Color32) -> Response {
    ui.scope(|ui| {
        flat_visuals(ui, Color32::TRANSPARENT, ACTIVE, Color32::from_rgb(0xd8, 0xdc, 0xe2));
        let t = RichText::new(label(icon, text)).family(medium_family());
        let t = if enabled { t.color(color) } else { t };
        ui.add_enabled(enabled, egui::Button::new(t).min_size(Vec2::new(0.0, 30.0)))
    })
    .inner
}

/// Pulsante con la sola icona e il suggerimento al passaggio del mouse.
pub fn icon_button(ui: &mut Ui, icon: &str, tip: &str) -> Response {
    ui.scope(|ui| {
        flat_visuals(ui, Color32::TRANSPARENT, RAISED, ACTIVE);
        ui.add(egui::Button::new(RichText::new(icon).size(18.0).color(INK_SOFT)).min_size(Vec2::splat(36.0)))
    })
    .inner
    .on_hover_text(tip)
}

/// Riga del titolo: percorso, titolo, sottotitolo a sinistra; azioni a destra.
pub fn title_row(ui: &mut Ui, crumbs: &[&str], title: &str, sub: Option<egui::text::LayoutJob>, actions: impl FnOnce(&mut Ui)) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            if !crumbs.is_empty() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for (i, c) in crumbs.iter().enumerate() {
                        if i > 0 {
                            ui.label(RichText::new(icon::CARET_RIGHT).size(11.0).color(MUTED));
                        }
                        ui.label(RichText::new(*c).size(13.0).color(MUTED));
                    }
                });
            }
            ui.label(bold(title).size(24.0));
            if let Some(j) = sub {
                ui.label(j);
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
            actions(ui);
        });
    });
    ui.add_space(14.0);
}

/// Sottotitolo composto: parti normali in grigio e parti evidenziate in nero.
pub fn subline(parts: &[(&str, bool)]) -> egui::text::LayoutJob {
    let mut j = egui::text::LayoutJob::default();
    for (t, strong) in parts {
        let fmt = if *strong {
            egui::TextFormat { font_id: FontId::new(15.0, medium_family()), color: INK, ..Default::default() }
        } else {
            egui::TextFormat { font_id: FontId::new(15.0, FontFamily::Proportional), color: MUTED, ..Default::default() }
        };
        j.append(t, 0.0, fmt);
    }
    j
}

/// Scheda bianca con bordo sottile.
pub fn card_frame() -> egui::Frame {
    egui::Frame::new().fill(SURFACE).stroke(Stroke::new(1.0_f32, BORDER)).corner_radius(8).inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 16 })
}

/// Scheda con intestazione (titolo a sinistra, elemento opzionale a destra).
pub fn card<R>(ui: &mut Ui, title: &str, right: impl FnOnce(&mut Ui), body: impl FnOnce(&mut Ui) -> R) -> R {
    card_frame()
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(medium(title).size(16.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| right(ui));
            });
            ui.add_space(6.0);
            body(ui)
        })
        .inner
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Positive,
    Negative,
    Warning,
    Neutral,
}

/// Etichetta di stato tonda (sfondo tenue, testo scuro della stessa tinta).
pub fn pill(ui: &mut Ui, text: &str, tone: Tone) -> Response {
    let (bg, fg) = match tone {
        Tone::Positive => (Color32::from_rgb(0xdc, 0xfa, 0xe5), Color32::from_rgb(0x04, 0x25, 0x0e)),
        Tone::Negative => (Color32::from_rgb(0xff, 0xed, 0xea), Color32::from_rgb(0x83, 0x1c, 0x1e)),
        Tone::Warning => (Color32::from_rgb(0xff, 0xf4, 0xd6), Color32::from_rgb(0x92, 0x40, 0x0e)),
        Tone::Neutral => (Color32::from_rgb(0xec, 0xee, 0xf1), Color32::from_rgb(0x3d, 0x3d, 0x3d)),
    };
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(12.0, medium_family()), fg);
    let size = Vec2::new(galley.size().x + 16.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(11), bg);
    ui.painter().galley(egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, fg);
    resp
}

/// Fasi della fattura elettronica: (tacche completate, tacca corrente, esito negativo, descrizione).
pub fn sdi_progress(code: Option<&str>) -> (usize, bool, bool, &'static str) {
    if code == Some("PRECEDENTE") {
        return (3, false, false, "Trasmessa dal programma precedente");
    }
    match code.map(EInvoiceStatus::from_code) {
        None => (0, true, false, "Da generare e inviare"),
        Some(EInvoiceStatus::ToSend) => (1, true, false, "XML pronto, da inviare"),
        Some(EInvoiceStatus::Sent) => (2, true, false, "Inviata, in attesa di esito"),
        Some(EInvoiceStatus::Rejected) => (2, false, true, "Scartata dallo SdI"),
        Some(EInvoiceStatus::Refused) => (2, false, true, "Rifiutata dalla PA"),
        Some(EInvoiceStatus::NotDelivered) => (3, false, false, "Messa a disposizione"),
        Some(EInvoiceStatus::Undeliverable) => (3, false, false, "Non recapitabile"),
        Some(EInvoiceStatus::Accepted) => (3, false, false, "Accettata dalla PA"),
        Some(EInvoiceStatus::Expired) => (3, false, false, "Decorrenza termini"),
        Some(EInvoiceStatus::Delivered) => (3, false, false, "Consegnata"),
        Some(EInvoiceStatus::Received) => (3, false, false, "Ricevuta"),
        Some(EInvoiceStatus::Registered) => (3, false, false, "Registrata"),
    }
}

/// Piccolo indicatore a tre tacche con la descrizione dello stato SdI.
pub fn sdi_steps(ui: &mut Ui, code: Option<&str>) {
    let (done, current, ko, text) = sdi_progress(code);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(3.0 * 16.0 + 2.0 * 3.0, 14.0), Sense::hover());
    for i in 0..3 {
        let x = rect.left() + i as f32 * 19.0;
        let r = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - 3.0), Vec2::new(16.0, 6.0));
        // completate in nero, quella in corso in blu, lo scarto in rosso
        let c = if i < done {
            PRIMARY
        } else if i == done && current {
            ACCENT
        } else if i == done && ko {
            NEGATIVE
        } else {
            TRACK
        };
        ui.painter().rect_filled(r, CornerRadius::same(2), c);
    }
    ui.label(if ko { RichText::new(text).color(NEGATIVE).family(medium_family()) } else { RichText::new(text).color(INK) });
}

/// Separatore orizzontale tenue (tra righe di elenchi).
pub fn rule(ui: &mut Ui) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0_f32, RULE));
}

/// Marchio dell'applicazione (disegnato, nessuna immagine esterna).
pub fn mark(ui: &mut Ui, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same((size * 0.22) as u8), PRIMARY);
    let s = size / 28.0;
    let line = |y: f32, x2: f32| {
        p.line_segment([rect.min + Vec2::new(7.0 * s, y * s), rect.min + Vec2::new(x2 * s, y * s)], Stroke::new(2.0 * s, Color32::WHITE));
    };
    line(9.0, 21.0);
    line(14.0, 21.0);
    line(19.0, 15.0);
    p.rect_filled(egui::Rect::from_min_size(rect.min + Vec2::new(17.0 * s, 17.0 * s), Vec2::splat(4.0 * s)), CornerRadius::same(1), ACCENT);
    resp
}

/// Grafico a colonne semplice: griglia tenue, un solo colore, l'ultima colonna (periodo in corso) in blu.
pub fn bar_chart(ui: &mut Ui, labels: &[String], values: &[f64], height: f32, fmt: impl Fn(f64) -> String) {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let p = ui.painter_at(rect.expand(2.0));
    let max = values.iter().cloned().fold(0.0_f64, f64::max);
    // tacche tonde sopra il massimo
    let raw = if max <= 0.0 { 1.0 } else { max / 3.0 };
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag);
    let mut top = (max / step).ceil() * step;
    if top <= max {
        top += step;
    }
    let axis_w = 48.0;
    let label_h = 20.0;
    let plot = egui::Rect::from_min_max(egui::pos2(rect.left() + axis_w, rect.top() + 6.0), egui::pos2(rect.right(), rect.bottom() - label_h));
    let font = FontId::new(12.0, FontFamily::Proportional);
    let mut t = 0.0;
    while t <= top + step / 2.0 {
        let y = plot.bottom() - (t / top) as f32 * plot.height();
        p.hline(plot.x_range(), y, Stroke::new(1.0_f32, if t == 0.0 { CONTROL } else { RULE }));
        p.text(egui::pos2(plot.left() - 8.0, y), egui::Align2::RIGHT_CENTER, fmt(t), font.clone(), MUTED);
        t += step;
    }
    let n = values.len().max(1);
    let slot = plot.width() / n as f32;
    let bw = (slot * 0.56).min(44.0);
    let hover = resp.hover_pos();
    for (i, v) in values.iter().enumerate() {
        let cx = plot.left() + slot * (i as f32 + 0.5);
        let h = (*v / top) as f32 * plot.height();
        let r = egui::Rect::from_min_max(egui::pos2(cx - bw / 2.0, plot.bottom() - h), egui::pos2(cx + bw / 2.0, plot.bottom()));
        let c = if i + 1 == values.len() { ACCENT } else { CHART_MUTED };
        p.rect_filled(r, CornerRadius::ZERO, c);
        p.text(egui::pos2(cx, rect.bottom() - 2.0), egui::Align2::CENTER_BOTTOM, &labels[i], font.clone(), MUTED);
        if let Some(h) = hover {
            if (h.x - cx).abs() < slot / 2.0 && plot.y_range().contains(h.y) {
                egui::show_tooltip_at_pointer(ui.ctx(), ui.layer_id(), egui::Id::new("bar_tip"), |ui| {
                    ui.label(format!("{}: {}", labels[i], fmt(*v)));
                });
            }
        }
    }
}
