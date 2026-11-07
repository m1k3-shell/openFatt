//! Magazzino: giacenze, impegni, movimenti e rettifiche.

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use openfatt_core::model::{DocKind, StockMove, Warehouse};
use openfatt_core::money::fmt_it_trim;
use rust_decimal::Decimal;

use super::widgets::{self, eur, GRAY, ORANGE, RED};
use super::{Ctx, Nav};

#[derive(Default)]
pub struct State {
    search: String,
    selected: Option<i64>,
    only_under_min: bool,
    adjusting: Option<(StockMove, bool)>,
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

    let mut close = false;
    if let Some((m, inventory)) = &mut st.adjusting {
        let prod = cx.store.product(m.product_id).ok().flatten();
        let pname = prod.as_ref().map(|p| format!("{} {}", p.code, p.description)).unwrap_or_default();
        egui::Window::new("Movimento di magazzino").collapsible(false).show(ui.ctx(), |ui| {
            ui.label(RichText::new(&pname).strong());
            ui.horizontal(|ui| {
                ui.radio_value(inventory, false, "Carico/scarico");
                ui.radio_value(inventory, true, "Inventario (quantità contata)");
            });
            egui::Grid::new("adj").num_columns(2).show(ui, |ui| {
                ui.label("Data");
                widgets::date_field(ui, "adj_date", &mut m.date);
                ui.end_row();
                ui.label("Magazzino");
                widgets::warehouse_combo(ui, "adj_wh", &mut m.warehouse, &warehouses, true, 200.0);
                ui.end_row();
                ui.label(if *inventory { "Quantità contata" } else { "Quantità (+ carico, - scarico)" });
                widgets::dec_field(ui, "adj_qty", &mut m.qty, 100.0, 4);
                ui.end_row();
                if !*inventory {
                    ui.label("Causale");
                    ui.add(egui::TextEdit::singleline(&mut m.reason).desired_width(220.0).hint_text("es. Carico iniziale"));
                    ui.end_row();
                    ui.label("Valore unitario");
                    widgets::dec_field(ui, "adj_val", &mut m.unit_value, 100.0, 4);
                    ui.end_row();
                }
                if let Some(p) = &prod {
                    if !p.variants.is_empty() {
                        ui.label("Variante");
                        egui::ComboBox::from_id_salt("adj_var").selected_text(if m.variant.is_empty() { "scegli..." } else { m.variant.as_str() }).show_ui(ui, |ui| {
                            for c in p.variants.combinations() {
                                ui.selectable_value(&mut m.variant, c.clone(), c);
                            }
                        });
                        ui.end_row();
                    }
                    if p.tracking == openfatt_core::model::Tracking::Serial {
                        ui.label("Numeri di serie");
                        ui.add(egui::TextEdit::singleline(&mut m.serial).desired_width(220.0).hint_text("separati da virgola"));
                        ui.end_row();
                    }
                }
                ui.label("Lotto");
                ui.add(egui::TextEdit::singleline(&mut m.lot).desired_width(140.0).hint_text("in uscita vuoto = automatico"));
                ui.end_row();
                ui.label("Scadenza lotto");
                widgets::opt_date_field(ui, "adj_exp", &mut m.expiry);
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if ui.button("Registra").clicked() {
                    let r = if *inventory { cx.store.set_inventory(m.product_id, m.qty, m.date, &m.warehouse).map(|_| ()) } else { cx.store.add_manual_stock_move(m).map(|_| ()) };
                    if cx.msgs.check(r, "Movimento registrato").is_some() {
                        close = true;
                    }
                }
                if ui.button("Annulla").clicked() {
                    close = true;
                }
            });
        });
    }
    if close {
        st.adjusting = None;
    }

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
            if ui.button("± Movimento / inventario").clicked() {
                st.adjusting = Some((StockMove { product_id: id, qty: Decimal::ZERO, reason: "Rettifica".into(), warehouse: st.warehouse_filter.clone().unwrap_or_default(), ..Default::default() }, false));
            }
        }
        if warehouses.len() > 1 && ui.button("\u{E0A0} Trasferimento").on_hover_text("Sposta merce tra due magazzini").clicked() {
            cx.nav.push(Nav::NewDocument(DocKind::StockTransfer));
        }
        if ui.button("\u{ECD4} Magazzini...").clicked() {
            st.managing = true;
        }
    });
    let rows: Vec<_> = cx.store.stock_report(&st.search, st.warehouse_filter.as_deref()).unwrap_or_default().into_iter().filter(|r| !st.only_under_min || r.under_min).collect();
    let value: Decimal = rows.iter().map(|r| r.value).sum();
    ui.label(RichText::new(format!("{} articoli gestiti · valore al costo € {}", rows.len(), eur(value))).color(GRAY));

    let h = ui.available_height() * 0.6;
    TableBuilder::new(ui)
        .id_salt("stock_table")
        .striped(true)
        .resizable(false)
        .max_scroll_height(h)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(110.0))
        .column(Column::initial(300.0).at_least(100.0))
        .column(Column::initial(90.0))
        .column(Column::initial(90.0))
        .column(Column::initial(90.0))
        .column(Column::initial(90.0))
        .column(Column::initial(90.0))
        .column(Column::remainder())
        .header(22.0, |mut hd| {
            for t in ["Codice", "Descrizione", "Giacenza", "Impegnato", "In arrivo", "Disponibile", "Scorta min.", "Valore"] {
                hd.col(|ui| {
                    ui.strong(t);
                });
            }
        })
        .body(|body| {
            body.rows(22.0, rows.len(), |mut row| {
                let r = &rows[row.index()];
                let sel = st.selected == Some(r.product.id);
                row.col(|ui| {
                    if ui.selectable_label(sel, &r.product.code).clicked() {
                        st.selected = Some(r.product.id);
                    }
                });
                row.col(|ui| {
                    if ui.selectable_label(sel, &r.product.description).clicked() {
                        st.selected = Some(r.product.id);
                    }
                });
                row.col(|ui| {
                    let t = fmt_it_trim(r.stock, 0, 3);
                    ui.label(if r.stock.is_sign_negative() { RichText::new(t).color(RED) } else if r.under_min { RichText::new(t).color(ORANGE) } else { RichText::new(t) });
                });
                row.col(|ui| {
                    ui.label(fmt_it_trim(r.ordered_by_customers, 0, 3));
                });
                row.col(|ui| {
                    ui.label(fmt_it_trim(r.incoming, 0, 3));
                });
                row.col(|ui| {
                    ui.label(fmt_it_trim(r.stock - r.ordered_by_customers + r.incoming, 0, 3));
                });
                row.col(|ui| {
                    ui.label(fmt_it_trim(r.product.min_stock, 0, 3));
                });
                row.col(|ui| {
                    ui.label(eur(r.value));
                });
            });
        });

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
    ui.label(RichText::new(if st.selected.is_some() { "Movimenti dell'articolo selezionato" } else { "Ultimi movimenti" }).strong());
    let moves = cx.store.stock_moves(st.selected, 300).unwrap_or_default();
    let mut delete = None;
    egui::ScrollArea::vertical().id_salt("moves").show(ui, |ui| {
        egui::Grid::new("moves_grid").striped(true).num_columns(6).show(ui, |ui| {
            for t in ["Data", "Articolo", "Quantità", "Causale", "Variante / lotto / serie", ""] {
                ui.strong(t);
            }
            ui.end_row();
            for m in &moves {
                ui.label(m.date.format("%d/%m/%Y").to_string());
                ui.label(cx.store.product(m.product_id).ok().flatten().map(|p| p.code).unwrap_or_default());
                ui.label(RichText::new(fmt_it_trim(m.qty, 0, 3)).color(if m.qty.is_sign_negative() { RED } else { egui::Color32::from_rgb(20, 120, 50) }));
                if let Some(d) = m.document_id {
                    if ui.link(&m.reason).clicked() {
                        cx.nav.push(Nav::OpenDocument(d));
                    }
                } else {
                    ui.label(&m.reason);
                }
                ui.label([m.variant.as_str(), m.lot.as_str(), m.serial.as_str()].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" · "));
                if m.document_id.is_none() && ui.small_button("\u{E4A6}").clicked() {
                    delete = Some(m.id);
                } else {
                    ui.label("");
                }
                ui.end_row();
            }
        });
    });
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
