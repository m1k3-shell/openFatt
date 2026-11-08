//! Listini prezzi: definizione, prezzi specifici per articolo, scaglioni di quantità.

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use openfatt_core::money::round2;
use openfatt_core::pricing::{PriceBase, PriceList, PriceListKind};
use rust_decimal::Decimal;

use super::widgets::{self, eur, GRAY, ORANGE};
use super::Ctx;

#[derive(Default)]
pub struct State {
    selected: Option<i64>,
    editing: Option<PriceList>,
    search: String,
    breaks_for: Option<i64>,
    new_break: (Decimal, Decimal),
    confirm_delete: Option<i64>,
}

impl State {
    /// Stato con un listino già selezionato (per nome).
    pub fn preset(store: &openfatt_store::Store, name: &str) -> Self {
        let l = store.price_lists().unwrap_or_default().into_iter().find(|l| l.name.eq_ignore_ascii_case(name));
        State { selected: l.as_ref().map(|l| l.id), editing: l, ..Default::default() }
    }
}

const ROUNDINGS: [(&str, i64, u32); 6] = [("nessuno", 0, 0), ("0,01", 1, 2), ("0,05", 5, 2), ("0,10", 10, 2), ("0,50", 50, 2), ("1,00", 100, 2)];

fn rounding_label(d: Decimal) -> String {
    ROUNDINGS.iter().find(|(_, n, s)| Decimal::new(*n, *s) == d).map(|(l, _, _)| l.to_string()).unwrap_or_else(|| openfatt_core::money::fmt_it_trim(d, 0, 2))
}

fn list_form(ui: &mut egui::Ui, l: &mut PriceList, all: &[PriceList]) {
    egui::Grid::new("pl_form").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Nome");
        ui.add(egui::TextEdit::singleline(&mut l.name).desired_width(240.0).hint_text("es. Rivenditori"));
        ui.end_row();
        ui.label("Tipo");
        ui.horizontal(|ui| {
            ui.radio_value(&mut l.kind, PriceListKind::Derived, "Calcolato con formula");
            ui.radio_value(&mut l.kind, PriceListKind::Manual, "Prezzi inseriti a mano");
        });
        ui.end_row();
        if l.kind == PriceListKind::Derived {
            ui.label("Calcolato da");
            let base_text = match l.base {
                PriceBase::SalePrice => "Prezzo di vendita".to_string(),
                PriceBase::Cost => "Costo d'acquisto".to_string(),
                PriceBase::List(id) => format!("Listino {}", all.iter().find(|x| x.id == id).map(|x| x.name.clone()).unwrap_or_default()),
            };
            egui::ComboBox::from_id_salt("pl_base").width(240.0).selected_text(base_text).show_ui(ui, |ui| {
                ui.selectable_value(&mut l.base, PriceBase::SalePrice, "Prezzo di vendita");
                ui.selectable_value(&mut l.base, PriceBase::Cost, "Costo d'acquisto");
                for o in all.iter().filter(|o| o.id != l.id) {
                    ui.selectable_value(&mut l.base, PriceBase::List(o.id), format!("Listino {}", o.name));
                }
            });
            ui.end_row();
            ui.label("Variazione %");
            ui.horizontal(|ui| {
                widgets::dec_field(ui, "pl_pct", &mut l.percent, 70.0, 3);
                ui.label(RichText::new("negativa = sconto, positiva = ricarico").small().color(GRAY));
            });
            ui.end_row();
            ui.label("Arrotondamento");
            egui::ComboBox::from_id_salt("pl_round").selected_text(rounding_label(l.round_to)).show_ui(ui, |ui| {
                for (lab, n, sc) in ROUNDINGS {
                    ui.selectable_value(&mut l.round_to, Decimal::new(n, sc), lab);
                }
            });
            ui.end_row();
        }
        ui.label("Prezzi");
        ui.checkbox(&mut l.vat_included, "IVA inclusa (listino al pubblico)");
        ui.end_row();
        ui.label("Valido dal / al");
        ui.horizontal(|ui| {
            widgets::opt_date_field(ui, "pl_from", &mut l.valid_from);
            ui.label("–");
            widgets::opt_date_field(ui, "pl_to", &mut l.valid_to);
        });
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut l.active, "Attivo");
        ui.end_row();
        ui.label("Note");
        ui.add(egui::TextEdit::singleline(&mut l.notes).desired_width(240.0));
        ui.end_row();
    });
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    super::title(ui, "Listini");
    let lists = cx.store.price_lists().unwrap_or_default();

    if let Some(id) = st.confirm_delete {
        match widgets::confirm(ui.ctx(), "Elimina listino", "Eliminare il listino e i suoi prezzi specifici?") {
            Some(true) => {
                if cx.msgs.check(cx.store.delete_price_list(id), "Listino eliminato").is_some() {
                    st.selected = None;
                    st.editing = None;
                }
                st.confirm_delete = None;
            }
            Some(false) => st.confirm_delete = None,
            None => {}
        }
    }

    ui.horizontal_top(|ui| {
        // --- elenco e scheda del listino
        ui.vertical(|ui| {
            ui.set_width(420.0);
            if ui.button("\u{E3D4} Nuovo listino").clicked() {
                st.editing = Some(PriceList { name: "Nuovo listino".into(), percent: Decimal::from(-10), ..Default::default() });
                st.selected = None;
            }
            egui::Grid::new("pl_list").striped(true).num_columns(3).show(ui, |ui| {
                for l in &lists {
                    let base_name = match l.base {
                        PriceBase::List(id) => lists.iter().find(|x| x.id == id).map(|x| x.name.clone()).unwrap_or_default(),
                        _ => String::new(),
                    };
                    if ui.selectable_label(st.selected == Some(l.id), RichText::new(&l.name).strong()).clicked() {
                        st.selected = Some(l.id);
                        st.editing = Some(l.clone());
                        st.breaks_for = None;
                    }
                    ui.label(RichText::new(l.describe(&base_name)).small().color(GRAY));
                    let n = cx.store.price_list_usage(l.id).unwrap_or(0);
                    ui.label(RichText::new(format!("{} soggetti{}", n, if l.active { "" } else { " · non attivo" })).small().color(GRAY));
                    ui.end_row();
                }
            });
            if lists.is_empty() {
                ui.label(RichText::new("Nessun listino. Esempi: 'Rivenditori' = prezzo di vendita −15%; 'Ingrosso' = costo +30%; 'Pubblico' IVA inclusa arrotondato a 0,10.").color(GRAY));
            }
            ui.separator();
            if let Some(l) = &mut st.editing {
                list_form(ui, l, &lists);
                ui.horizontal(|ui| {
                    if ui.button("\u{E248} Salva listino").clicked() {
                        if let Some(saved) = cx.msgs.check(cx.store.save_price_list(l), "Listino salvato") {
                            st.selected = Some(saved.id);
                            *l = saved;
                        }
                    }
                    if l.id != 0 && ui.button("\u{E4A6} Elimina").clicked() {
                        st.confirm_delete = Some(l.id);
                    }
                });
                ui.label(RichText::new("Il listino si assegna al cliente o al fornitore nella sua scheda. Le condizioni particolari del soggetto hanno la precedenza sul listino.").small().color(GRAY));
            }
        });
        ui.separator();
        // --- prezzi per articolo
        ui.vertical(|ui| {
            let Some(list) = st.selected.and_then(|id| lists.iter().find(|l| l.id == id)).cloned() else {
                ui.label(RichText::new("Selezionare un listino per vedere e modificare i prezzi degli articoli").color(GRAY));
                return;
            };
            let items = cx.store.price_list_items(list.id).unwrap_or_default();
            let products = cx.store.products(&st.search, false).unwrap_or_default();
            let vats = cx.store.vat_codes().unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Prezzi del listino {}", list.name)).strong());
                ui.add(egui::TextEdit::singleline(&mut st.search).hint_text("\u{E30C} articolo").desired_width(200.0));
                if ui.button("\u{E20C} Esporta CSV").clicked() {
                    let mut csv = String::from("Codice;Descrizione;U.M.;Prezzo netto;IVA;Prezzo IVA inclusa;Da quantità\n");
                    for p in &products {
                        let rate = p.vat_id.and_then(|id| vats.iter().find(|v| v.id == id)).map(|v| v.rate).unwrap_or(Decimal::from(22));
                        let mut qtys: Vec<Decimal> = items.iter().filter(|i| i.product_id == p.id && i.min_qty > Decimal::ONE).map(|i| i.min_qty).collect();
                        qtys.insert(0, Decimal::ONE);
                        for q in qtys {
                            if let Ok(Some((net, _))) = cx.store.list_price_of(list.id, p, q) {
                                csv.push_str(&format!(
                                    "{};{};{};{};{};{};{}\n",
                                    p.code,
                                    p.description.replace(';', ","),
                                    p.unit,
                                    eur(round2(net)),
                                    rate.normalize(),
                                    eur(round2(net * (Decimal::ONE + rate / Decimal::ONE_HUNDRED))),
                                    q.normalize()
                                ));
                            }
                        }
                    }
                    if let Some(path) = rfd::FileDialog::new().set_file_name(format!("Listino {}.csv", list.name)).save_file() {
                        cx.msgs.check(std::fs::write(path, csv), "Listino esportato");
                    }
                }
            });
            ui.label(RichText::new("Prezzo specifico: sostituisce la formula per quell'articolo. Lasciarlo vuoto per usare la formula.").small().color(GRAY));
            let avail = ui.available_height();
            TableBuilder::new(ui)
                .striped(true)
                .max_scroll_height(avail)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(70.0))
                .column(Column::initial(170.0).at_least(100.0).clip(true))
                .column(Column::exact(64.0))
                .column(Column::exact(70.0))
                .column(Column::exact(122.0))
                .column(Column::exact(70.0))
                .column(Column::remainder().at_least(80.0))
                .header(22.0, |mut h| {
                    for t in ["Codice", "Descrizione", "Base", "Listino", if list.vat_included { "Specifico (ivato)" } else { "Specifico (netto)" }, "Ivato", "Scaglioni"] {
                        h.col(|ui| {
                            ui.strong(t);
                        });
                    }
                })
                .body(|body| {
                    body.rows(26.0, products.len(), |mut row| {
                        let p = &products[row.index()];
                        let rate = p.vat_id.and_then(|id| vats.iter().find(|v| v.id == id)).map(|v| v.rate).unwrap_or(Decimal::from(22));
                        let specific = items.iter().find(|i| i.product_id == p.id && i.min_qty <= Decimal::ONE).map(|i| i.price);
                        let price = cx.store.list_price_of(list.id, p, Decimal::ONE).ok().flatten().map(|x| x.0);
                        row.col(|ui| {
                            super::quick::link(ui, p.code.clone(), super::quick::Ref::Product(p.id));
                        });
                        row.col(|ui| {
                            super::quick::link(ui, p.description.clone(), super::quick::Ref::Product(p.id));
                        });
                        row.col(|ui| {
                            let base = match list.base {
                                PriceBase::Cost => p.cost,
                                _ => p.price,
                            };
                            ui.label(RichText::new(eur(base)).color(GRAY));
                        });
                        row.col(|ui| match price {
                            Some(v) => {
                                ui.label(RichText::new(eur(round2(v))).strong());
                            }
                            None => {
                                ui.label(RichText::new("—").color(ORANGE)).on_hover_text("Nessun prezzo: vale il prezzo base dell'articolo");
                            }
                        });
                        row.col(|ui| {
                            let id = ui.make_persistent_id(("plspec", list.id, p.id));
                            let mut buf: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_else(|| specific.map(|v| openfatt_core::money::fmt_it_trim(v, 2, 4)).unwrap_or_default());
                            let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(72.0).horizontal_align(egui::Align::RIGHT).hint_text("formula"));
                            if r.lost_focus() {
                                let value = if buf.trim().is_empty() { None } else { openfatt_core::money::parse_dec(&buf) };
                                if buf.trim().is_empty() || value.is_some() {
                                    cx.msgs.check(cx.store.set_price_list_item(list.id, p.id, Decimal::ZERO, value), "");
                                }
                                ui.data_mut(|d| d.remove::<String>(id));
                            } else {
                                ui.data_mut(|d| d.insert_temp(id, buf));
                            }
                            if specific.is_some() && ui.small_button("\u{E4F6}").on_hover_text("Torna alla formula").clicked() {
                                cx.msgs.check(cx.store.set_price_list_item(list.id, p.id, Decimal::ZERO, None), "");
                            }
                        });
                        row.col(|ui| {
                            if let Some(v) = price {
                                ui.label(RichText::new(eur(round2(v * (Decimal::ONE + rate / Decimal::ONE_HUNDRED)))).color(GRAY));
                            }
                        });
                        row.col(|ui| {
                            let n = items.iter().filter(|i| i.product_id == p.id && i.min_qty > Decimal::ONE).count();
                            if ui.small_button(if n == 0 { "aggiungi".to_string() } else { format!("{} scaglioni", n) }).clicked() {
                                st.breaks_for = Some(p.id);
                                st.new_break = (Decimal::from(10), price.map(round2).unwrap_or_default());
                            }
                        });
                    });
                });

            // finestra scaglioni
            let mut close = false;
            if let Some(pid) = st.breaks_for {
                let pname = products.iter().find(|p| p.id == pid).map(|p| format!("{} {}", p.code, p.description)).unwrap_or_default();
                egui::Window::new(format!("Scaglioni di quantità - {}", pname)).collapsible(false).show(ui.ctx(), |ui| {
                    ui.label(format!("Prezzo {} per quantità uguali o superiori alla soglia.", if list.vat_included { "IVA inclusa" } else { "netto" }));
                    egui::Grid::new("breaks").striped(true).num_columns(3).show(ui, |ui| {
                        ui.strong("Da quantità");
                        ui.strong("Prezzo");
                        ui.label("");
                        ui.end_row();
                        for i in items.iter().filter(|i| i.product_id == pid && i.min_qty > Decimal::ONE) {
                            ui.label(i.min_qty.normalize().to_string());
                            ui.label(eur(i.price));
                            if ui.small_button("\u{E4A6}").clicked() {
                                cx.msgs.check(cx.store.set_price_list_item(list.id, pid, i.min_qty, None), "");
                            }
                            ui.end_row();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Da");
                        widgets::dec_field(ui, "nb_q", &mut st.new_break.0, 60.0, 3);
                        ui.label("prezzo");
                        widgets::dec_field(ui, "nb_p", &mut st.new_break.1, 80.0, 4);
                        if ui.button("Aggiungi").clicked() {
                            if st.new_break.0 <= Decimal::ONE {
                                cx.msgs.warn("La soglia deve essere superiore a 1");
                            } else {
                                cx.msgs.check(cx.store.set_price_list_item(list.id, pid, st.new_break.0, Some(st.new_break.1)), "Scaglione aggiunto");
                            }
                        }
                    });
                    if ui.button("Chiudi").clicked() {
                        close = true;
                    }
                });
            }
            if close {
                st.breaks_for = None;
            }
        });
    });
}
