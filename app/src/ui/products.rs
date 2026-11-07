//! Articoli, servizi e listini.

use std::collections::HashMap;

use eframe::egui::{self, RichText};
use openfatt_core::model::{Component, PartyRole, Product, ProductKind, Tracking, VariantBarcode};
use openfatt_core::money::fmt_it_trim;
use rust_decimal::Decimal;

use super::grid::{Col, Grid, Val};
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
}

impl Default for State {
    fn default() -> Self {
        State { search: String::new(), selected: None, editing: None, confirm_delete: None, show_inactive: false, bulk_pct: Decimal::ZERO, bulk_open: false, grid: Grid::new("articoli"), multi: None, comp_search: String::new(), data: Default::default(), stock_cache: Default::default(), suppliers: Default::default(), attachments: None, photo: None }
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
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    super::title(ui, "Articoli e listini");
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
                ui.label("Listini aggiuntivi");
                ui.horizontal(|ui| {
                    while p.extra_prices.len() < 3 {
                        p.extra_prices.push(Decimal::ZERO);
                    }
                    for i in 0..3 {
                        ui.label(format!("L{}", i + 2));
                        widgets::dec_field(ui, ("lp", i), &mut p.extra_prices[i], 80.0, 6);
                    }
                });
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
                ui.label("Fornitore abituale");
                let current = p.supplier_id.and_then(|id| cx.store.party(id).ok().flatten()).map(|x| x.display_name()).unwrap_or_default();
                if let Some(s) = widgets::party_picker(ui, cx.store, "prod_supplier", &current, Some(PartyRole::Supplier), 260.0) {
                    p.supplier_id = Some(s.id);
                }
                ui.label("Codice del fornitore");
                ui.add(egui::TextEdit::singleline(&mut p.supplier_code).desired_width(140.0));
                ui.end_row();
            });
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

    ui.horizontal(|ui| {
        if ui.button("\u{E3D4} Nuovo articolo").clicked() {
            st.editing = Some(Product { vat_id: vats.iter().find(|v| v.code == "22").map(|v| v.id), ..Default::default() });
        }
        if ui.button("\u{E3D4} Nuovo servizio").clicked() {
            st.editing = Some(Product { is_service: true, manage_stock: false, unit: String::new(), vat_id: vats.iter().find(|v| v.code == "22").map(|v| v.id), ..Default::default() });
        }
        ui.add(egui::TextEdit::singleline(&mut st.search).hint_text("\u{E30C} codice, descrizione, barcode, categoria").desired_width(300.0));
        ui.checkbox(&mut st.show_inactive, "mostra non attivi");
        if let Some(id) = st.selected {
            if ui.button("\u{E3B4} Modifica").clicked() {
                st.editing = cx.store.product(id).ok().flatten();
            }
            if ui.button("\u{E1CA} Duplica").clicked() {
                if let Ok(Some(mut p)) = cx.store.product(id) {
                    p.id = 0;
                    p.version = 0;
                    p.code.clear();
                    st.editing = Some(p);
                }
            }
            if ui.button("\u{E4A6} Elimina").clicked() {
                st.confirm_delete = Some(id);
            }
        }
        if ui.button("% Variazione prezzi").clicked() {
            st.bulk_open = true;
        }
        if ui.button(format!("{} Listino Metel...", super::theme::icon::FILE_ARROW_DOWN)).on_hover_text("Importa il listino prezzi Metel di un fornitore di materiale elettrico").clicked() {
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
            Some(p) => ("Etichette".into(), vec![p.clone()]),
            None => (format!("Etichette ({})", list.iter().filter(|p| !p.is_service).count()), list.iter().filter(|p| !p.is_service).cloned().collect()),
        };
        if ui.button(format!("{} {}", super::theme::icon::BARCODE, lbl)).on_hover_text("Etichette con prezzo e codice a barre: dell'articolo selezionato, oppure di tutti quelli dell'elenco").clicked() {
            cx.nav.push(super::Nav::Labels(crate::printing::product_items(cx.store, &items)));
        }
    });
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
    let cols: Vec<Col<Product>> = vec![
        Col::text("code", "Codice", 100.0, |p: &Product| p.code.clone()),
        Col::text("desc", "Descrizione", 300.0, |p: &Product| p.description.clone()),
        Col::text("cat", "Categoria", 120.0, |p: &Product| p.category.clone()).group(),
        Col::text("unit", "U.M.", 50.0, |p: &Product| p.unit.clone()).group(),
        Col::money("price", "Prezzo", 90.0, |p: &Product| p.price),
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
        Col::new("margin", "Margine %", 80.0, |p: &Product| if p.price.is_zero() { Val::Text(String::new()) } else { Val::Num(((p.price - p.cost) / p.price * Decimal::ONE_HUNDRED).round_dp(1), 1) }).right().hidden(),
        Col::money("value", "Valore magazzino", 120.0, move |p: &Product| if p.manage_stock { (stock_value.get(&p.id).copied().unwrap_or_default() * p.cost).round_dp(2) } else { Decimal::ZERO }).total().hidden(),
        Col::new("min", "Scorta min.", 80.0, |p: &Product| Val::Num(p.min_stock, 3)).right().hidden(),
        Col::text("barcode", "Codice a barre", 130.0, |p: &Product| p.barcode.clone()).hidden(),
        Col::text("supplier", "Fornitore", 180.0, move |p: &Product| p.supplier_id.and_then(|id| sup_c.iter().find(|s| s.id == id)).map(|s| s.display_name()).unwrap_or_default()).hidden().group(),
        Col::text("sup_code", "Cod. fornitore", 110.0, |p: &Product| p.supplier_code.clone()).hidden(),
        Col::text("kind", "Tipo", 80.0, |p: &Product| if p.is_service { "servizio".into() } else { "articolo".into() }).hidden().group(),
        Col::new("active", "Attivo", 60.0, |p: &Product| Val::Bool(p.active)).hidden().group(),
    ];
    ui.horizontal(|ui| {
        let n = st.grid.selected.len();
        ui.label(RichText::new(if n > 1 { format!("{} articoli, {} selezionati (Ctrl o Maiusc + clic)", list.len(), n) } else { format!("{} articoli", list.len()) }).color(GRAY));
        if n > 1 && ui.button(format!("{} Modifica i {} selezionati", super::theme::icon::PENCIL_SIMPLE_LINE, n)).clicked() {
            st.multi = Some(Multi { manage_stock: (false, true), active: (false, true), ..Default::default() });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            st.grid.toolbar(ui, cx.store, cx.msgs, &cols, list, "Articoli");
        });
    });
    ui.add_space(4.0);
    let avail = ui.available_height();
    st.grid.version = st.data.version.wrapping_mul(31).wrapping_add(st.stock_cache.version);
    let ev = st.grid.show(ui, cx.store, &cols, list, |p| p.id, avail, false, 26.0);
    st.selected = st.grid.single();
    if let Some(id) = ev.opened {
        st.editing = list.iter().find(|p| p.id == id).cloned();
    }

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
