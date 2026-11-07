//! Inventario fisico: sessioni, conteggio (anche con lettore di codici a barre),
//! esportazione/importazione Excel, differenze e chiusura con rettifiche.

use chrono::NaiveDate;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use openfatt_core::money::{fmt_it_trim, parse_dec};
use openfatt_interop::inventory::InventoryRow;
use openfatt_store::{CountRow, InventoryLine, InventoryStatus};
use rust_decimal::Decimal;

use super::widgets::{self, eur, GRAY, GREEN, ORANGE, RED};
use super::Ctx;

#[derive(PartialEq, Clone, Copy, Default)]
enum Filter {
    #[default]
    All,
    Uncounted,
    Differences,
}

pub struct State {
    selected: Option<i64>,
    new_date: NaiveDate,
    new_warehouse: String,
    new_description: String,
    new_category: String,
    creating: bool,
    filter: Filter,
    search: String,
    scan: String,
    scan_qty: Decimal,
    last_scan: Option<String>,
    confirm_close: bool,
    uncounted_as_zero: bool,
    confirm_delete: bool,
    adding: Option<(Option<i64>, String, String, String, Decimal)>,
    focus_scan: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            selected: None,
            new_date: chrono::Local::now().date_naive(),
            new_warehouse: String::new(),
            new_description: String::new(),
            new_category: String::new(),
            creating: false,
            filter: Filter::All,
            search: String::new(),
            scan: String::new(),
            scan_qty: Decimal::ONE,
            last_scan: None,
            confirm_close: false,
            uncounted_as_zero: false,
            confirm_delete: false,
            adding: None,
            focus_scan: false,
        }
    }
}

impl State {
    /// Stato con l'inventario più recente selezionato.
    pub fn latest(store: &openfatt_store::Store) -> Self {
        State { selected: store.inventory_sessions().ok().and_then(|v| v.first().map(|s| s.id)), ..Default::default() }
    }
}

fn to_rows(lines: &[InventoryLine]) -> Vec<InventoryRow> {
    lines
        .iter()
        .map(|l| InventoryRow {
            line_id: l.id,
            code: l.code.clone(),
            description: l.description.clone(),
            category: l.category.clone(),
            variant: l.variant.clone(),
            lot: l.lot.clone(),
            expiry: l.expiry.map(|e| e.format("%d/%m/%Y").to_string()).unwrap_or_default(),
            serial: l.serial.clone(),
            unit: l.unit.clone(),
            barcode: l.barcode.clone(),
            cost: l.cost,
            expected: l.expected,
            counted: l.counted,
            note: l.note.clone(),
        })
        .collect()
}

/// Campo della quantità contata: vuoto = non contato. Restituisce il nuovo valore se confermato.
fn count_field(ui: &mut egui::Ui, line: &InventoryLine, enabled: bool) -> Option<Option<Decimal>> {
    let id = ui.make_persistent_id(("invcount", line.id));
    let mut buf: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_else(|| line.counted.map(|c| fmt_it_trim(c, 0, 3)).unwrap_or_default());
    let valid = buf.trim().is_empty() || parse_dec(&buf).map(|v| !v.is_sign_negative()).unwrap_or(false);
    let r = ui.add_enabled(
        enabled,
        egui::TextEdit::singleline(&mut buf)
            .desired_width(80.0)
            .horizontal_align(egui::Align::RIGHT)
            .hint_text("—")
            .text_color(if valid { ui.visuals().text_color() } else { RED }),
    );
    let mut out = None;
    if r.lost_focus() {
        if valid {
            let v = if buf.trim().is_empty() { None } else { parse_dec(&buf) };
            if v != line.counted {
                out = Some(v);
            }
        }
        ui.data_mut(|d| d.remove::<String>(id));
    } else if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, buf));
    }
    out
}

fn new_session_window(ui: &egui::Ui, cx: &mut Ctx, st: &mut State) {
    if !st.creating {
        return;
    }
    let warehouses = cx.store.warehouses().unwrap_or_default();
    let categories = cx.store.product_categories().unwrap_or_default();
    let mut close = false;
    egui::Window::new("Nuovo inventario").collapsible(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
        egui::Grid::new("inv_new").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Data dell'inventario");
            widgets::date_field(ui, "inv_date", &mut st.new_date);
            ui.end_row();
            ui.label("Magazzino");
            widgets::warehouse_combo(ui, "inv_wh", &mut st.new_warehouse, &warehouses, true, 220.0);
            ui.end_row();
            ui.label("Categoria (facoltativa)");
            egui::ComboBox::from_id_salt("inv_cat").width(220.0).selected_text(if st.new_category.is_empty() { "tutte" } else { st.new_category.as_str() }).show_ui(ui, |ui| {
                ui.selectable_value(&mut st.new_category, String::new(), "tutte");
                for c in &categories {
                    ui.selectable_value(&mut st.new_category, c.clone(), c);
                }
            });
            ui.end_row();
            ui.label("Descrizione");
            ui.add(egui::TextEdit::singleline(&mut st.new_description).desired_width(220.0).hint_text("es. Inventario di fine anno"));
            ui.end_row();
        });
        ui.label(RichText::new("Saranno inclusi tutti gli articoli gestiti a magazzino, anche con giacenza zero o negativa, con una riga per ogni variante, lotto e numero di serie. La giacenza teorica è quella alla data indicata.").small().color(GRAY));
        ui.horizontal(|ui| {
            if ui.button("Crea inventario").clicked() {
                let cat = if st.new_category.is_empty() { None } else { Some(st.new_category.as_str()) };
                if let Some(id) = cx.msgs.check(cx.store.create_inventory(st.new_date, &st.new_warehouse, &st.new_description, cat), "Inventario creato") {
                    st.selected = Some(id);
                    close = true;
                }
            }
            if ui.button("Annulla").clicked() {
                close = true;
            }
        });
    });
    if close {
        st.creating = false;
    }
}

fn add_line_window(ui: &egui::Ui, cx: &mut Ctx, st: &mut State, session_id: i64) {
    let mut close = false;
    if let Some((pid, variant, lot, serial, qty)) = &mut st.adding {
        egui::Window::new("Aggiungi riga all'inventario").collapsible(false).show(ui.ctx(), |ui| {
            let product = pid.and_then(|id| cx.store.product(id).ok().flatten());
            ui.horizontal(|ui| {
                ui.label("Articolo");
                let search_id = ui.make_persistent_id("inv_add_search");
                let mut search: String = ui.data_mut(|d| d.get_temp(search_id)).unwrap_or_default();
                egui::ComboBox::from_id_salt("inv_add_prod")
                    .width(300.0)
                    .selected_text(product.as_ref().map(|p| format!("{} {}", p.code, p.description)).unwrap_or_else(|| "scegli...".into()))
                    .show_ui(ui, |ui| {
                        ui.add(egui::TextEdit::singleline(&mut search).hint_text("cerca").desired_width(280.0)).request_focus();
                        for p in cx.store.products(&search, false).unwrap_or_default().into_iter().filter(|p| p.manage_stock).take(50) {
                            if ui.selectable_label(false, format!("{} {}", p.code, p.description)).clicked() {
                                *pid = Some(p.id);
                            }
                        }
                    });
                ui.data_mut(|d| d.insert_temp(search_id, search));
            });
            if let Some(p) = &product {
                egui::Grid::new("inv_add").num_columns(2).show(ui, |ui| {
                    if !p.variants.is_empty() {
                        ui.label("Variante");
                        egui::ComboBox::from_id_salt("inv_add_var").selected_text(if variant.is_empty() { "scegli..." } else { variant.as_str() }).show_ui(ui, |ui| {
                            for c in p.variants.combinations() {
                                ui.selectable_value(variant, c.clone(), c);
                            }
                        });
                        ui.end_row();
                    }
                    match p.tracking {
                        openfatt_core::model::Tracking::Lot => {
                            ui.label("Lotto");
                            ui.text_edit_singleline(lot);
                            ui.end_row();
                        }
                        openfatt_core::model::Tracking::Serial => {
                            ui.label("Numero di serie");
                            ui.text_edit_singleline(serial);
                            ui.end_row();
                        }
                        openfatt_core::model::Tracking::None => {}
                    }
                    ui.label("Quantità contata");
                    widgets::dec_field(ui, "inv_add_q", qty, 80.0, 3);
                    ui.end_row();
                });
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(product.is_some(), egui::Button::new("Aggiungi")).clicked() {
                    let r = cx.store.add_inventory_line(session_id, pid.unwrap(), variant, lot, None, serial, Some(*qty));
                    if cx.msgs.check(r, "Riga aggiunta").is_some() {
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
        st.adding = None;
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    super::title(ui, "Inventario");
    new_session_window(ui, cx, st);
    let sessions = cx.store.inventory_sessions().unwrap_or_default();

    ui.horizontal_top(|ui| {
        // --- elenco delle sessioni
        ui.vertical(|ui| {
            ui.set_width(230.0);
            if ui.button("\u{E3D4} Nuovo inventario").clicked() {
                st.creating = true;
            }
            ui.add_space(4.0);
            for s in &sessions {
                let t = cx.store.inventory_totals(s.id).unwrap_or_default();
                let sel = st.selected == Some(s.id);
                let label = format!(
                    "N. {} del {} · {}\n{} · {}/{} contate{}",
                    s.number,
                    s.date.format("%d/%m/%Y"),
                    s.warehouse,
                    if s.status == InventoryStatus::Open { "aperto" } else { "chiuso" },
                    t.counted,
                    t.lines,
                    if s.description.is_empty() { String::new() } else { format!("\n{}", s.description) }
                );
                if ui.selectable_label(sel, label).clicked() {
                    st.selected = Some(s.id);
                }
                ui.add_space(2.0);
            }
            if sessions.is_empty() {
                ui.label(RichText::new("Nessun inventario. Creane uno per contare la merce, a mano, con il lettore di codici a barre o compilando il file Excel.").color(GRAY));
            }
        });
        ui.separator();
        ui.vertical(|ui| {
            let Some(session) = st.selected.and_then(|id| sessions.iter().find(|s| s.id == id)).cloned() else {
                ui.label(RichText::new("Seleziona o crea un inventario").color(GRAY));
                return;
            };
            session_view(ui, cx, st, &session);
        });
    });
}

fn session_view(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, s: &openfatt_store::InventorySession) {
    let open = s.status == InventoryStatus::Open;
    add_line_window(ui, cx, st, s.id);
    let lines = cx.store.inventory_lines(s.id).unwrap_or_default();
    let totals = cx.store.inventory_totals(s.id).unwrap_or_default();

    if st.confirm_close {
        let mut close = false;
        egui::Window::new("Chiudi inventario").collapsible(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
            ui.label(format!(
                "Verranno registrate le rettifiche di magazzino alla data {} nel magazzino {}.\nRighe con differenza: {} (più € {}, meno € {} al costo).",
                s.date.format("%d/%m/%Y"),
                s.warehouse,
                totals.with_difference,
                eur(totals.plus_value),
                eur(totals.minus_value)
            ));
            let uncounted = totals.lines - totals.counted;
            if uncounted > 0 {
                ui.label(RichText::new(format!("{} righe non sono state contate:", uncounted)).color(ORANGE));
                ui.radio_value(&mut st.uncounted_as_zero, false, "lasciare invariata la loro giacenza");
                ui.radio_value(&mut st.uncounted_as_zero, true, "considerarle a zero (merce non trovata)");
            }
            ui.horizontal(|ui| {
                if ui.button("Chiudi e registra le rettifiche").clicked() {
                    match cx.store.close_inventory(s.id, st.uncounted_as_zero) {
                        Ok(a) => cx.msgs.ok(format!(
                            "Inventario chiuso: {} rettifiche (+{} / -{} pezzi)",
                            a.adjustments,
                            fmt_it_trim(a.plus_qty, 0, 3),
                            fmt_it_trim(a.minus_qty, 0, 3)
                        )),
                        Err(e) => cx.msgs.err(e.to_string()),
                    }
                    close = true;
                }
                if ui.button("Annulla").clicked() {
                    close = true;
                }
            });
        });
        if close {
            st.confirm_close = false;
        }
    }
    if st.confirm_delete {
        match widgets::confirm(ui.ctx(), "Elimina inventario", "Eliminare l'inventario e tutti i conteggi inseriti?") {
            Some(true) => {
                if cx.msgs.check(cx.store.delete_inventory(s.id), "Inventario eliminato").is_some() {
                    st.selected = None;
                }
                st.confirm_delete = false;
                return;
            }
            Some(false) => st.confirm_delete = false,
            None => {}
        }
    }

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("Inventario n. {} del {} · magazzino {}", s.number, s.date.format("%d/%m/%Y"), s.warehouse)).strong().size(16.0));
        ui.label(RichText::new(if open { "APERTO" } else { "CHIUSO" }).color(if open { ORANGE } else { GREEN }).strong());
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button("\u{E4C0} Esporta Excel").on_hover_text("Tutte le righe, anche con giacenza zero: compilare la colonna 'Contato' e reimportare").clicked() {
            let name = format!("Inventario {} {} {}.xlsx", s.number, s.date.format("%Y-%m-%d"), s.warehouse);
            if let Some(p) = rfd::FileDialog::new().add_filter("Excel", &["xlsx"]).set_file_name(name).save_file() {
                let title = format!("Inventario n. {} del {} - magazzino {}", s.number, s.date.format("%d/%m/%Y"), s.warehouse);
                match openfatt_interop::inventory::export_xlsx(&p, &title, &to_rows(&lines)) {
                    Ok(_) => cx.msgs.ok(format!("Esportate {} righe in {}", lines.len(), p.display())),
                    Err(e) => cx.msgs.err(e),
                }
            }
        }
        if ui.button("\u{E4C0} Esporta CSV").clicked() {
            if let Some(p) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name(format!("Inventario {}.csv", s.number)).save_file() {
                cx.msgs.check(std::fs::write(&p, openfatt_interop::inventory::export_csv(&to_rows(&lines))), "File CSV esportato");
            }
        }
        if ui.add_enabled(open, egui::Button::new("\u{E20C} Importa conteggi")).on_hover_text("File Excel (.xlsx, .xls), OpenDocument (.ods) o CSV con la colonna 'Contato'").clicked() {
            if let Some(p) = rfd::FileDialog::new().add_filter("Foglio di calcolo", &["xlsx", "xlsm", "xls", "ods", "csv"]).pick_file() {
                match openfatt_interop::inventory::import_counts(&p) {
                    Ok(read) => {
                        let rows: Vec<CountRow> = read
                            .rows
                            .iter()
                            .map(|r| CountRow {
                                file_row: r.file_row,
                                line_id: r.line_id,
                                code: r.code.clone(),
                                variant: r.variant.clone(),
                                lot: r.lot.clone(),
                                expiry: r.expiry.clone(),
                                serial: r.serial.clone(),
                                counted: r.counted,
                                note: r.note.clone(),
                            })
                            .collect();
                        match cx.store.import_inventory_counts(s.id, &rows) {
                            Ok(res) => {
                                let mut errs = read.warnings.clone();
                                errs.extend(res.errors.clone());
                                let msg = format!("Conteggi importati: {} righe aggiornate, {} aggiunte", res.updated, res.added);
                                if errs.is_empty() {
                                    cx.msgs.ok(msg);
                                } else {
                                    cx.msgs.info_dialog("Importazione conteggi", format!("{}\n\nRighe non importate ({}):\n{}", msg, errs.len(), errs.join("\n")));
                                }
                            }
                            Err(e) => cx.msgs.err(e.to_string()),
                        }
                    }
                    Err(e) => cx.msgs.err(e),
                }
            }
        }
        if open && ui.button("\u{E3D4} Riga").on_hover_text("Lotto, seriale o variante trovati ma non presenti nell'elenco").clicked() {
            st.adding = Some((None, String::new(), String::new(), String::new(), Decimal::ONE));
        }
        if open && ui.button("\u{E036} Aggiorna giacenze").on_hover_text("Ricalcola la giacenza teorica alla data (dopo movimenti inseriti in ritardo)").clicked() {
            if let Some(n) = cx.msgs.check(cx.store.refresh_inventory_expected(s.id), "") {
                cx.msgs.ok(format!("Giacenze teoriche aggiornate su {} righe", n));
            }
        }
        if open {
            if ui.button("\u{E182} Chiudi inventario").clicked() {
                st.confirm_close = true;
            }
            if ui.button("\u{E4A6} Elimina").clicked() {
                st.confirm_delete = true;
            }
        } else if ui.button("\u{E038} Riapri").on_hover_text("Annulla le rettifiche registrate e permette di correggere i conteggi").clicked() {
            if let Some(n) = cx.msgs.check(cx.store.reopen_inventory(s.id), "") {
                cx.msgs.ok(format!("Inventario riaperto: {} rettifiche annullate", n));
            }
        }
    });

    // riepilogo
    ui.horizontal_wrapped(|ui| {
        let pct = if totals.lines == 0 { 0 } else { totals.counted * 100 / totals.lines };
        ui.label(RichText::new(format!("Contate {} di {} righe ({}%)", totals.counted, totals.lines, pct)).strong());
        ui.separator();
        ui.label(RichText::new(format!("Con differenza: {}", totals.with_difference)).color(if totals.with_difference > 0 { ORANGE } else { GRAY }));
        ui.separator();
        ui.label(RichText::new(format!("Eccedenze € {}", eur(totals.plus_value))).color(GREEN));
        ui.label(RichText::new(format!("Ammanchi € {}", eur(totals.minus_value))).color(RED));
        ui.separator();
        ui.label(format!("Valore inventario al costo € {}", eur(totals.stock_value)));
    });

    // lettore di codici a barre
    if open {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Conteggio con lettore").strong());
            let r = ui.add(egui::TextEdit::singleline(&mut st.scan).desired_width(220.0).hint_text("leggi codice a barre o seriale"));
            if st.focus_scan {
                r.request_focus();
                st.focus_scan = false;
            }
            ui.label("quantità per lettura");
            widgets::dec_field(ui, "scan_qty", &mut st.scan_qty, 50.0, 3);
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !st.scan.trim().is_empty() {
                match cx.store.inventory_scan(s.id, &st.scan, st.scan_qty) {
                    Ok(l) => {
                        st.last_scan = Some(format!(
                            "{} {} {} → contati {}",
                            l.code,
                            l.description,
                            [l.variant.as_str(), l.lot.as_str(), l.serial.as_str()].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" "),
                            fmt_it_trim(l.counted.unwrap_or_default(), 0, 3)
                        ))
                    }
                    Err(e) => cx.msgs.warn(e.to_string()),
                }
                st.scan.clear();
                st.focus_scan = true;
            }
            if let Some(l) = &st.last_scan {
                ui.label(RichText::new(l).color(GREEN));
            }
        });
    }

    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut st.search).hint_text("\u{E30C} codice, descrizione, lotto, seriale").desired_width(240.0));
        ui.selectable_value(&mut st.filter, Filter::All, "Tutte");
        ui.selectable_value(&mut st.filter, Filter::Uncounted, "Da contare");
        ui.selectable_value(&mut st.filter, Filter::Differences, "Con differenza");
    });
    let q = st.search.trim().to_lowercase();
    let shown: Vec<&InventoryLine> = lines
        .iter()
        .filter(|l| match st.filter {
            Filter::All => true,
            Filter::Uncounted => l.counted.is_none(),
            Filter::Differences => l.difference().map(|d| !d.is_zero()).unwrap_or(false),
        })
        .filter(|l| q.is_empty() || [&l.code, &l.description, &l.lot, &l.serial, &l.variant, &l.barcode].iter().any(|x| x.to_lowercase().contains(&q)))
        .collect();

    let avail = ui.available_height();
    let mut updates: Vec<(i64, Option<Decimal>)> = Vec::new();
    TableBuilder::new(ui)
        .striped(true)
        .max_scroll_height(avail)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(70.0))
        .column(Column::initial(190.0).at_least(110.0).clip(true))
        .column(Column::exact(120.0).clip(true))
        .column(Column::exact(60.0))
        .column(Column::exact(92.0))
        .column(Column::exact(70.0))
        .column(Column::exact(76.0))
        .column(Column::remainder().at_least(80.0).clip(true))
        .header(22.0, |mut h| {
            for t in ["Codice", "Descrizione", "Variante / lotto / serie", "Teorica", "Contata", "Differenza", "Valore diff.", "Note"] {
                h.col(|ui| {
                    ui.strong(t);
                });
            }
        })
        .body(|body| {
            body.rows(26.0, shown.len(), |mut row| {
                let l = shown[row.index()];
                row.col(|ui| {
                    ui.label(&l.code);
                });
                row.col(|ui| {
                    ui.label(&l.description);
                });
                row.col(|ui| {
                    let mut parts = Vec::new();
                    if !l.variant.is_empty() {
                        parts.push(l.variant.clone());
                    }
                    if !l.lot.is_empty() {
                        parts.push(format!("lotto {}{}", l.lot, l.expiry.map(|e| format!(" ({})", e.format("%d/%m/%y"))).unwrap_or_default()));
                    }
                    if !l.serial.is_empty() {
                        parts.push(format!("S/N {}", l.serial));
                    }
                    ui.label(RichText::new(parts.join(" · ")).small());
                });
                row.col(|ui| {
                    let t = fmt_it_trim(l.expected, 0, 3);
                    ui.label(if l.expected.is_sign_negative() { RichText::new(t).color(RED) } else { RichText::new(t).color(GRAY) });
                });
                row.col(|ui| {
                    if let Some(v) = count_field(ui, l, open) {
                        updates.push((l.id, v));
                    }
                });
                row.col(|ui| match l.difference() {
                    Some(d) if d.is_zero() => {
                        ui.label(RichText::new("\u{E182}").color(GREEN));
                    }
                    Some(d) => {
                        ui.label(RichText::new(format!("{}{}", if d > Decimal::ZERO { "+" } else { "" }, fmt_it_trim(d, 0, 3))).strong().color(if d > Decimal::ZERO { GREEN } else { RED }));
                    }
                    None => {
                        ui.label("");
                    }
                });
                row.col(|ui| {
                    if let Some(d) = l.difference().filter(|d| !d.is_zero()) {
                        ui.label(eur(d * l.cost));
                    }
                });
                row.col(|ui| {
                    ui.label(RichText::new(&l.note).small());
                });
            });
        });
    for (id, v) in updates {
        cx.msgs.check(cx.store.set_inventory_count(id, v, None), "");
    }
}

/// Esportazione/importazione da riga di comando (sessione indicata con il suo numero).
pub fn cli(action: &str, archive: &str, number: &str, file: &str) -> Result<String, String> {
    let store = openfatt_store::Store::open(archive).map_err(|e| e.to_string())?;
    let n: i64 = number.parse().map_err(|_| "numero di inventario non valido".to_string())?;
    let session = store.inventory_sessions().map_err(|e| e.to_string())?.into_iter().find(|s| s.number == n).ok_or("inventario non trovato")?;
    let path = std::path::Path::new(file);
    if action == "--inventario-esporta" {
        let lines = store.inventory_lines(session.id).map_err(|e| e.to_string())?;
        if file.to_lowercase().ends_with(".csv") {
            std::fs::write(path, openfatt_interop::inventory::export_csv(&to_rows(&lines))).map_err(|e| e.to_string())?;
        } else {
            openfatt_interop::inventory::export_xlsx(path, &format!("Inventario n. {}", session.number), &to_rows(&lines))?;
        }
        Ok(format!("{} righe esportate", lines.len()))
    } else {
        let read = openfatt_interop::inventory::import_counts(path)?;
        let rows: Vec<CountRow> = read
            .rows
            .iter()
            .map(|r| CountRow { file_row: r.file_row, line_id: r.line_id, code: r.code.clone(), variant: r.variant.clone(), lot: r.lot.clone(), expiry: r.expiry.clone(), serial: r.serial.clone(), counted: r.counted, note: r.note.clone() })
            .collect();
        let res = store.import_inventory_counts(session.id, &rows).map_err(|e| e.to_string())?;
        let t = store.inventory_totals(session.id).map_err(|e| e.to_string())?;
        Ok(format!(
            "{} righe aggiornate, {} aggiunte, {} errori{}; contate {} di {}",
            res.updated,
            res.added,
            res.errors.len() + read.warnings.len(),
            if res.errors.is_empty() && read.warnings.is_empty() { String::new() } else { format!(" ({})", [read.warnings, res.errors].concat().join("; ")) },
            t.counted,
            t.lines
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfatt_core::dec;

    /// Il caso segnalato: l'esportazione per l'aggiornamento in Excel deve contenere anche gli
    /// articoli a giacenza zero, e il file compilato deve rientrare aggiornando le giacenze.
    #[test]
    fn excel_round_trip_with_zero_stock_items() {
        let s = openfatt_store::Store::open_in_memory().unwrap();
        crate::demo::populate(&s).unwrap();
        // un articolo mai caricato (giacenza zero)
        let nuovo = s.save_product(&openfatt_core::model::Product { code: "ZERO1".into(), description: "Articolo a giacenza zero".into(), cost: dec!(3), ..Default::default() }).unwrap();
        let today = chrono::Local::now().date_naive();
        let sid = s.create_inventory(today, "", "Prova Excel", None).unwrap();
        let lines = s.inventory_lines(sid).unwrap();
        let zero_line = lines.iter().find(|l| l.product_id == nuovo.id).expect("la riga a giacenza zero deve esserci");
        assert_eq!(zero_line.expected, dec!(0));

        let dir = std::env::temp_dir().join(format!("ga-inv-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exported = dir.join("inventario.xlsx");
        openfatt_interop::inventory::export_xlsx(&exported, "Prova", &to_rows(&lines)).unwrap();
        // il file esportato contiene tutte le righe, compresa quella a zero
        let read = openfatt_interop::inventory::import_counts(&exported).unwrap();
        assert_eq!(read.rows.len(), lines.len());
        assert!(read.rows.iter().any(|r| r.code == "ZERO1"));

        // l'utente compila il file: 6 pezzi trovati dell'articolo a zero, 10 sedie; righe in altro
        // ordine e solo una parte di esse (filtro di Excel)
        let mut filled: Vec<InventoryRow> = to_rows(&lines)
            .into_iter()
            .filter(|r| r.code == "ZERO1" || r.code == "SED01")
            .map(|mut r| {
                r.counted = Some(if r.code == "ZERO1" { dec!(6) } else { dec!(12) });
                r
            })
            .collect();
        filled.reverse();
        let compiled = dir.join("inventario compilato.xlsx");
        openfatt_interop::inventory::export_xlsx(&compiled, "Prova", &filled).unwrap();
        let read = openfatt_interop::inventory::import_counts(&compiled).unwrap();
        let rows: Vec<CountRow> = read.rows.iter().map(|r| CountRow { file_row: r.file_row, line_id: r.line_id, code: r.code.clone(), counted: r.counted, ..Default::default() }).collect();
        let res = s.import_inventory_counts(sid, &rows).unwrap();
        assert!(res.errors.is_empty(), "{:?}", res.errors);
        assert_eq!(res.updated, filled.len());

        let sedia = s.product_by_code("SED01").unwrap().unwrap();
        let before_sedia = s.stock_in(sedia.id, openfatt_store::DEFAULT_WAREHOUSE).unwrap();
        s.close_inventory(sid, false).unwrap();
        assert_eq!(s.stock_of(nuovo.id).unwrap(), dec!(6));
        assert_eq!(before_sedia, dec!(10));
        assert_eq!(s.stock_in(sedia.id, openfatt_store::DEFAULT_WAREHOUSE).unwrap(), dec!(12));
        let _ = std::fs::remove_dir_all(dir);
    }
}
