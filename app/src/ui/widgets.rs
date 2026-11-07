//! Componenti riutilizzabili: campi decimali, date, scelta da tabelle di codici,
//! selettori di soggetti, aliquote e pagamenti.

use chrono::NaiveDate;
use eframe::egui::{self, Color32, RichText};
use openfatt_core::codes::CodeTable;
use openfatt_core::model::{Party, PartyRole, PaymentMethod, VatCode, VatRef};
use openfatt_core::money::{fmt_eur, fmt_it_trim, parse_dec_or_zero};
use rust_decimal::Decimal;

pub const RED: Color32 = super::theme::NEGATIVE;
pub const GREEN: Color32 = super::theme::POSITIVE;
pub const ORANGE: Color32 = Color32::from_rgb(0xb4, 0x53, 0x09);
pub const GRAY: Color32 = super::theme::MUTED;

pub fn eur(d: Decimal) -> String {
    fmt_eur(d)
}

/// Campo decimale collegato direttamente a un valore (usa un buffer interno nella memoria di egui).
pub fn dec_field(ui: &mut egui::Ui, id: impl std::hash::Hash, value: &mut Decimal, width: f32, max_dp: u32) -> bool {
    let id = ui.make_persistent_id(id);
    let mut buf: String = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| fmt_it_trim(*value, 0, max_dp));
    // se il valore è cambiato dall'esterno si riallinea il testo
    if parse_dec_or_zero(&buf) != Some(*value) && !ui.memory(|m| m.has_focus(id.with("te"))) {
        buf = fmt_it_trim(*value, 0, max_dp);
    }
    let color = if parse_dec_or_zero(&buf).is_some() { ui.visuals().text_color() } else { RED };
    let r = ui.add(egui::TextEdit::singleline(&mut buf).id(id.with("te")).desired_width(width).horizontal_align(egui::Align::RIGHT).text_color(color));
    let mut changed = false;
    if r.changed() {
        if let Some(v) = parse_dec_or_zero(&buf) {
            *value = v;
            changed = true;
        }
    }
    if r.lost_focus() {
        buf = fmt_it_trim(*value, 0, max_dp);
    }
    ui.data_mut(|d| d.insert_temp(id, buf));
    changed
}

pub fn date_field(ui: &mut egui::Ui, id: &str, d: &mut NaiveDate) -> bool {
    let before = *d;
    ui.add(egui_extras::DatePickerButton::new(d).id_salt(id).format("%d/%m/%Y").show_icon(true));
    before != *d
}

pub fn opt_date_field(ui: &mut egui::Ui, id: &str, d: &mut Option<NaiveDate>) {
    let mut on = d.is_some();
    if ui.checkbox(&mut on, "").changed() {
        *d = if on { Some(chrono::Local::now().date_naive()) } else { None };
    }
    if let Some(v) = d {
        date_field(ui, id, v);
    }
}

/// Selezione da una tabella di codici ufficiali.
pub fn code_combo(ui: &mut egui::Ui, id: &str, value: &mut String, table: CodeTable, width: f32, allow_empty: bool) -> bool {
    let before = value.clone();
    let text = if value.is_empty() { "-".to_string() } else { format!("{} {}", value, openfatt_core::codes::describe(table, value)) };
    egui::ComboBox::from_id_salt(id).width(width).selected_text(truncate(&text, 48)).show_ui(ui, |ui| {
        if allow_empty {
            ui.selectable_value(value, String::new(), "-");
        }
        for (c, d) in table {
            ui.selectable_value(value, c.to_string(), format!("{} {}", c, d));
        }
    });
    before != *value
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// Scelta dell'aliquota IVA fra quelle in archivio.
pub fn vat_combo(ui: &mut egui::Ui, id: impl std::hash::Hash, vat: &mut VatRef, vats: &[VatCode], width: f32) -> bool {
    let before = vat.clone();
    egui::ComboBox::from_id_salt(id).width(width).selected_text(vat.label()).show_ui(ui, |ui| {
        for v in vats.iter().filter(|v| v.active) {
            let sel = vat.vat_id == Some(v.id) || (vat.vat_id.is_none() && vat.rate == v.rate && vat.natura == v.natura);
            if ui.selectable_label(sel, format!("{}  {}", v.code, v.description)).clicked() {
                *vat = VatRef::from_code(v);
            }
        }
    });
    before != *vat
}

pub fn payment_combo(ui: &mut egui::Ui, id: &str, value: &mut Option<i64>, payments: &[PaymentMethod], width: f32) -> bool {
    let before = *value;
    let text = value.and_then(|id| payments.iter().find(|p| p.id == id)).map(|p| p.name.clone()).unwrap_or_else(|| "-".into());
    egui::ComboBox::from_id_salt(id).width(width).selected_text(text).show_ui(ui, |ui| {
        ui.selectable_value(value, None, "-");
        for p in payments.iter().filter(|p| p.active) {
            ui.selectable_value(value, Some(p.id), &p.name);
        }
    });
    before != *value
}

/// Selettore di cliente/fornitore con ricerca. Restituisce il soggetto scelto.
pub fn party_picker(ui: &mut egui::Ui, store: &openfatt_store::Store, id: &str, current: &str, role: Option<PartyRole>, width: f32) -> Option<Party> {
    let search_id = ui.make_persistent_id((id, "search"));
    let mut search: String = ui.data_mut(|d| d.get_temp(search_id)).unwrap_or_default();
    let mut chosen = None;
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .height(420.0)
        .selected_text(if current.is_empty() { RichText::new("scegli...").color(GRAY) } else { RichText::new(truncate(current, 50)) })
        .show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut search).hint_text("cerca nome, P.IVA, città...").desired_width(width - 20.0)).request_focus();
            let list = store.parties(&openfatt_store::PartyFilter { role, search: search.clone(), include_inactive: false }).unwrap_or_default();
            for p in list.iter().take(80) {
                let label = format!("{}  ·  {} {}", p.display_name(), p.city, if p.vat_number.is_empty() { &p.tax_code } else { &p.vat_number });
                if ui.selectable_label(false, label).clicked() {
                    chosen = Some(p.clone());
                }
            }
            if list.is_empty() {
                ui.label(RichText::new("Nessun risultato").color(GRAY));
            }
        });
    ui.data_mut(|d| d.insert_temp(search_id, search));
    chosen
}

pub fn status_color(code: &str) -> Color32 {
    match code {
        "CONSEGNATA" | "ACCETTATA" | "REGISTRATA" | "DECORRENZA_TERMINI" | "MANCATA_CONSEGNA" | "NON_RECAPITABILE" => GREEN,
        "SCARTATA" | "RIFIUTATA" => RED,
        "INVIATA" | "DA_INVIARE" | "RICEVUTA" => ORANGE,
        _ => GRAY,
    }
}

/// Scelta di un magazzino ("" = predefinito se `allow_default`).
pub fn warehouse_combo(ui: &mut egui::Ui, id: &str, value: &mut String, warehouses: &[openfatt_core::model::Warehouse], allow_default: bool, width: f32) -> bool {
    let before = value.clone();
    let default_name = warehouses.iter().find(|w| w.is_default).map(|w| w.name.clone()).unwrap_or_default();
    let text = if value.is_empty() { format!("{} (predefinito)", default_name) } else { value.clone() };
    egui::ComboBox::from_id_salt(id).width(width).selected_text(text).show_ui(ui, |ui| {
        if allow_default {
            ui.selectable_value(value, String::new(), format!("{} (predefinito)", default_name));
        }
        for w in warehouses.iter().filter(|w| w.active) {
            ui.selectable_value(value, w.name.clone(), &w.name);
        }
    });
    before != *value
}

/// Finestra di conferma: restituisce Some(true) se confermato, Some(false) se annullato.
pub fn confirm(ctx: &egui::Context, title: &str, text: &str) -> Option<bool> {
    let mut out = None;
    egui::Window::new(title).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.set_max_width(440.0);
        ui.label(text);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if super::theme::primary(ui, super::theme::icon::CHECK, "Conferma").clicked() {
                out = Some(true);
            }
            if super::theme::button(ui, "", "Annulla").clicked() {
                out = Some(false);
            }
        });
    });
    out
}
