//! Aspetto dell'interfaccia, personale per ogni utente: colore di ogni sezione, ordine e
//! visibilità dei pulsanti della barra delle sezioni, posizione delle finestre.
//!
//! Le scelte si salvano nell'archivio (`Store::set_user_pref`), quindi seguono l'utente
//! su ogni postazione che apre lo stesso archivio.

use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText, Sense, Vec2};
use serde::{Deserialize, Serialize};

use super::theme::{self, icon, Section};
use super::{Ctx, Nav};

const PREF: &str = "aspetto";

/// Pulsanti della barra delle sezioni: (codice, nome, icona, sezione).
pub const DOCK: [(&str, &str, &str, Section); 14] = [
    ("start", "Start", icon::HOUSE, Section::Start),
    ("banco", "Banco", icon::CASH_REGISTER, Section::Counter),
    ("clienti", "Clienti", icon::USERS, Section::Customers),
    ("fornitori", "Fornitori", icon::TRUCK, Section::Suppliers),
    ("prodotti", "Prodotti", icon::PACKAGE, Section::Products),
    ("documenti", "Documenti", icon::FILES, Section::Sales),
    ("fatture_elettroniche", "Fatture elettr.", icon::PAPER_PLANE_TILT, Section::EInvoice),
    ("pagamenti", "Pagamenti", icon::CURRENCY_CIRCLE_DOLLAR, Section::Payments),
    ("magazzino", "Magazzino", icon::WAREHOUSE, Section::Stock),
    ("agenda", "Agenda", icon::CALENDAR_BLANK, Section::Agenda),
    ("analisi", "Analisi", icon::CHART_LINE_UP, Section::Analysis),
    ("strumenti", "Strumenti", icon::WRENCH, Section::Tools),
    ("opzioni", "Opzioni", icon::GEAR_SIX, Section::Options),
    ("archivi", "Archivi", icon::FOLDER_OPEN, Section::Archives),
];

/// Pulsanti che non si possono nascondere (da lì si torna alla scrivania e alle opzioni).
const FIXED: [&str; 2] = ["start", "opzioni"];

/// Colori proposti (oltre al selettore libero).
const SWATCHES: [(u8, u8, u8); 12] = [
    (0x36, 0x4f, 0xc7),
    (0x19, 0x71, 0xc2),
    (0x0c, 0x85, 0x99),
    (0x08, 0x7f, 0x5b),
    (0x2f, 0x7d, 0x32),
    (0x5f, 0x7a, 0x1a),
    (0xa8, 0x5d, 0x00),
    (0xc2, 0x41, 0x0c),
    (0xc2, 0x25, 0x5c),
    (0xa6, 0x1e, 0x4d),
    (0x86, 0x2e, 0x9c),
    (0x49, 0x5a, 0x6e),
];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Colore scelto per sezione (codice della sezione → RGB).
    pub colors: Vec<(String, [u8; 3])>,
    /// Ordine dei pulsanti della barra; vuoto = ordine predefinito.
    pub dock: Vec<DockItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DockItem {
    pub code: String,
    #[serde(default)]
    pub hidden: bool,
}

impl Prefs {
    /// Barra completa nell'ordine scelto: i pulsanti nuovi (versioni successive) in coda.
    pub fn dock_items(&self) -> Vec<DockItem> {
        let mut out: Vec<DockItem> = self.dock.iter().filter(|d| DOCK.iter().any(|x| x.0 == d.code)).cloned().collect();
        out.dedup_by(|a, b| a.code == b.code);
        for (code, ..) in DOCK {
            if !out.iter().any(|d| d.code == code) {
                out.push(DockItem { code: code.to_string(), hidden: false });
            }
        }
        for d in &mut out {
            if FIXED.contains(&d.code.as_str()) {
                d.hidden = false;
            }
        }
        out
    }

    /// Codici dei pulsanti visibili, nell'ordine.
    pub fn visible_dock(&self) -> Vec<&'static str> {
        self.dock_items().into_iter().filter(|d| !d.hidden).filter_map(|d| DOCK.iter().find(|x| x.0 == d.code).map(|x| x.0)).collect()
    }

    fn color(&self, s: Section) -> Option<Color32> {
        self.colors.iter().find(|(c, _)| c == s.code()).map(|(_, [r, g, b])| Color32::from_rgb(*r, *g, *b))
    }

    fn set_color(&mut self, s: Section, c: Option<Color32>) {
        self.colors.retain(|(x, _)| x != s.code());
        if let Some(c) = c {
            self.colors.push((s.code().to_string(), [c.r(), c.g(), c.b()]));
        }
    }

    fn apply(&self, ctx: &egui::Context) {
        let list: Vec<(Section, Color32)> = self.colors.iter().filter_map(|(c, [r, g, b])| Section::from_code(c).map(|s| (s, Color32::from_rgb(*r, *g, *b)))).collect();
        theme::set_colors(ctx, &list);
    }
}

#[derive(Default)]
pub struct State {
    pub prefs: Prefs,
    /// Archivio e utente per cui sono state lette le preferenze.
    loaded: Option<(PathBuf, String)>,
    saved: Prefs,
    /// Momento dell'ultima modifica non ancora salvata.
    dirty_at: Option<f64>,
    pub open: bool,
}

impl State {
    /// Rilegge le preferenze quando cambiano archivio o utente e le applica.
    pub fn sync(&mut self, ctx: &egui::Context, store: Option<&openfatt_store::Store>) {
        let key = store.map(|s| (s.path().to_path_buf(), s.current_user()));
        if key != self.loaded {
            // cambio di archivio o di utente: le modifiche degli ultimi istanti non si salvano
            // nell'archivio nuovo
            self.prefs = store.and_then(|s| s.user_pref::<Prefs>(PREF).ok()).unwrap_or_default();
            self.saved = self.prefs.clone();
            self.loaded = key;
            self.dirty_at = None;
            self.prefs.apply(ctx);
        }
        // salvataggio poco dopo l'ultima modifica (un trascinamento del selettore non
        // scrive a ogni movimento)
        if let (Some(t), Some(s)) = (self.dirty_at, store) {
            let now = ctx.input(|i| i.time);
            if now - t > 0.8 {
                self.flush(Some(s));
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(300));
            }
        }
    }

    pub fn flush(&mut self, store: Option<&openfatt_store::Store>) {
        if self.prefs != self.saved {
            if let Some(s) = store {
                if s.set_user_pref(PREF, &self.prefs).is_ok() {
                    self.saved = self.prefs.clone();
                }
            }
        }
        self.dirty_at = None;
    }

    fn changed(&mut self, ctx: &egui::Context) {
        self.prefs.apply(ctx);
        self.dirty_at = Some(ctx.input(|i| i.time));
    }
}

/// Finestra "Aspetto dell'interfaccia".
pub fn window(ctx: &egui::Context, cx: &mut Ctx, st: &mut State) {
    if !st.open {
        return;
    }
    let mut open = true;
    let mut changed = false;
    egui::Window::new(format!("{}  Aspetto dell'interfaccia", icon::PALETTE))
        .id(egui::Id::new("finestra_aspetto"))
        .open(&mut open)
        .collapsible(false)
        .default_width(760.0)
        .default_height(620.0)
        .default_pos(ctx.screen_rect().center())
        .pivot(egui::Align2::CENTER_CENTER)
        .show(ctx, |ui| {
            ui.label(theme::muted("Le scelte valgono per il tuo utente e si salvano nell'archivio: le ritrovi su ogni computer che lo apre."));
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(ctx.screen_rect().height() * 0.7).show(ui, |ui| {
                ui.columns(2, |c| {
                    let left = egui::Layout::top_down(egui::Align::Min);
                    changed |= c[0].with_layout(left, |ui| colors(ui, &mut st.prefs)).inner;
                    c[1].with_layout(left, |ui| {
                        changed |= dock(ui, &mut st.prefs);
                        ui.add_space(14.0);
                        windows(ui, cx);
                    });
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if theme::button(ui, icon::ARROW_COUNTER_CLOCKWISE, "Ripristina colori e barra").clicked() {
                    st.prefs = Prefs::default();
                    changed = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::primary(ui, icon::CHECK, "Fatto").clicked() {
                        st.open = false;
                    }
                });
            });
        });
    if changed {
        st.changed(ctx);
    }
    if !open {
        st.open = false;
    }
    if !st.open {
        st.flush(Some(cx.store));
    }
}

fn heading(ui: &mut egui::Ui, ic: &str, t: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(ic).size(16.0).color(theme::tint().main));
        ui.label(theme::medium(t).size(15.0));
    });
    ui.add_space(4.0);
}

fn colors(ui: &mut egui::Ui, p: &mut Prefs) -> bool {
    let mut changed = false;
    heading(ui, icon::PALETTE, "Colori delle sezioni");
    ui.label(RichText::new("Colorano la barra, i titoli, i pulsanti principali e le righe selezionate. Rosso, verde e arancio restano per gli avvisi. I colori troppo chiari vengono scuriti per tenere leggibile il testo bianco.").size(12.5).color(theme::MUTED));
    ui.add_space(6.0);
    for s in Section::ALL {
        let cur = s.main();
        let custom = p.color(s).is_some();
        ui.horizontal(|ui| {
            ui.set_min_height(30.0);
            let (r, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
            ui.painter().rect_filled(r, 7, theme::mix(Color32::WHITE, cur, 0.15));
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, s.icon(), egui::FontId::proportional(15.0), cur);
            ui.allocate_ui_with_layout(Vec2::new(150.0, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_width(150.0);
                ui.add(egui::Label::new(RichText::new(s.label()).color(theme::INK)).truncate());
            });
            // colore scelto così com'è (quello mostrato può essere stato scurito): il
            // selettore non deve tornare indietro mentre si trascina verso un colore chiaro
            let raw = p.color(s).unwrap_or(s.default_main());
            let mut rgb = [raw.r(), raw.g(), raw.b()];
            if ui.color_edit_button_srgb(&mut rgb).on_hover_text("Scegli un colore qualsiasi").changed() {
                p.set_color(s, Some(Color32::from_rgb(rgb[0], rgb[1], rgb[2])));
                changed = true;
            }
            let id = ui.make_persistent_id(("aspetto_tavolozza", s.code()));
            let resp = ui.add(egui::Button::new(RichText::new(icon::CARET_DOWN).size(12.0)).frame(false)).on_hover_text("Colori proposti");
            if resp.clicked() {
                ui.memory_mut(|m| m.toggle_popup(id));
            }
            egui::popup_below_widget(ui, id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                ui.set_min_width(190.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(4.0);
                    for (r, g, b) in SWATCHES {
                        let c = Color32::from_rgb(r, g, b);
                        let (rr, resp) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
                        ui.painter().circle_filled(rr.center(), 10.0, c);
                        if resp.clicked() {
                            p.set_color(s, Some(c));
                            changed = true;
                        }
                    }
                });
            });
            if custom && ui.add(egui::Button::new(RichText::new("Predefinito").size(12.0).color(theme::MUTED)).frame(false)).on_hover_text("Torna al colore originale della sezione").clicked() {
                p.set_color(s, None);
                changed = true;
            }
        });
    }
    changed
}

fn dock(ui: &mut egui::Ui, p: &mut Prefs) -> bool {
    let mut changed = false;
    heading(ui, icon::SQUARES_FOUR, "Barra delle sezioni");
    ui.label(RichText::new("Ordina i pulsanti con le frecce e togli quelli che non usi. Start e Opzioni restano sempre.").size(12.5).color(theme::MUTED));
    ui.add_space(6.0);
    let mut items = p.dock_items();
    let n = items.len();
    let mut swap: Option<(usize, usize)> = None;
    for (i, it) in items.iter_mut().enumerate() {
        let Some(&(code, label, ic, sec)) = DOCK.iter().find(|x| x.0 == it.code) else { continue };
        ui.horizontal(|ui| {
            ui.set_min_height(28.0);
            ui.add_enabled_ui(i > 0, |ui| {
                if ui.add(egui::Button::new(icon::ARROW_UP).frame(false)).on_hover_text("Sposta a sinistra").clicked() {
                    swap = Some((i, i - 1));
                }
            });
            ui.add_enabled_ui(i + 1 < n, |ui| {
                if ui.add(egui::Button::new(icon::ARROW_DOWN).frame(false)).on_hover_text("Sposta a destra").clicked() {
                    swap = Some((i, i + 1));
                }
            });
            let fixed = FIXED.contains(&code);
            let mut shown = !it.hidden;
            ui.add_enabled_ui(!fixed, |ui| {
                if ui.checkbox(&mut shown, "").on_hover_text(if fixed { "Sempre visibile" } else { "Mostra nella barra" }).changed() {
                    it.hidden = !shown;
                    changed = true;
                }
            });
            let c = if it.hidden { theme::CONTROL } else { sec.main() };
            ui.label(RichText::new(ic).size(16.0).color(c));
            ui.label(if it.hidden { RichText::new(label).color(theme::MUTED).strikethrough() } else { RichText::new(label).color(theme::INK) });
        });
    }
    if let Some((a, b)) = swap {
        items.swap(a, b);
        changed = true;
    }
    if changed {
        p.dock = items;
    }
    changed
}

fn windows(ui: &mut egui::Ui, cx: &mut Ctx) {
    heading(ui, icon::APP_WINDOW, "Finestre");
    ui.label(RichText::new("Le finestre (incassi, stampe, ricerca...) si spostano trascinandole dal titolo e si riaprono dove le hai lasciate. Le animazioni valgono solo per questo computer.").size(12.5).color(theme::MUTED));
    ui.add_space(6.0);
    if theme::button(ui, icon::ARROWS_IN, "Rimetti le finestre al centro").on_hover_text("Dimentica le posizioni e le misure scelte per le finestre").clicked() {
        ui.ctx().memory_mut(|m| m.reset_areas());
        cx.msgs.ok("Le finestre si riapriranno nella posizione iniziale");
    }
    ui.add_space(4.0);
    let mut on = theme::motion();
    if ui.checkbox(&mut on, "Animazioni (transizioni e menu che scendono)").changed() {
        cx.nav.push(Nav::Motion(on));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn barra_nell_ordine_scelto_con_start_e_opzioni_sempre_visibili() {
        let p = Prefs {
            colors: vec![],
            dock: vec![
                DockItem { code: "agenda".into(), hidden: false },
                DockItem { code: "start".into(), hidden: true },
                DockItem { code: "analisi".into(), hidden: true },
                DockItem { code: "pulsante_futuro".into(), hidden: false },
            ],
        };
        let v = p.visible_dock();
        assert_eq!(&v[..2], &["agenda", "start"]);
        assert!(!v.contains(&"analisi"));
        assert!(v.contains(&"opzioni"));
        assert_eq!(v.len(), DOCK.len() - 1);
    }

    #[test]
    fn colori_troppo_chiari_resi_leggibili() {
        let c = theme::readable(Color32::from_rgb(0xff, 0xf3, 0x80));
        assert!(c.r() < 0xc0 && c.g() < 0xc0, "{:?}", c);
        let dark = Color32::from_rgb(0x19, 0x71, 0xc2);
        assert_eq!(theme::readable(dark), dark);
    }
}
