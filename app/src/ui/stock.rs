//! Magazzino: giacenze, impegni, movimenti e rettifiche.

use eframe::egui::{self, RichText};
use openfatt_core::model::{DocKind, StockMove, Warehouse};
use openfatt_core::money::fmt_it_trim;
use rust_decimal::Decimal;

use super::grid::{Col, Grid, Val};
use super::quick::{self, MoveKind, Ref};
use super::widgets::{eur, GRAY, ORANGE, RED};
use super::{Cache, Ctx, Nav};
use openfatt_store::StockRow;

#[derive(Default)]
pub struct State {
    search: String,
    selected: Option<i64>,
    only_under_min: bool,
    grid: Grid,
    moves_grid: Grid,
    data: Cache<(String, Option<String>), Vec<StockRow>>,
    moves: Cache<Option<i64>, Vec<(StockMove, String, String)>>,
    trace_code: String,
    trace: Option<Vec<openfatt_store::TraceRow>>,
    expiring_days: u32,
    warehouse_filter: Option<String>,
    managing: bool,
    wh_edit: Option<Warehouse>,
}

impl State {
    /// Stato con articolo selezionato e/o ricerca di tracciabilità già eseguita.
    pub fn preset(store: &openfatt_store::Store, product_code: Option<&str>, trace: Option<&str>) -> Self {
        let mut st = State::default();
        if let Some(c) = product_code {
            st.selected = store.product_by_code(c).ok().flatten().map(|p| p.id);
        }
        if let Some(t) = trace {
            st.trace_code = t.to_string();
            st.trace = Some(store.trace(t).unwrap_or_default());
        }
        st
    }
}

fn tracking_details(ui: &mut egui::Ui, cx: &mut Ctx, product_id: i64, warehouse: Option<&str>) {
    let Ok(Some(p)) = cx.store.product(product_id) else { return };
    let by_wh = cx.store.stock_by_warehouse(p.id).unwrap_or_default();
    if p.variants.is_empty() && p.tracking == openfatt_core::model::Tracking::None && by_wh.len() < 2 {
        return;
    }
    let today = chrono::Local::now().date_naive();
    ui.horizontal_top(|ui| {
        if by_wh.len() > 1 || warehouse.is_some() {
            ui.vertical(|ui| {
                ui.label(RichText::new("Giacenza per magazzino").strong());
                egui::Grid::new("by_wh").striped(true).show(ui, |ui| {
                    for (w, q) in &by_wh {
                        ui.label(w);
                        ui.label(fmt_it_trim(*q, 0, 3));
                        ui.end_row();
                    }
                });
            });
            ui.add_space(30.0);
        }
        if !p.variants.is_empty() {
            ui.vertical(|ui| {
                ui.label(RichText::new("Giacenza per variante").strong());
                egui::Grid::new("by_var").striped(true).show(ui, |ui| {
                    for (v, q) in cx.store.stock_by_variant(p.id).unwrap_or_default() {
                        ui.label(if v.is_empty() { "(senza variante)".to_string() } else { v });
                        ui.label(fmt_it_trim(q, 0, 3));
                        ui.end_row();
                    }
                });
            });
            ui.add_space(30.0);
        }
        match p.tracking {
            openfatt_core::model::Tracking::Lot => {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Lotti disponibili").strong());
                    egui::Grid::new("lots").striped(true).show(ui, |ui| {
                        for l in cx.store.lots_available(p.id, None, warehouse).unwrap_or_default() {
                            ui.label(&l.lot);
                            ui.label(&l.variant);
                            ui.label(fmt_it_trim(l.qty, 0, 3));
                            match l.expiry {
                                Some(e) => {
                                    let c = if e < today { RED } else if e <= today + chrono::Duration::days(30) { ORANGE } else { ui.visuals().text_color() };
                                    ui.label(RichText::new(format!("scad. {}", e.format("%d/%m/%Y"))).color(c));
                                }
                                None => {
                                    ui.label("");
                                }
                            }
                            ui.end_row();
                        }
                    });
                });
            }
            openfatt_core::model::Tracking::Serial => {
                ui.vertical(|ui| {
                    let list = cx.store.serials_in_stock(p.id, warehouse).unwrap_or_default();
                    ui.label(RichText::new(format!("Numeri di serie a magazzino ({})", list.len())).strong());
                    let text: Vec<String> = list.iter().map(|(s, v)| if v.is_empty() { s.clone() } else { format!("{} ({})", s, v) }).collect();
                    ui.label(text.join(", "));
                });
            }
            openfatt_core::model::Tracking::None => {}
        }
    });
    ui.separator();
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    super::title(ui, "Magazzino");
    let warehouses = cx.store.warehouses().unwrap_or_default();
    manage_window(ui, cx, st, &warehouses);

    st.grid.name = "magazzino";
    st.moves_grid.name = "magazzino-movimenti";
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut st.search).hint_text("\u{E30C} articolo").desired_width(260.0));
        ui.checkbox(&mut st.only_under_min, "solo sotto scorta");
        let text = st.warehouse_filter.clone().unwrap_or_else(|| "Tutti i magazzini".into());
        egui::ComboBox::from_id_salt("wh_filter").width(180.0).selected_text(text).show_ui(ui, |ui| {
            ui.selectable_value(&mut st.warehouse_filter, None, "Tutti i magazzini");
            for w in &warehouses {
                ui.selectable_value(&mut st.warehouse_filter, Some(w.name.clone()), &w.name);
            }
        });
        if let Some(id) = st.selected {
            for (kind, t) in [(MoveKind::Load, "Carica"), (MoveKind::Unload, "Scarica"), (MoveKind::Adjust, "Rettifica")] {
                if ui.button(t).clicked() {
                    quick::open_move(ui.ctx(), kind, id);
                }
            }
        }
        if warehouses.len() > 1 && ui.button("\u{E0A0} Trasferimento").on_hover_text("Sposta merce tra due magazzini").clicked() {
            cx.nav.push(Nav::NewDocument(DocKind::StockTransfer));
        }
        if ui.button("\u{ECD4} Magazzini...").clicked() {
            st.managing = true;
        }
    });
    let key = (st.search.clone(), st.warehouse_filter.clone());
    let rows_rc = st.data.get(cx.store.generation(), &key, || cx.store.stock_report(&key.0, key.1.as_deref()).unwrap_or_default());
    let rows: Vec<&StockRow> = rows_rc.iter().filter(|r| !st.only_under_min || r.under_min).collect();
    let cols: Vec<Col<&StockRow>> = vec![
        Col::text("code", "Codice", 110.0, |r: &&StockRow| r.product.code.clone()).opens(|r: &&StockRow| Some(Ref::Product(r.product.id))),
        Col::text("desc", "Descrizione", 300.0, |r: &&StockRow| r.product.description.clone()).opens(|r: &&StockRow| Some(Ref::Product(r.product.id))),
        Col::text("cat", "Categoria", 180.0, |r: &&StockRow| r.product.category.clone()).hidden().group(),
        Col::text("producer", "Produttore", 130.0, |r: &&StockRow| r.product.producer.clone()).hidden().group(),
        Col::text("location", "Ubicazione", 100.0, |r: &&StockRow| r.product.location.clone()).hidden().group(),
        Col::new("stock", "Giacenza", 85.0, |r: &&StockRow| Val::Num(r.stock, 3)).right(),
        Col::new("committed", "Impegnato", 85.0, |r: &&StockRow| Val::Num(r.ordered_by_customers, 3)).right(),
        Col::new("incoming", "In arrivo", 85.0, |r: &&StockRow| Val::Num(r.incoming, 3)).right(),
        Col::new("available", "Disponibile", 90.0, |r: &&StockRow| Val::Num(r.stock - r.ordered_by_customers, 3)).right(),
        Col::new("min", "Scorta min.", 85.0, |r: &&StockRow| Val::Num(r.product.min_stock, 3)).right(),
        Col::new("under", "Sotto scorta", 80.0, |r: &&StockRow| Val::Bool(r.under_min)).hidden().group(),
        Col::money("cost", "Costo", 85.0, |r: &&StockRow| r.product.cost).hidden(),
        Col::money("value", "Valore", 100.0, |r: &&StockRow| r.value).total(),
    ];
    ui.horizontal(|ui| {
        let value: Decimal = rows.iter().filter(|r| st.grid.accepts(&cols, r)).map(|r| r.value).sum();
        ui.label(RichText::new(format!("{} articoli gestiti · valore al costo € {}", rows.len(), eur(value))).color(GRAY));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            st.grid.toolbar(ui, cx.store, cx.msgs, &cols, &rows, "Magazzino");
        });
    });
    if let (Some(id), true) = (st.selected, st.grid.selected.is_empty()) {
        st.grid.select_only(id);
    }
    let h = ui.available_height() * 0.52;
    st.grid.version = st.data.version.wrapping_mul(2).wrapping_add(st.only_under_min as u64);
    st.grid.show(ui, cx.store, &cols, &rows, |r| r.product.id, h, false, 22.0);
    st.selected = st.grid.single();

    ui.separator();
    if let Some(id) = st.selected {
        tracking_details(ui, cx, id, st.warehouse_filter.as_deref());
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Tracciabilità").strong());
        let r = ui.add(egui::TextEdit::singleline(&mut st.trace_code).hint_text("lotto o numero di serie").desired_width(180.0));
        if ui.button("Cerca").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
            st.trace = Some(cx.store.trace(&st.trace_code).unwrap_or_default());
        }
        ui.separator();
        if st.expiring_days == 0 {
            st.expiring_days = 30;
        }
        let until = chrono::Local::now().date_naive() + chrono::Duration::days(st.expiring_days as i64);
        let exp = cx.store.expiring_lots(until).unwrap_or_default();
        ui.label(RichText::new(format!("Lotti scaduti o in scadenza entro {} giorni: {}", st.expiring_days, exp.len())).color(if exp.is_empty() { GRAY } else { ORANGE }))
            .on_hover_text(exp.iter().map(|(p, l)| format!("{} lotto {} ({}) scad. {}", p.code, l.lot, fmt_it_trim(l.qty, 0, 3), l.expiry.map(|e| e.format("%d/%m/%Y").to_string()).unwrap_or_default())).collect::<Vec<_>>().join("\n"));
    });
    let mut close_trace = false;
    if let Some(rows) = &st.trace {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Percorso di '{}': {} movimenti", st.trace_code.trim(), rows.len())).strong());
                if ui.small_button("chiudi").clicked() {
                    close_trace = true;
                }
            });
            egui::Grid::new("trace").striped(true).num_columns(7).show(ui, |ui| {
                for h in ["Data", "Articolo", "Variante", "Lotto / serie", "Quantità", "Documento", "Soggetto"] {
                    ui.strong(h);
                }
                ui.end_row();
                for r in rows {
                    ui.label(r.date.format("%d/%m/%Y").to_string());
                    ui.label(format!("{} {}", r.product_code, r.product_description));
                    ui.label(&r.variant);
                    ui.label(format!("{}{}", r.lot, if r.serial.is_empty() { String::new() } else { format!(" S/N {}", r.serial) }));
                    ui.label(RichText::new(fmt_it_trim(r.qty, 0, 3)).color(if r.qty.is_sign_negative() { RED } else { egui::Color32::from_rgb(20, 120, 50) }));
                    match r.document_id {
                        Some(d) => {
                            if ui.link(&r.reason).clicked() {
                                cx.nav.push(Nav::OpenDocument(d));
                            }
                        }
                        None => {
                            ui.label(&r.reason);
                        }
                    }
                    ui.label(&r.party_name);
                    ui.end_row();
                }
            });
        });
    }
    if close_trace {
        st.trace = None;
    }
    let selected = st.selected;
    let moves_rc = st.moves.get(cx.store.generation(), &selected, || {
        let names: std::collections::HashMap<i64, (String, String)> = cx.store.products("", true).unwrap_or_default().into_iter().map(|p| (p.id, (p.code, p.description))).collect();
        cx.store.stock_moves(selected, 2000).unwrap_or_default().into_iter().map(|m| {
            let (code, desc) = names.get(&m.product_id).cloned().unwrap_or_default();
            (m, code, desc)
        }).collect()
    });
    type MoveRow = (StockMove, String, String);
    let delete_req = std::cell::Cell::new(None);
    let open_req = std::cell::Cell::new(None);
    let (delete_ref, open_ref) = (&delete_req, &open_req);
    let cols: Vec<Col<MoveRow>> = vec![
        Col::new("date", "Data", 90.0, |r: &MoveRow| Val::Date(Some(r.0.date))),
        Col::text("code", "Articolo", 110.0, |r: &MoveRow| r.1.clone()).opens(|r: &MoveRow| Some(Ref::Product(r.0.product_id))),
        Col::text("desc", "Descrizione", 240.0, |r: &MoveRow| r.2.clone()).opens(|r: &MoveRow| Some(Ref::Product(r.0.product_id))),
        Col::new("qty", "Quantità", 80.0, |r: &MoveRow| Val::Num(r.0.qty, 3)).right().render(|ui: &mut egui::Ui, r: &MoveRow| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(fmt_it_trim(r.0.qty, 0, 3)).color(if r.0.qty.is_sign_negative() { RED } else { egui::Color32::from_rgb(20, 120, 50) }));
            });
        }),
        Col::text("reason", "Causale", 260.0, |r: &MoveRow| r.0.reason.clone()).render(move |ui: &mut egui::Ui, r: &MoveRow| match r.0.document_id {
            Some(d) => {
                if ui.link(&r.0.reason).clicked() {
                    open_ref.set(Some(d));
                }
            }
            None => {
                ui.label(&r.0.reason);
            }
        }),
        Col::text("wh", "Magazzino", 110.0, |r: &MoveRow| r.0.warehouse.clone()).group(),
        Col::text("track", "Variante / lotto / serie", 160.0, |r: &MoveRow| [r.0.variant.as_str(), r.0.lot.as_str(), r.0.serial.as_str()].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" · ")),
        Col::money("value", "Valore unit.", 90.0, |r: &MoveRow| r.0.unit_value).hidden(),
        Col::text("act", "", 34.0, |_: &MoveRow| String::new()).render(move |ui: &mut egui::Ui, r: &MoveRow| {
            if r.0.document_id.is_none() && ui.small_button("\u{E4A6}").on_hover_text("Elimina il movimento").clicked() {
                delete_ref.set(Some(r.0.id));
            }
        }),
    ];
    ui.horizontal(|ui| {
        ui.label(RichText::new(if selected.is_some() { "Movimenti dell'articolo selezionato" } else { "Ultimi movimenti" }).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            st.moves_grid.toolbar(ui, cx.store, cx.msgs, &cols, &moves_rc, "Movimenti di magazzino");
        });
    });
    let h = ui.available_height() - 4.0;
    st.moves_grid.version = st.moves.version;
    st.moves_grid.show(ui, cx.store, &cols, &moves_rc, |r| r.0.id, h, false, 22.0);
    if let Some(d) = open_req.get() {
        cx.nav.push(Nav::OpenDocument(d));
    }
    let delete = delete_req.get();
    if let Some(id) = delete {
        cx.msgs.check(cx.store.delete_manual_stock_move(id), "Movimento eliminato");
    }
}

fn manage_window(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, warehouses: &[Warehouse]) {
    if !st.managing {
        return;
    }
    let mut open = true;
    egui::Window::new("Magazzini").open(&mut open).default_width(560.0).collapsible(false).show(ui.ctx(), |ui| {
        egui::Grid::new("whs").striped(true).num_columns(5).show(ui, |ui| {
            for h in ["Nome", "Indirizzo", "Predefinito", "Attivo", ""] {
                ui.strong(h);
            }
            ui.end_row();
            for w in warehouses {
                ui.label(&w.name);
                ui.label(&w.address);
                ui.label(if w.is_default { "sì" } else { "" });
                ui.label(if w.active { "sì" } else { "no" });
                ui.horizontal(|ui| {
                    if ui.small_button("Modifica").clicked() {
                        st.wh_edit = Some(w.clone());
                    }
                    if !w.is_default && ui.small_button("\u{E4A6}").clicked() {
                        cx.msgs.check(cx.store.delete_warehouse(w.id), "Magazzino eliminato");
                    }
                });
                ui.end_row();
            }
        });
        if ui.button("\u{E3D4} Nuovo magazzino").clicked() {
            st.wh_edit = Some(Warehouse { active: true, ..Default::default() });
        }
        let mut done = false;
        if let Some(w) = &mut st.wh_edit {
            ui.separator();
            egui::Grid::new("wh_form").num_columns(2).show(ui, |ui| {
                ui.label("Nome");
                ui.add(egui::TextEdit::singleline(&mut w.name).desired_width(220.0).hint_text("es. Negozio, Deposito esterno"));
                ui.end_row();
                ui.label("Indirizzo");
                ui.add(egui::TextEdit::singleline(&mut w.address).desired_width(300.0));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut w.is_default, "Predefinito (usato quando il documento non indica un magazzino)");
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut w.active, "Attivo");
                ui.end_row();
                ui.label("Note");
                ui.add(egui::TextEdit::singleline(&mut w.notes).desired_width(300.0));
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if ui.button("\u{E248} Salva").clicked() && cx.msgs.check(cx.store.save_warehouse(w), "Magazzino salvato").is_some() {
                    done = true;
                }
                if ui.button("Annulla").clicked() {
                    done = true;
                }
            });
            ui.label(RichText::new("Rinominando un magazzino si aggiornano anche movimenti e documenti che lo citano.").small().color(GRAY));
        }
        if done {
            st.wh_edit = None;
        }
        ui.separator();
        ui.label(RichText::new("Ultimi trasferimenti").strong());
        let list = cx.store.documents(&openfatt_store::DocFilter { kinds: vec![DocKind::StockTransfer], ..Default::default() }).unwrap_or_default();
        for d in list.iter().take(15) {
            if ui.link(format!("{} n. {} del {} · {}", d.kind.label(), d.number_label, d.date.format("%d/%m/%Y"), d.status.label())).clicked() {
                cx.nav.push(Nav::OpenDocument(d.id));
            }
        }
        if list.is_empty() {
            ui.label(RichText::new("Nessun trasferimento").color(GRAY));
        }
    });
    if !open {
        st.managing = false;
    }
}
