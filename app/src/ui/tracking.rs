//! Cella "variante / lotto / numeri di serie" usata nelle righe dei documenti e nella cassa.

use eframe::egui::{self, RichText};
use openfatt_core::model::{DocLine, Product, Tracking};
use openfatt_core::money::fmt_it_trim;

use super::widgets::{GRAY, RED};

/// Disegna i controlli per la riga. `incoming` = la riga carica il magazzino.
/// Restituisce true se la riga è stata modificata.
pub fn cell(ui: &mut egui::Ui, store: &openfatt_store::Store, product: Option<&Product>, line: &mut DocLine, idx: usize, incoming: bool, warehouse: Option<&str>) -> bool {
    let Some(p) = product else {
        ui.label("");
        return false;
    };
    if p.variants.is_empty() && p.tracking == Tracking::None {
        ui.label("");
        return false;
    }
    let mut changed = false;
    ui.horizontal(|ui| {
        if !p.variants.is_empty() {
            let combos = p.variants.combinations();
            let text = if line.variant.is_empty() { RichText::new("variante?").color(RED) } else { RichText::new(&line.variant) };
            egui::ComboBox::from_id_salt(("var", idx)).width(90.0).selected_text(text).show_ui(ui, |ui| {
                for c in &combos {
                    changed |= ui.selectable_value(&mut line.variant, c.clone(), c).changed();
                }
            });
        }
        match p.tracking {
            Tracking::None => {}
            Tracking::Lot => {
                if incoming {
                    changed |= ui.add(egui::TextEdit::singleline(&mut line.lot).desired_width(70.0).hint_text("lotto")).changed();
                    let mut has = line.lot_expiry.is_some();
                    if ui.checkbox(&mut has, "").on_hover_text("scadenza").changed() {
                        line.lot_expiry = if has { Some(chrono::Local::now().date_naive()) } else { None };
                        changed = true;
                    }
                    if let Some(d) = line.lot_expiry.as_mut() {
                        changed |= super::widgets::date_field(ui, &format!("lotexp{}", idx), d);
                    }
                } else {
                    let variant = if p.variants.is_empty() { None } else { Some(line.variant.as_str()) };
                    let lots = store.lots_available(p.id, variant, warehouse).unwrap_or_default();
                    let text = if line.lot.is_empty() { "lotto: automatico".to_string() } else { format!("lotto {}", line.lot) };
                    egui::ComboBox::from_id_salt(("lot", idx)).width(150.0).selected_text(text).show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut line.lot, String::new(), "automatico (scade prima)").changed();
                        for l in &lots {
                            let label = format!(
                                "{}  disp. {}{}",
                                l.lot,
                                fmt_it_trim(l.qty, 0, 3),
                                l.expiry.map(|e| format!("  scad. {}", e.format("%d/%m/%Y"))).unwrap_or_default()
                            );
                            changed |= ui.selectable_value(&mut line.lot, l.lot.clone(), label).changed();
                        }
                        if lots.is_empty() {
                            ui.label(RichText::new("nessun lotto disponibile").color(GRAY));
                        }
                    });
                }
            }
            Tracking::Serial => {
                let mut text = line.serials.join(", ");
                let need = line.qty.abs();
                let ok = rust_decimal::Decimal::from(line.serials.len()) == need;
                let r = ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .desired_width(150.0)
                        .hint_text("numeri di serie, separati da virgola")
                        .text_color(if ok { ui.visuals().text_color() } else { RED }),
                );
                if r.changed() {
                    line.serials = text.split([',', ';']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                    changed = true;
                }
                if !incoming {
                    let avail = store.serials_in_stock(p.id, warehouse).unwrap_or_default();
                    let choices: Vec<(String, String)> = avail
                        .iter()
                        .filter(|(s, v)| !line.serials.contains(s) && (p.variants.is_empty() || line.variant.is_empty() || *v == line.variant))
                        .cloned()
                        .collect();
                    egui::ComboBox::from_id_salt(("ser", idx)).width(24.0).selected_text("+").show_ui(ui, |ui| {
                        for (s, v) in &choices {
                            if ui.selectable_label(false, if v.is_empty() { s.clone() } else { format!("{} ({})", s, v) }).clicked() {
                                line.serials.push(s.clone());
                                changed = true;
                            }
                        }
                        if avail.is_empty() {
                            ui.label(RichText::new("nessun seriale a magazzino").color(GRAY));
                        }
                    });
                }
            }
        }
    });
    changed
}
