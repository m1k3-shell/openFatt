//! Articoli, servizi e listini.

use std::collections::HashMap;

use eframe::egui::{self, RichText};
use openfatt_core::model::{Component, PartyRole, Product, ProductKind, Tracking, VariantBarcode};
use openfatt_core::money::fmt_it_trim;
use rust_decimal::Decimal;

use super::grid::{Col, Grid, Val};
use super::theme::{self, icon};
use super::widgets::{self, GRAY, ORANGE};
use super::Ctx;

pub struct State {
    search: String,
    selected: Option<i64>,
    editing: Option<Product>,
    confirm_delete: Option<i64>,
    show_inactive: bool,
    bulk_pct: Decimal,
    bulk_open: bool,
    grid: Grid,
    multi: Option<Multi>,
    comp_search: String,
    data: super::Cache<(String, bool), Vec<Product>>,
    stock_cache: super::Cache<(), HashMap<i64, Decimal>>,
    suppliers: super::Cache<(), Vec<openfatt_core::model::Party>>,
    attachments: Option<super::linetools::Attachments>,
    photo: Option<(i64, Option<egui::TextureHandle>)>,
    /// Listino mostrato nell'elenco (vuoto = prezzo base) e prezzi netti o ivati
    list_sel: Option<i64>,
    ivati: bool,
    list_prices: super::Cache<Option<i64>, HashMap<i64, Decimal>>,
    price_title: (String, &'static str),
    /// Scheda laterale
    panel: Option<Product>,
    panel_dirty: bool,
    panel_gen: u64,
    panel_tab: u8,
    panel_rows: Vec<ListRow>,
}

/// Prezzo di un listino nella scheda dell'articolo.
struct ListRow {
    list: openfatt_core::pricing::PriceList,
    net: Option<Decimal>,
    overridden: bool,
    dirty: bool,
}

impl Default for State {
    fn default() -> Self {
        State { search: String::new(), selected: None, editing: None, confirm_delete: None, show_inactive: false, bulk_pct: Decimal::ZERO, bulk_open: false, grid: Grid::new("articoli"), multi: None, comp_search: String::new(), data: Default::default(), stock_cache: Default::default(), suppliers: Default::default(), attachments: None, photo: None, list_sel: None, ivati: false, list_prices: Default::default(), price_title: (String::new(), ""), panel: None, panel_dirty: false, panel_gen: 0, panel_tab: 0, panel_rows: Vec::new() }
    }
}

/// Modifica di più articoli: ogni campo si applica solo se spuntato.
#[derive(Default)]
struct Multi {
    category: (bool, String),
    vat: (bool, Option<i64>),
    unit: (bool, String),
    supplier: (bool, Option<i64>),
    min_stock: (bool, Decimal),
    manage_stock: (bool, bool),
    active: (bool, bool),
}

impl State {
    /// Mostra un elemento trovato con la ricerca generale.
    pub fn focus(&mut self, id: i64, search: &str) {
        self.search = search.to_string();
        self.selected = Some(id);
        self.editing = None;
        self.panel = None;
        self.panel_dirty = false;
    }
}

/// Ricalcola i prezzi dei listini dell'articolo (quelli inseriti a mano restano).
fn refresh_rows(cx: &Ctx, p: &Product, rows: &mut [ListRow]) {
    for r in rows.iter_mut().filter(|r| !r.overridden) {
        r.net = cx.store.list_price_of(r.list.id, p, Decimal::ZERO).ok().flatten().map(|x| x.0);
    }
}

fn load_rows(cx: &Ctx, p: &Product, lists: &[openfatt_core::pricing::PriceList]) -> Vec<ListRow> {
    let over = if p.id != 0 { cx.store.product_list_overrides(p.id).unwrap_or_default() } else { HashMap::new() };
    let mut rows: Vec<ListRow> = lists.iter().filter(|l| l.active).map(|l| ListRow { list: l.clone(), net: over.get(&l.id).copied(), overridden: over.contains_key(&l.id), dirty: false }).collect();
    // i prezzi specifici possono essere IVA inclusa: si legge sempre il netto calcolato
    for r in rows.iter_mut() {
        r.net = cx.store.list_price_of(r.list.id, p, Decimal::ZERO).ok().flatten().map(|x| x.0).or(r.net);
    }
    rows
}

fn save_panel(cx: &mut Ctx, p: &Product, rows: &mut [ListRow]) -> Option<Product> {
    let saved = cx.msgs.check(cx.store.save_product(p), "Prodotto salvato")?;
    for r in rows.iter_mut().filter(|r| r.dirty) {
        let price = if r.overridden { r.net.map(|n| if r.list.vat_included { n * (Decimal::ONE + vat_of(cx, &saved) / Decimal::ONE_HUNDRED) } else { n }) } else { None };
        cx.msgs.check(cx.store.set_price_list_item(r.list.id, saved.id, Decimal::ZERO, price), "");
        r.dirty = false;
    }
    Some(saved)
}

fn vat_of(cx: &Ctx, p: &Product) -> Decimal {
    p.vat_id.and_then(|id| cx.store.vat_codes().ok()?.into_iter().find(|v| v.id == id)).map(|v| v.rate).unwrap_or_default()
}

fn lab(ui: &mut egui::Ui, t: &str) {
    ui.label(RichText::new(t).size(12.5));
}

/// Campo di testo con menu a tendina delle voci già usate.
fn combo_text(ui: &mut egui::Ui, id: &str, v: &mut String, options: &[String], width: f32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        changed |= ui.add(egui::TextEdit::singleline(v).desired_width(width)).changed();
        ui.push_id(id, |ui| {
            ui.menu_button(icon::CARET_DOWN, |ui| {
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for o in options {
                        if ui.button(o).clicked() {
                            *v = o.clone();
                            changed = true;
                            ui.close_menu();
                        }
                    }
                });
            });
        });
    });
    changed
}

/// Scheda a destra dell'elenco: si modifica direttamente e si salva da sola
/// quando si passa a un altro articolo.
fn side_panel(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, vats: &[openfatt_core::model::VatCode], lists: &[openfatt_core::pricing::PriceList], base_name: &str, stock: &HashMap<i64, Decimal>) {
    let gen = cx.store.generation();
    let wanted = st.selected;
    let current = st.panel.as_ref().map(|p| p.id);
    let new_unsaved = current == Some(0);
    if !new_unsaved && (current != wanted || (st.panel_gen != gen && !st.panel_dirty)) {
        if st.panel_dirty {
            if let Some(p) = st.panel.clone() {
                save_panel(cx, &p, &mut st.panel_rows);
            }
        }
        st.panel = wanted.and_then(|id| cx.store.product(id).ok().flatten());
        st.panel_rows = st.panel.as_ref().map(|p| load_rows(cx, p, lists)).unwrap_or_default();
        st.panel_dirty = false;
    } else if new_unsaved && st.panel_rows.is_empty() {
        if let Some(p) = st.panel.clone() {
            st.panel_rows = load_rows(cx, &p, lists);
        }
    }
    st.panel_gen = cx.store.generation();
    let Some(mut p) = st.panel.take() else {
        ui.centered_and_justified(|ui| {
            ui.label(theme::muted("Seleziona un prodotto dall'elenco"));
        });
        return;
    };
    let mut changed = false;
    let mut prices_changed = false;
    let mut save = false;
    egui::TopBottomPanel::bottom("product_actions").frame(egui::Frame::new().inner_margin(egui::Margin { left: 0, right: 0, top: 6, bottom: 0 })).show_inside(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if st.panel_dirty && theme::primary(ui, icon::FLOPPY_DISK, "Salva").clicked() {
                save = true;
            }
            if p.id != 0 {
                let n = cx.store.attachments("product", p.id).map(|a| a.len()).unwrap_or(0);
                if theme::button(ui, icon::PAPERCLIP, &if n > 0 { format!("Allegati ({})", n) } else { "Allegati...".to_string() }).clicked() {
                    st.attachments = Some(super::linetools::Attachments::new("product", p.id, p.description.clone()));
                }
                if theme::button(ui, icon::IMAGE, "Scheda...").on_hover_text("Foto, varianti, lotti, distinta base").clicked() {
                    st.editing = Some(p.clone());
                }
                if theme::button(ui, icon::COPY, "Duplica").clicked() {
                    let mut c = p.clone();
                    c.id = 0;
                    c.version = 0;
                    c.code.clear();
                    st.editing = Some(c);
                }
                if theme::ghost_colored(ui, true, icon::TRASH, "Elimina", theme::NEGATIVE).clicked() {
                    st.confirm_delete = Some(p.id);
                }
            }
        });
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, t) in ["Prodotto", "Caratteristiche", "Dim. e peso", "Magazzino", "Fornitori"].iter().enumerate() {
            ui.selectable_value(&mut st.panel_tab, i as u8, *t);
        }
    });
    ui.add_space(2.0);
    let w = 268.0_f32;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(ui.available_width() - 8.0);
        match st.panel_tab {
            0 => {
                let cats = cx.store.product_categories().unwrap_or_default();
                let mut mains: Vec<String> = cats.iter().map(|c| c.split(" > ").next().unwrap_or("").to_string()).filter(|c| !c.is_empty()).collect();
                mains.dedup();
                let (mut cat, mut sub) = match p.category.split_once(" > ") {
                    Some((a, b)) => (a.to_string(), b.to_string()),
                    None => (p.category.clone(), String::new()),
                };
                let subs: Vec<String> = cats.iter().filter_map(|c| c.split_once(" > ")).filter(|(a, _)| *a == cat).map(|(_, b)| b.to_string()).collect();
                egui::Grid::new("pr_main").num_columns(2).spacing([6.0, 4.0]).min_col_width(92.0).show(ui, |ui| {
                    lab(ui, "Cod. prodotto");
                    changed |= ui.add(egui::TextEdit::singleline(&mut p.code).hint_text("automatico").desired_width(w)).changed();
                    ui.end_row();
                    lab(ui, "Descrizione");
                    changed |= ui.add(egui::TextEdit::multiline(&mut p.description).desired_rows(3).desired_width(w)).changed();
                    ui.end_row();
                    lab(ui, "Tipologia");
                    let current = if p.is_service { 2 } else if p.manage_stock { 0 } else { 1 };
                    let names = ["Art. con magazzino", "Art. senza magazzino", "Servizio"];
                    let mut sel = current;
                    egui::ComboBox::from_id_salt("pr_type").width(w - 8.0).selected_text(names[sel]).show_ui(ui, |ui| {
                        for (i, n) in names.iter().enumerate() {
                            ui.selectable_value(&mut sel, i, *n);
                        }
                    });
                    if sel != current {
                        p.is_service = sel == 2;
                        p.manage_stock = sel == 0;
                        changed = true;
                    }
                    ui.end_row();
                    lab(ui, "Categoria");
                    let c1 = combo_text(ui, "pr_cat", &mut cat, &mains, w - 34.0);
                    ui.end_row();
                    lab(ui, "Sottocategoria");
                    let c2 = combo_text(ui, "pr_sub", &mut sub, &subs, w - 34.0);
                    ui.end_row();
                    if c1 || c2 {
                        p.category = if sub.trim().is_empty() { cat.trim().to_string() } else { format!("{} > {}", cat.trim(), sub.trim()) };
                        changed = true;
                    }
                    lab(ui, "U.m.");
                    let ums: Vec<String> = ["pz", "kg", "g", "m", "mq", "mc", "lt", "h", "cf", "ct"].iter().map(|x| x.to_string()).collect();
                    changed |= combo_text(ui, "pr_um", &mut p.unit, &ums, 70.0);
                    ui.end_row();
                    lab(ui, "Aliquota Iva");
                    let before = p.vat_id;
                    let text = p.vat_id.and_then(|id| vats.iter().find(|v| v.id == id)).map(|v| format!("{}  {}", v.code, v.description)).unwrap_or_else(|| "-".into());
                    egui::ComboBox::from_id_salt("pr_vat").width(w - 8.0).selected_text(text).show_ui(ui, |ui| {
                        for v in vats.iter().filter(|v| v.active) {
                            ui.selectable_value(&mut p.vat_id, Some(v.id), format!("{}  {}", v.code, v.description));
                        }
                    });
                    if before != p.vat_id {
                        changed = true;
                        prices_changed = true;
                    }
                    ui.end_row();
                });
                // ---- prezzi per listino
                let rate = p.vat_id.and_then(|id| vats.iter().find(|v| v.id == id)).map(|v| v.rate).unwrap_or_default();
                let k = Decimal::ONE + rate / Decimal::ONE_HUNDRED;
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(theme::bold("Prezzi").size(12.5));
                    ui.scope(|ui| {
                        ui.visuals_mut().button_frame = false;
                        ui.menu_button(RichText::new(format!("{} {}", if st.ivati { "Ivati" } else { "Netti" }, icon::CARET_DOWN)).color(ui.visuals().hyperlink_color), |ui| {
                            if ui.selectable_label(!st.ivati, "Netti").clicked() {
                                st.ivati = false;
                                ui.close_menu();
                            }
                            if ui.selectable_label(st.ivati, "Ivati").clicked() {
                                st.ivati = true;
                                ui.close_menu();
                            }
                        });
                    });
                });
                let ivati = st.ivati;
                let margin_txt = |net: Decimal, cost: Decimal| {
                    if net.is_zero() || cost.is_zero() {
                        String::new()
                    } else {
                        format!("{}%", fmt_it_trim(((net - cost) / net * Decimal::ONE_HUNDRED).round_dp(1), 0, 1))
                    }
                };
                let shown = |net: Decimal| if ivati { openfatt_core::money::round2(net * k) } else { net };
                let back = |v: Decimal| if ivati { openfatt_core::money::round_dp(v / k, 6) } else { v };
                egui::Grid::new("pr_prices").num_columns(4).spacing([4.0, 3.0]).min_col_width(40.0).max_col_width(112.0).show(ui, |ui| {
                    lab(ui, "Prezzo acquisto");
                    if widgets::dec_field(ui, "pr_cost", &mut p.cost, 100.0, 4) {
                        changed = true;
                        prices_changed = true;
                    }
                    ui.label("");
                    ui.label(theme::muted("Margine").size(11.5));
                    ui.end_row();
                    let base = p.price;
                    let cost = p.cost;
                    lab(ui, base_name);
                    let mut v = shown(base);
                    if widgets::dec_field(ui, ("pr_base", ivati), &mut v, 100.0, 4) {
                        p.price = back(v);
                        changed = true;
                        prices_changed = true;
                    }
                    let mut set_base: Option<Decimal> = None;
                    ui.menu_button(icon::CARET_DOWN, |ui| {
                        for pct in [10, 20, 30, 40, 50, 80, 100] {
                            if ui.add_enabled(!cost.is_zero(), egui::Button::new(format!("Prezzo acquisto + {}%", pct))).clicked() {
                                set_base = Some(openfatt_core::money::round2(cost * (Decimal::ONE + Decimal::from(pct) / Decimal::ONE_HUNDRED)));
                                ui.close_menu();
                            }
                        }
                    });
                    if let Some(x) = set_base {
                        p.price = x;
                        changed = true;
                        prices_changed = true;
                    }
                    ui.label(theme::muted(margin_txt(p.price, cost)).size(12.0));
                    ui.end_row();
                    let base = p.price;
                    for (i, r) in st.panel_rows.iter_mut().enumerate() {
                        lab(ui, &r.list.name);
                        let net = r.net.unwrap_or(base);
                        let mut v = shown(net);
                        if widgets::dec_field(ui, ("pr_list", r.list.id, ivati), &mut v, 100.0, 4) {
                            r.net = Some(back(v));
                            r.overridden = true;
                            r.dirty = true;
                            changed = true;
                        }
                        let mut set: Option<Option<Decimal>> = None;
                        ui.push_id(("pr_list_menu", i), |ui| {
                            ui.menu_button(icon::CARET_DOWN, |ui| {
                                if r.list.kind == openfatt_core::pricing::PriceListKind::Derived && ui.button("Formula del listino").clicked() {
                                    set = Some(None);
                                    ui.close_menu();
                                }
                                for pct in [5, 10, 15, 20, 25, 30, 40] {
                                    if ui.button(format!("{} - {}%", base_name, pct)).clicked() {
                                        set = Some(Some(openfatt_core::money::round2(base * (Decimal::ONE - Decimal::from(pct) / Decimal::ONE_HUNDRED))));
                                        ui.close_menu();
                                    }
                                }
                                ui.separator();
                                for pct in [10, 20, 30, 50, 80] {
                                    if ui.add_enabled(!cost.is_zero(), egui::Button::new(format!("Prezzo acquisto + {}%", pct))).clicked() {
                                        set = Some(Some(openfatt_core::money::round2(cost * (Decimal::ONE + Decimal::from(pct) / Decimal::ONE_HUNDRED))));
                                        ui.close_menu();
                                    }
                                }
                            });
                        });
                        match set {
                            Some(Some(x)) => {
                                r.net = Some(x);
                                r.overridden = true;
                                r.dirty = true;
                                changed = true;
                            }
                            Some(None) => {
                                r.overridden = false;
                                r.dirty = true;
                                changed = true;
                                prices_changed = true;
                            }
                            None => {}
                        }
                        ui.label(theme::muted(margin_txt(r.net.unwrap_or(base), cost)).size(12.0));
                        ui.end_row();
                    }
                });
                if lists.is_empty() {
                    ui.label(theme::muted("Nessun listino: si creano da Strumenti > Listini prezzi.").size(12.0));
                }
                ui.add_space(4.0);
                lab(ui, "Note");
                changed |= ui.add(egui::TextEdit::multiline(&mut p.notes).desired_rows(4).desired_width(410.0)).changed();
            }
            1 => {
                egui::Grid::new("pr_car").num_columns(2).spacing([6.0, 4.0]).min_col_width(92.0).show(ui, |ui| {
                    lab(ui, "Produttore");
                    changed |= ui.add(egui::TextEdit::singleline(&mut p.producer).desired_width(w)).changed();
                    ui.end_row();
                    lab(ui, "Cod. a barre");
                    changed |= ui.add(egui::TextEdit::singleline(&mut p.barcode).desired_width(w)).changed();
                    ui.end_row();
                    lab(ui, "Sito web");
                    changed |= ui.add(egui::TextEdit::singleline(&mut p.url).desired_width(w)).changed();
                    ui.end_row();
                    lab(ui, "Eco-contributo");
                    changed |= widgets::dec_field(ui, "pr_eco", &mut p.eco_fee, 90.0, 4);
                    ui.end_row();
                    lab(ui, "Stato");
                    ui.horizontal(|ui| {
                        changed |= ui.checkbox(&mut p.active, "Attivo").changed();
                        changed |= ui.checkbox(&mut p.online, "Pubblica sul negozio").changed();
                    });
                    ui.end_row();
                });
            }
            2 => {
                egui::Grid::new("pr_dim").num_columns(4).spacing([6.0, 4.0]).min_col_width(70.0).show(ui, |ui| {
                    lab(ui, "U.m. dimens.");
                    changed |= ui.add(egui::TextEdit::singleline(&mut p.dim_unit).desired_width(60.0).hint_text("cm")).changed();
                    ui.label("");
                    ui.label("");
                    ui.end_row();
                    for (name, id, vals) in [("Netto (L A P)", "pr_dn", &mut p.dim_net), ("Imballo (L A P)", "pr_dp", &mut p.dim_pack)] {
                        lab(ui, name);
                        for (i, x) in vals.iter_mut().enumerate() {
                            changed |= widgets::dec_field(ui, (id, i), x, 66.0, 3);
                        }
                        ui.end_row();
                    }
                    lab(ui, "U.m. peso");
                    changed |= ui.add(egui::TextEdit::singleline(&mut p.weight_unit).desired_width(60.0).hint_text("kg")).changed();
                    ui.label("");
                    ui.label("");
                    ui.end_row();
                    lab(ui, "Peso netto");
                    changed |= widgets::dec_field(ui, "pr_wn", &mut p.weight_net, 66.0, 3);
                    ui.label("");
                    ui.label("");
                    ui.end_row();
                    lab(ui, "Peso lordo");
                    changed |= widgets::dec_field(ui, "pr_wg", &mut p.weight_gross, 66.0, 3);
                    ui.label("");
                    ui.label("");
                    ui.end_row();
                });
            }
            3 => {
                let qty = stock.get(&p.id).copied().unwrap_or_default();
                egui::Grid::new("pr_stock").num_columns(2).spacing([6.0, 4.0]).min_col_width(120.0).show(ui, |ui| {
                    lab(ui, "Giacenza");
                    ui.label(theme::bold(fmt_it_trim(qty, 0, 3)));
                    ui.end_row();
                    lab(ui, "Valore al costo");
                    ui.label(format!("€ {}", widgets::eur((qty * p.cost).round_dp(2))));
                    ui.end_row();
                    lab(ui, "Scorta minima");
                    changed |= widgets::dec_field(ui, "pr_min", &mut p.min_stock, 90.0, 3);
                    ui.end_row();
                    lab(ui, "Giorni per l'ordine");
                    let mut d = Decimal::from(p.order_days);
                    if widgets::dec_field(ui, "pr_days", &mut d, 90.0, 0) {
                        p.order_days = rust_decimal::prelude::ToPrimitive::to_u32(&d).unwrap_or(0);
                        changed = true;
                    }
                    ui.end_row();
                    lab(ui, "Ordina a multipli di");
                    changed |= widgets::dec_field(ui, "pr_mult", &mut p.order_multiple, 90.0, 3);
                    ui.end_row();
                });
            }
            _ => {
                let name = |id: Option<i64>| id.and_then(|i| cx.store.party(i).ok().flatten()).map(|x| x.display_name()).unwrap_or_default();
                if p.supplier_id.is_none() && p.other_suppliers.is_empty() {
                    ui.label(theme::muted("Nessun fornitore indicato."));
                }
                egui::Grid::new("pr_sup").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                    if p.supplier_id.is_some() {
                        ui.label(RichText::new(format!("{} {}", icon::STAR, widgets::truncate(&name(p.supplier_id), 34))));
                        ui.label(&p.supplier_code);
                        ui.label(format!("€ {}", widgets::eur(p.supplier_price)));
                        ui.end_row();
                    }
                    for s in &p.other_suppliers {
                        ui.label(widgets::truncate(&name(Some(s.supplier_id)), 36));
                        ui.label(&s.code);
                        ui.label(format!("€ {}", widgets::eur(s.price)));
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
                if theme::button(ui, icon::PENCIL_SIMPLE, "Gestisci i fornitori...").clicked() {
                    st.editing = Some(p.clone());
                }
            }
        }
    });
    if prices_changed {
        refresh_rows(cx, &p, &mut st.panel_rows);
    }
    if changed {
        st.panel_dirty = true;
    }
    if save {
        if let Some(saved) = save_panel(cx, &p, &mut st.panel_rows) {
            st.selected = Some(saved.id);
            st.grid.select_only(saved.id);
            st.panel_dirty = false;
            p = saved;
        }
    }
    st.panel = Some(p);
}

/// Fornitori dell'articolo: il principale e gli altri, ciascuno con codice, prezzo e note.
fn suppliers_section(ui: &mut egui::Ui, cx: &mut Ctx, p: &mut Product) {
    use openfatt_core::model::ProductSupplier;
    let n = p.supplier_id.is_some() as usize + p.other_suppliers.len();
    egui::CollapsingHeader::new(format!("Fornitori ({})", n)).default_open(true).show(ui, |ui| {
        let name = |store: &openfatt_store::Store, id: Option<i64>| id.and_then(|i| store.party(i).ok().flatten()).map(|x| x.display_name()).unwrap_or_default();
        let mut main_to: Option<usize> = None;
        let mut remove: Option<usize> = None;
        egui::Grid::new("prod_suppliers").num_columns(6).spacing([8.0, 4.0]).show(ui, |ui| {
            for h in ["Fornitore", "Cod. prod. forn.", "Prezzo fornitore", "Note fornitura", "Principale", ""] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            let current = name(cx.store, p.supplier_id);
            if let Some(s) = widgets::party_picker(ui, cx.store, "prod_supplier", &current, Some(PartyRole::Supplier), 230.0) {
                p.supplier_id = Some(s.id);
            }
            ui.add(egui::TextEdit::singleline(&mut p.supplier_code).desired_width(120.0));
            widgets::dec_field(ui, "sup_price", &mut p.supplier_price, 90.0, 4);
            ui.add(egui::TextEdit::singleline(&mut p.supplier_notes).desired_width(180.0));
            ui.label(RichText::new(icon::STAR).color(theme::ACCENT));
            ui.label("");
            ui.end_row();
            for (i, s) in p.other_suppliers.iter_mut().enumerate() {
                let current = name(cx.store, (s.supplier_id != 0).then_some(s.supplier_id));
                if let Some(x) = widgets::party_picker(ui, cx.store, &format!("prod_supplier{}", i), &current, Some(PartyRole::Supplier), 230.0) {
                    s.supplier_id = x.id;
                }
                ui.add(egui::TextEdit::singleline(&mut s.code).desired_width(120.0));
                widgets::dec_field(ui, ("sup_price", i), &mut s.price, 90.0, 4);
                ui.add(egui::TextEdit::singleline(&mut s.notes).desired_width(180.0));
                if ui.small_button("Usa come principale").clicked() {
                    main_to = Some(i);
                }
                if ui.small_button(icon::TRASH).on_hover_text("Rimuovi").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            p.other_suppliers.remove(i);
        }
        if let Some(i) = main_to {
            let s = p.other_suppliers.remove(i);
            if let Some(old) = p.supplier_id {
                p.other_suppliers.push(ProductSupplier { supplier_id: old, code: std::mem::take(&mut p.supplier_code), price: p.supplier_price, notes: std::mem::take(&mut p.supplier_notes) });
            }
            p.supplier_id = Some(s.supplier_id);
            p.supplier_code = s.code;
            p.supplier_price = s.price;
            p.supplier_notes = s.notes;
        }
        if ui.button(format!("{}  Aggiungi fornitore", icon::PLUS)).clicked() {
            p.other_suppliers.push(ProductSupplier::default());
        }
    });
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let vats = cx.store.vat_codes().unwrap_or_default();

    if let Some(p) = &mut st.editing {
        let mut open = true;
        let mut save = false;
        let mut close = false;
        egui::Window::new(if p.id == 0 { "Nuovo articolo" } else { "Modifica articolo" }).open(&mut open).default_width(700.0).collapsible(false).show(ui.ctx(), |ui| {
            egui::Grid::new("prod_form").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Codice");
                ui.add(egui::TextEdit::singleline(&mut p.code).hint_text("automatico").desired_width(140.0));
                ui.label("Codice a barre");
                ui.add(egui::TextEdit::singleline(&mut p.barcode).desired_width(160.0));
                ui.end_row();
                ui.label("Descrizione");
                ui.add(egui::TextEdit::singleline(&mut p.description).desired_width(320.0));
                ui.label("Categoria");
                ui.add(egui::TextEdit::singleline(&mut p.category).desired_width(160.0));
                ui.end_row();
                ui.label("Unità di misura");
                ui.add(egui::TextEdit::singleline(&mut p.unit).desired_width(60.0));
                ui.label("Aliquota IVA");
                let mut vr = p.vat_id.and_then(|id| vats.iter().find(|v| v.id == id)).map(openfatt_core::model::VatRef::from_code).unwrap_or_default();
                if widgets::vat_combo(ui, "prod_vat", &mut vr, &vats, 180.0) {
                    p.vat_id = vr.vat_id;
                }
                ui.end_row();
                ui.label("Prezzo di vendita (netto IVA)");
                widgets::dec_field(ui, "price", &mut p.price, 110.0, 6);
                ui.label("Costo d'acquisto");
                widgets::dec_field(ui, "cost", &mut p.cost, 110.0, 6);
                ui.end_row();
                ui.label("");
                ui.label(theme::muted("I listini si impostano nella scheda a destra"));
                if let Some(v) = p.vat_id.and_then(|id| vats.iter().find(|v| v.id == id)) {
                    ui.label("Prezzo IVA inclusa");
                    let mut gross = openfatt_core::money::round2(p.price * (Decimal::ONE + v.rate / Decimal::ONE_HUNDRED));
                    if widgets::dec_field(ui, "gross", &mut gross, 110.0, 2) {
                        p.price = openfatt_core::money::round_dp(gross / (Decimal::ONE + v.rate / Decimal::ONE_HUNDRED), 6);
                    }
                } else {
                    ui.label("");
                    ui.label("");
                }
                ui.end_row();
                ui.label("Tipo");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut p.is_service, "Servizio");
                    ui.add_enabled(!p.is_service, egui::Checkbox::new(&mut p.manage_stock, "Gestisci magazzino"));
                });
                ui.label("Scorta minima");
                widgets::dec_field(ui, "minstock", &mut p.min_stock, 80.0, 3);
                ui.end_row();
            });
            suppliers_section(ui, cx, p);
            ui.add_enabled_ui(!p.is_service, |ui| {
                egui::CollapsingHeader::new("Varianti, lotti e numeri di serie").default_open(!p.variants.is_empty() || p.tracking != Tracking::None).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Tracciabilità");
                        for t in [Tracking::None, Tracking::Lot, Tracking::Serial] {
                            ui.radio_value(&mut p.tracking, t, t.label());
                        }
                    });
                    let mut v1 = p.variants.values1.join(", ");
                    let mut v2 = p.variants.values2.join(", ");
                    egui::Grid::new("variants").num_columns(3).show(ui, |ui| {
                        ui.label("Primo asse");
                        ui.add(egui::TextEdit::singleline(&mut p.variants.axis1).desired_width(90.0).hint_text("Taglia"));
                        if ui.add(egui::TextEdit::singleline(&mut v1).desired_width(320.0).hint_text("S, M, L, XL")).changed() {
                            p.variants.values1 = v1.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
                        }
                        ui.end_row();
                        ui.label("Secondo asse");
                        ui.add(egui::TextEdit::singleline(&mut p.variants.axis2).desired_width(90.0).hint_text("Colore"));
                        if ui.add(egui::TextEdit::singleline(&mut v2).desired_width(320.0).hint_text("Rosso, Blu")).changed() {
                            p.variants.values2 = v2.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
                        }
                        ui.end_row();
                    });
                    let combos = p.variants.combinations();
                    if !combos.is_empty() {
                        ui.label(RichText::new(format!("{} varianti. Codici a barre per variante (facoltativi):", combos.len())).color(GRAY));
                        egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                            egui::Grid::new("vbarcodes").striped(true).num_columns(2).show(ui, |ui| {
                                for c in &combos {
                                    ui.label(c);
                                    let pos = match p.variants.barcodes.iter().position(|b| &b.variant == c) {
                                        Some(i) => i,
                                        None => {
                                            p.variants.barcodes.push(VariantBarcode { variant: c.clone(), barcode: String::new() });
                                            p.variants.barcodes.len() - 1
                                        }
                                    };
                                    let b = &mut p.variants.barcodes[pos].barcode;
                                    let bad = b.len() == 13 && b.bytes().all(|x| x.is_ascii_digit()) && !openfatt_core::model::ean13_valid(b);
                                    ui.add(egui::TextEdit::singleline(b).desired_width(160.0).text_color(if bad { widgets::RED } else { ui.visuals().text_color() }));
                                    ui.end_row();
                                }
                            });
                        });
                        if ui.button("Genera codici EAN-13 interni (prefisso 2)").on_hover_text("Codici per uso interno al negozio, non validi fuori dal punto vendita").clicked() {
                            let base = if p.id == 0 { (chrono::Local::now().timestamp() % 100_000) as i64 } else { p.id };
                            for (i, c) in combos.iter().enumerate() {
                                let first12 = format!("2{:06}{:05}", base % 1_000_000, i + 1);
                                let check = openfatt_core::model::ean13_check_digit(&first12).unwrap_or(0);
                                if let Some(b) = p.variants.barcodes.iter_mut().find(|b| &b.variant == c) {
                                    if b.barcode.is_empty() {
                                        b.barcode = format!("{}{}", first12, check);
                                    }
                                }
                            }
                        }
                    }
                });
            });
            egui::CollapsingHeader::new("Gruppo, distinta base, confezioni e contributi").default_open(p.kind != ProductKind::Simple || !p.pack_size.is_zero() || !p.eco_fee.is_zero()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for k in [ProductKind::Simple, ProductKind::Kit, ProductKind::Assembled] {
                        ui.radio_value(&mut p.kind, k, k.label());
                    }
                });
                if p.kind != ProductKind::Simple {
                    ui.label(RichText::new(if p.kind == ProductKind::Kit {
                        "Il gruppo inserito in un documento si espande nei componenti. Con un prezzo proprio i componenti vanno a prezzo zero."
                    } else {
                        "Il prodotto si carica a magazzino con un documento di Produzione, che scarica i componenti."
                    }).small().color(GRAY));
                    let mut remove = None;
                    egui::Grid::new("components").striped(true).num_columns(5).spacing([10.0, 4.0]).show(ui, |ui| {
                        for h in ["Componente", "Q.tà", "Costo", "Note", ""] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        for (i, c) in p.components.iter_mut().enumerate() {
                            let cp = cx.store.product(c.product_id).ok().flatten();
                            ui.label(cp.as_ref().map(|x| format!("{}  {}", x.code, widgets::truncate(&x.description, 34))).unwrap_or_else(|| "(eliminato)".into()));
                            widgets::dec_field(ui, ("cq", i), &mut c.qty, 70.0, 4);
                            ui.label(cp.as_ref().map(|x| widgets::eur(x.cost * c.qty)).unwrap_or_default());
                            ui.add(egui::TextEdit::singleline(&mut c.note).desired_width(140.0));
                            if ui.small_button(super::theme::icon::TRASH).clicked() {
                                remove = Some(i);
                            }
                            ui.end_row();
                        }
                    });
                    if let Some(i) = remove {
                        p.components.remove(i);
                    }
                    ui.horizontal(|ui| {
                        let mut picked = None;
                        egui::ComboBox::from_id_salt("comp_add").width(300.0).selected_text("aggiungi un componente...").show_ui(ui, |ui| {
                            ui.add(egui::TextEdit::singleline(&mut st.comp_search).hint_text("codice o descrizione").desired_width(280.0)).request_focus();
                            for c in cx.store.products_page(&st.comp_search, false, 40).unwrap_or_default() {
                                if c.id != p.id && ui.selectable_label(false, format!("{}  {}", c.code, c.description)).clicked() {
                                    picked = Some(c.id);
                                }
                            }
                        });
                        if let Some(id) = picked {
                            p.components.push(Component { product_id: id, qty: Decimal::ONE, note: String::new() });
                        }
                        if p.kind == ProductKind::Assembled && !p.components.is_empty() {
                            let tmp = Product { components: p.components.clone(), ..Default::default() };
                            let c = cx.store.bom_cost(&tmp).unwrap_or_default();
                            ui.label(RichText::new(format!("Costo da distinta € {}", widgets::eur(c))).color(GRAY));
                            if ui.small_button("Usa come costo").clicked() {
                                p.cost = c;
                            }
                            if let Ok(Some(n)) = cx.store.producible(&tmp) {
                                ui.label(RichText::new(format!("· producibili con le giacenze: {}", fmt_it_trim(n, 0, 0))).color(GRAY));
                            }
                        }
                    });
                }
                egui::Grid::new("packs_eco").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Pezzi per confezione");
                    widgets::dec_field(ui, "pack_size", &mut p.pack_size, 80.0, 3);
                    ui.label("Unità della confezione");
                    ui.add(egui::TextEdit::singleline(&mut p.pack_unit).desired_width(60.0).hint_text("CF"));
                    ui.end_row();
                    ui.label("Contributo ambientale per unità");
                    widgets::dec_field(ui, "eco_fee", &mut p.eco_fee, 80.0, 4);
                    ui.label("Descrizione del contributo");
                    ui.add(egui::TextEdit::singleline(&mut p.eco_label).desired_width(200.0).hint_text("Contributo RAEE"));
                    ui.end_row();
                    ui.label("Provvigione agenti %");
                    let mut has = p.commission.is_some();
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut has, "specifica").changed() {
                            p.commission = if has { Some(Decimal::ZERO) } else { None };
                        }
                        if let Some(c) = &mut p.commission {
                            widgets::dec_field(ui, "prod_comm", c, 60.0, 2);
                        }
                    });
                    ui.label("Negozio online");
                    ui.checkbox(&mut p.online, "pubblicato");
                    ui.end_row();
                });
            });
            if p.id != 0 {
                ui.horizontal(|ui| {
                    let photo = cx.store.product_photo(p.id).ok().flatten();
                    if let Some(bytes) = &photo {
                        let reload = st.photo.as_ref().map(|(id, _)| *id != p.id).unwrap_or(true);
                        if reload {
                            let tex = load_texture(ui.ctx(), bytes);
                            st.photo = Some((p.id, tex));
                        }
                        if let Some((_, Some(t))) = &st.photo {
                            ui.add(egui::Image::new(t).max_size(egui::vec2(96.0, 96.0)).corner_radius(6.0));
                        }
                    }
                    ui.vertical(|ui| {
                        if ui.button(format!("{} {}", super::theme::icon::IMAGE, if photo.is_some() { "Cambia foto..." } else { "Aggiungi foto..." })).clicked() {
                            if let Some(f) = rfd::FileDialog::new().add_filter("Immagini", &["png", "jpg", "jpeg"]).pick_file() {
                                if let Ok(data) = std::fs::read(&f) {
                                    let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                                    cx.msgs.check(cx.store.set_product_photo(p.id, &name, Some(&data)), "Foto salvata");
                                    st.photo = None;
                                }
                            }
                        }
                        if photo.is_some() && ui.button("Rimuovi la foto").clicked() {
                            cx.msgs.check(cx.store.set_product_photo(p.id, "", None), "Foto rimossa");
                            st.photo = None;
                        }
                        let n = cx.store.attachments("product", p.id).map(|a| a.len()).unwrap_or(0);
                        if ui.button(format!("{} Allegati ({})...", super::theme::icon::PAPERCLIP, n)).on_hover_text("Schede tecniche, certificati, manuali").clicked() {
                            st.attachments = Some(super::linetools::Attachments::new("product", p.id, p.description.clone()));
                        }
                    });
                });
            }
            ui.checkbox(&mut p.active, "Attivo");
            ui.label("Note");
            ui.add(egui::TextEdit::multiline(&mut p.notes).desired_rows(2).desired_width(f32::INFINITY));
            if p.price > Decimal::ZERO && p.cost > Decimal::ZERO {
                let margin = (p.price - p.cost) / p.price * Decimal::ONE_HUNDRED;
                ui.label(RichText::new(format!("Margine sul prezzo: {}%   Ricarico: {}%", fmt_it_trim(margin, 0, 2), fmt_it_trim((p.price - p.cost) / p.cost * Decimal::ONE_HUNDRED, 0, 2))).color(GRAY));
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("\u{E248} Salva").clicked() {
                    save = true;
                }
                if ui.button("Annulla").clicked() {
                    close = true;
                }
            });
        });
        if save {
            p.extra_prices.retain(|x| !x.is_zero());
            if let Some(saved) = cx.msgs.check(cx.store.save_product(p), "Articolo salvato") {
                st.selected = Some(saved.id);
                close = true;
            }
        }
        if !open || close {
            st.editing = None;
        }
    }

    if let Some(a) = &mut st.attachments {
        if !a.show(ui.ctx(), cx.store, cx.msgs) {
            st.attachments = None;
        }
    }
    if let Some(id) = st.confirm_delete {
        match widgets::confirm(ui.ctx(), "Elimina", "Eliminare l'articolo?") {
            Some(true) => {
                cx.msgs.check(cx.store.delete_product(id), "Articolo eliminato");
                st.confirm_delete = None;
                st.selected = None;
            }
            Some(false) => st.confirm_delete = None,
            None => {}
        }
    }

    let gen = cx.store.generation();
    let key = (st.search.clone(), st.show_inactive);
    let list_rc = st.data.get(gen, &key, || cx.store.products(&key.0, key.1).unwrap_or_default());
    let list: &Vec<Product> = &list_rc;

    if st.bulk_open {
        let mut close = false;
        egui::Window::new("Variazione prezzi").collapsible(false).show(ui.ctx(), |ui| {
            ui.label(format!("Applica una variazione percentuale al prezzo di vendita dei {} articoli in elenco.", list.len()));
            ui.horizontal(|ui| {
                ui.label("Variazione %");
                widgets::dec_field(ui, "bulkpct", &mut st.bulk_pct, 80.0, 2);
            });
            ui.horizontal(|ui| {
                if ui.button("Applica").clicked() {
                    let mut n = 0;
                    for p in list.iter() {
                        let mut p = p.clone();
                        p.price = openfatt_core::money::round_dp(p.price * (Decimal::ONE + st.bulk_pct / Decimal::ONE_HUNDRED), 4);
                        if cx.store.save_product(&p).is_ok() {
                            n += 1;
                        }
                    }
                    cx.msgs.ok(format!("Prezzi aggiornati: {}", n));
                    close = true;
                }
                if ui.button("Annulla").clicked() {
                    close = true;
                }
            });
        });
        if close {
            st.bulk_open = false;
        }
    }

    // elenco con colonne, ordinamento, gruppi ed esportazione
    let stock_rc = st.stock_cache.get(gen, &(), || cx.store.stock_levels().unwrap_or_default());
    let stock: &HashMap<i64, Decimal> = &stock_rc;
    let suppliers_rc = st.suppliers.get(gen, &(), || cx.store.parties(&openfatt_store::PartyFilter { role: Some(PartyRole::Supplier), ..Default::default() }).unwrap_or_default());
    let suppliers: Vec<openfatt_core::model::Party> = (*suppliers_rc).clone();
    if let Some(id) = st.selected {
        if !st.grid.selected.contains(&id) {
            st.grid.select_only(id);
        }
    }
    let vats_c = vats.clone();
    let vats_r = vats.clone();
    let sup_c = suppliers.clone();
    let stock_v = stock.clone();
    let stock_r = stock.clone();
    let stock_value = stock.clone();
    let vat_label = move |id: Option<i64>| id.and_then(|id| vats_c.iter().find(|v| v.id == id)).map(|v| v.code.clone()).unwrap_or_default();
    let vat_rate = move |id: Option<i64>| id.and_then(|id| vats_r.iter().find(|v| v.id == id)).map(|v| v.rate).unwrap_or_default();
    let base_name = cx.store.base_list_name();
    let lists = cx.store.price_lists().unwrap_or_default();
    if st.list_sel.is_some_and(|id| !lists.iter().any(|l| l.id == id)) {
        st.list_sel = None;
    }
    let sel_name = st.list_sel.and_then(|id| lists.iter().find(|l| l.id == id)).map(|l| l.name.clone()).unwrap_or_else(|| base_name.clone());
    let wanted_title = format!("{}{}", sel_name, if st.ivati { " (ivato)" } else { "" });
    if st.price_title.0 != wanted_title {
        // i titoli delle colonne vivono quanto il programma: pochi nomi di listino
        st.price_title = (wanted_title.clone(), Box::leak(wanted_title.into_boxed_str()));
    }
    let price_title = st.price_title.1;
    let sel = st.list_sel;
    let prices_rc = st.list_prices.get(gen, &sel, || match sel {
        Some(id) => cx.store.list_prices_for(id, list).unwrap_or_default(),
        None => HashMap::new(),
    });
    let prices: &HashMap<i64, Decimal> = &prices_rc;
    let ivati = st.ivati;
    let vat_rate2 = vat_rate.clone();
    let net_of = move |p: &Product| if sel.is_some() { prices.get(&p.id).copied().unwrap_or(p.price) } else { p.price };
    let cols: Vec<Col<Product>> = vec![
        Col::text("cat", "Categoria", 210.0, |p: &Product| p.category.replace(" > ", "  »  ")).group(),
        Col::text("code", "Cod.", 90.0, |p: &Product| p.code.clone()),
        Col::text("desc", "Descrizione", 400.0, |p: &Product| p.description.clone()),
        Col::money("price", price_title, 100.0, move |p: &Product| if ivati { (net_of(p) * (Decimal::ONE + vat_rate2(p.vat_id) / Decimal::ONE_HUNDRED)).round_dp(2) } else { net_of(p) }),
        Col::text("producer", "Produttore", 130.0, |p: &Product| p.producer.clone()).group(),
        Col::text("unit", "U.M.", 50.0, |p: &Product| p.unit.clone()).hidden().group(),
        Col::money("price_vat", "Prezzo IVA incl.", 110.0, move |p: &Product| (p.price * (Decimal::ONE + vat_rate(p.vat_id) / Decimal::ONE_HUNDRED)).round_dp(2)).hidden(),
        Col::text("vat", "IVA", 55.0, move |p: &Product| vat_label(p.vat_id)).group(),
        Col::new("stock", "Giacenza", 85.0, move |p: &Product| if p.manage_stock { Val::Num(stock_v.get(&p.id).copied().unwrap_or_default(), 3) } else { Val::Text(String::new()) })
            .right()
            .render(move |ui: &mut egui::Ui, p: &Product| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    if p.manage_stock {
                        let q = stock_r.get(&p.id).copied().unwrap_or_default();
                        let t = fmt_it_trim(q, 0, 3);
                        if !p.min_stock.is_zero() && q < p.min_stock {
                            ui.label(RichText::new(t).color(ORANGE)).on_hover_text("Sotto la scorta minima");
                        } else {
                            ui.label(t);
                        }
                    } else {
                        ui.label(RichText::new("-").color(GRAY));
                    }
                });
            }),
        Col::money("cost", "Costo", 85.0, |p: &Product| p.cost),
        Col::new("margin", "Margine %", 80.0, move |p: &Product| if net_of(p).is_zero() || p.cost.is_zero() { Val::Text(String::new()) } else { Val::Num(((net_of(p) - p.cost) / net_of(p) * Decimal::ONE_HUNDRED).round_dp(1), 1) }).right(),
        Col::money("value", "Valore magazzino", 120.0, move |p: &Product| if p.manage_stock { (stock_value.get(&p.id).copied().unwrap_or_default() * p.cost).round_dp(2) } else { Decimal::ZERO }).total().hidden(),
        Col::new("min", "Scorta min.", 80.0, |p: &Product| Val::Num(p.min_stock, 3)).right().hidden(),
        Col::text("barcode", "Codice a barre", 130.0, |p: &Product| p.barcode.clone()).hidden(),
        Col::text("supplier", "Fornitore", 180.0, move |p: &Product| p.supplier_id.and_then(|id| sup_c.iter().find(|s| s.id == id)).map(|s| s.display_name()).unwrap_or_default()).hidden().group(),
        Col::text("sup_code", "Cod. fornitore", 110.0, |p: &Product| p.supplier_code.clone()).hidden(),
        Col::text("kind", "Tipo", 80.0, |p: &Product| if p.is_service { "servizio".into() } else { "articolo".into() }).hidden().group(),
        Col::new("active", "Attivo", 60.0, |p: &Product| Val::Bool(p.active)).hidden().group(),
    ];
    let n_voci = format!("{} voci", list.len());
    theme::title_row(ui, &[], "Prodotti", Some(theme::subline(&[(&n_voci, false)])), |ui| {
        st.grid.toolbar(ui, cx.store, cx.msgs, &cols, list, "Articoli");
        ui.add(egui::TextEdit::singleline(&mut st.search).hint_text(format!("{}  Cerca codice, descrizione, barcode", icon::MAGNIFYING_GLASS)).desired_width(230.0));
        ui.checkbox(&mut st.show_inactive, "non attivi");
        egui::ComboBox::from_id_salt("prod_mode").width(70.0).selected_text(if st.ivati { "Ivati" } else { "Netti" }).show_ui(ui, |ui| {
            ui.selectable_value(&mut st.ivati, false, "Netti");
            ui.selectable_value(&mut st.ivati, true, "Ivati");
        });
        egui::ComboBox::from_id_salt("prod_list").width(130.0).selected_text(&sel_name).show_ui(ui, |ui| {
            ui.selectable_value(&mut st.list_sel, None, &base_name);
            for l in lists.iter().filter(|l| l.active) {
                ui.selectable_value(&mut st.list_sel, Some(l.id), &l.name);
            }
        });
        ui.label("Listino");
        egui::menu::menu_custom_button(ui, theme::button_widget(icon::PLUS, "Nuovo"), |ui| {
            if ui.button("Articolo").clicked() {
                ui.close_menu();
                st.panel = Some(Product { vat_id: vats.iter().find(|v| v.code == "22").map(|v| v.id), manage_stock: true, ..Default::default() });
                st.panel_dirty = true;
                st.panel_tab = 0;
                st.panel_rows.clear();
                st.selected = None;
                st.grid.selected.clear();
            }
            if ui.button("Servizio").clicked() {
                ui.close_menu();
                st.panel = Some(Product { is_service: true, manage_stock: false, unit: String::new(), vat_id: vats.iter().find(|v| v.code == "22").map(|v| v.id), ..Default::default() });
                st.panel_dirty = true;
                st.panel_tab = 0;
                st.panel_rows.clear();
                st.selected = None;
                st.grid.selected.clear();
            }
        });
        egui::menu::menu_custom_button(ui, theme::button_widget(icon::DOTS_THREE, "Altro"), |ui| {
            if ui.button("Variazione prezzi...").clicked() {
                ui.close_menu();
                st.bulk_open = true;
            }
            if ui.button("Importa listino Metel...").clicked() {
                ui.close_menu();
            if let Some(f) = rfd::FileDialog::new().add_filter("Listino Metel", &["txt", "lsp", "dat", "*"]).pick_file() {
                match std::fs::read(&f).map_err(|e| e.to_string()).and_then(|b| openfatt_interop::metel::parse(&openfatt_interop::metel::decode(&b))) {
                    Ok(list) => {
                        let supplier = cx.store.find_party_by_tax_id(&list.header.vat_number, "").ok().flatten().map(|p| p.id);
                        if let Some(r) = cx.msgs.check(cx.store.import_metel(&list, supplier, Decimal::ZERO, true), "") {
                            cx.msgs.info_dialog(
                                "Listino Metel importato",
                                format!(
                                    "{} {} (decorrenza {}): {} articoli nuovi, {} aggiornati, {} saltati.{}\n\nPrezzo di vendita = prezzo al pubblico; costo = prezzo al rivenditore (applicare gli sconti del fornitore con la variazione prezzi o un listino).",
                                    list.header.brand,
                                    list.header.description,
                                    list.header.valid_from.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default(),
                                    r.created,
                                    r.updated,
                                    r.skipped,
                                    if list.warnings.is_empty() { String::new() } else { format!("\n\nAvvisi:\n{}", list.warnings.iter().take(20).cloned().collect::<Vec<_>>().join("\n")) }
                                ),
                            );
                        }
                    }
                    Err(e) => cx.msgs.err(e),
                }
            }
            }
            let (lbl, items): (String, Vec<openfatt_core::model::Product>) = match st.selected.and_then(|id| list.iter().find(|p| p.id == id)) {
                Some(p) => ("Etichette dell'articolo...".into(), vec![p.clone()]),
                None => (format!("Etichette ({})...", list.iter().filter(|p| !p.is_service).count()), list.iter().filter(|p| !p.is_service).cloned().collect()),
            };
            if ui.button(lbl).clicked() {
                ui.close_menu();
                cx.nav.push(super::Nav::Labels(crate::printing::product_items(cx.store, &items)));
            }
            if st.grid.selected.len() > 1 && ui.button(format!("Modifica i {} selezionati...", st.grid.selected.len())).clicked() {
                ui.close_menu();
                st.multi = Some(Multi { manage_stock: (false, true), active: (false, true), ..Default::default() });
            }
        });
    });
    egui::SidePanel::right("product_panel").resizable(false).exact_width(452.0).frame(egui::Frame::new().fill(theme::SURFACE).inner_margin(8)).show_inside(ui, |ui| side_panel(ui, cx, st, &vats, &lists, &base_name, &stock_rc));
    egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::SURFACE)).show_inside(ui, |ui| {
        let avail = ui.available_height();
        st.grid.version = st.data.version.wrapping_mul(31).wrapping_add(st.stock_cache.version).wrapping_mul(31).wrapping_add(st.list_prices.version).wrapping_mul(2).wrapping_add(st.ivati as u64);
        let ev = st.grid.show(ui, cx.store, &cols, list, |p| p.id, avail, false, 22.0);
        st.selected = st.grid.single().or(if st.panel.as_ref().map(|p| p.id == 0).unwrap_or(false) { None } else { st.selected.filter(|id| st.grid.selected.contains(id)) });
        if let Some(id) = ev.opened {
            st.editing = list.iter().find(|p| p.id == id).cloned();
        }
    });

    // modifica multipla
    let mut close = false;
    if let Some(m) = &mut st.multi {
        let ids: Vec<i64> = st.grid.selected.iter().copied().collect();
        let cats = cx.store.product_categories().unwrap_or_default();
        egui::Window::new(format!("Modifica {} articoli", ids.len())).collapsible(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
            ui.label(RichText::new("Spuntare i dati da cambiare: gli altri restano come sono. Per i prezzi usare «% Variazione prezzi».").color(GRAY));
            ui.add_space(4.0);
            egui::Grid::new("multi_prod").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.checkbox(&mut m.category.0, "Categoria");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut m.category.1).desired_width(180.0));
                    egui::ComboBox::from_id_salt("multi_cat").width(30.0).selected_text("").show_ui(ui, |ui| {
                        for c in &cats {
                            if ui.selectable_label(false, c).clicked() {
                                m.category.1 = c.clone();
                                m.category.0 = true;
                            }
                        }
                    });
                });
                ui.end_row();
                ui.checkbox(&mut m.vat.0, "Aliquota IVA");
                let text = m.vat.1.and_then(|id| vats.iter().find(|v| v.id == id)).map(|v| format!("{} {}", v.code, v.description)).unwrap_or_else(|| "-".into());
                egui::ComboBox::from_id_salt("multi_vat").width(240.0).selected_text(text).show_ui(ui, |ui| {
                    for v in &vats {
                        ui.selectable_value(&mut m.vat.1, Some(v.id), format!("{} {}", v.code, v.description));
                    }
                });
                ui.end_row();
                ui.checkbox(&mut m.unit.0, "Unità di misura");
                ui.add(egui::TextEdit::singleline(&mut m.unit.1).desired_width(80.0));
                ui.end_row();
                ui.checkbox(&mut m.supplier.0, "Fornitore abituale");
                let text = m.supplier.1.and_then(|id| suppliers.iter().find(|s| s.id == id)).map(|s| s.display_name()).unwrap_or_else(|| "nessuno".into());
                egui::ComboBox::from_id_salt("multi_sup").width(240.0).selected_text(text).show_ui(ui, |ui| {
                    ui.selectable_value(&mut m.supplier.1, None, "nessuno");
                    for s in &suppliers {
                        ui.selectable_value(&mut m.supplier.1, Some(s.id), s.display_name());
                    }
                });
                ui.end_row();
                ui.checkbox(&mut m.min_stock.0, "Scorta minima");
                widgets::dec_field(ui, "multi_min", &mut m.min_stock.1, 80.0, 3);
                ui.end_row();
                ui.checkbox(&mut m.manage_stock.0, "Magazzino");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut m.manage_stock.1, true, "gestito");
                    ui.radio_value(&mut m.manage_stock.1, false, "non gestito");
                });
                ui.end_row();
                ui.checkbox(&mut m.active.0, "Stato");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut m.active.1, true, "attivi");
                    ui.radio_value(&mut m.active.1, false, "non attivi");
                });
                ui.end_row();
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let any = m.category.0 || m.vat.0 || m.unit.0 || m.supplier.0 || m.min_stock.0 || m.manage_stock.0 || m.active.0;
                if super::theme::primary_enabled(ui, any, super::theme::icon::CHECK, &format!("Applica a {} articoli", ids.len())).clicked() {
                    let (cat, vat, unit, sup, min, ms, act) = (m.category.clone(), m.vat, m.unit.clone(), m.supplier, m.min_stock, m.manage_stock, m.active);
                    let r = cx.store.update_products(&ids, |p| {
                        if cat.0 {
                            p.category = cat.1.trim().to_string();
                        }
                        if vat.0 && vat.1.is_some() {
                            p.vat_id = vat.1;
                        }
                        if unit.0 {
                            p.unit = unit.1.trim().to_string();
                        }
                        if sup.0 {
                            p.supplier_id = sup.1;
                        }
                        if min.0 {
                            p.min_stock = min.1;
                        }
                        if ms.0 && !p.is_service {
                            p.manage_stock = ms.1;
                        }
                        if act.0 {
                            p.active = act.1;
                        }
                    });
                    if let Some(n) = cx.msgs.check(r, "") {
                        cx.msgs.ok(format!("{} articoli aggiornati", n));
                        close = true;
                    }
                }
                if super::theme::button(ui, super::theme::icon::X, "Annulla").clicked() {
                    close = true;
                }
            });
        });
    }
    if close {
        st.multi = None;
    }
}


/// Decodifica una foto (PNG o JPEG) in una texture per l'anteprima.
pub fn load_texture(ctx: &egui::Context, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory(bytes).ok()?.thumbnail(256, 256).to_rgba8();
    let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
    Some(ctx.load_texture(format!("foto-{}", bytes.len()), ci, egui::TextureOptions::LINEAR))
}

/// Immagine a grandezza utile per lo sfondo dell'editor delle stampe (lato massimo 1600 px).
pub fn load_texture_full(ctx: &egui::Context, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width().max(img.height()) > 1600 { img.thumbnail(1600, 1600) } else { img };
    let img = img.to_rgba8();
    let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
    Some(ctx.load_texture(format!("sfondo-{}", bytes.len()), ci, egui::TextureOptions::LINEAR))
}
