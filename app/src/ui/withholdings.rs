//! Riepilogo delle ritenute per percipiente e impostazioni Enasarco (Contabilità > IVA,
//! scheda "Ritenute").

use eframe::egui::{self, RichText};
use openfatt_store::{percipient_totals, EnasarcoSettings, EnasarcoYear, PercipientTotal, WithholdingRow};
use rust_decimal::Decimal;

use super::grid::{Col, Grid, Val};
use super::theme::{self, icon};
use super::widgets::{self, eur};
use super::{Ctx, Nav};

pub struct State {
    grid: Grid,
    totals_grid: Grid,
    /// true = compensi pagati (acquisti), false = ritenute subite (vendite)
    purchases: bool,
    enasarco: Option<EnasarcoSettings>,
}

impl Default for State {
    fn default() -> Self {
        State { grid: Grid::new("ritenute"), totals_grid: Grid::new("ritenute-percipienti"), purchases: true, enasarco: None }
    }
}

fn kind_label(k: &str) -> &'static str {
    match k {
        "RT01" => "Ritenuta persone fisiche",
        "RT02" => "Ritenuta persone giuridiche",
        "RT03" => "Contributo INPS",
        "RT04" => "Contributo Enasarco",
        "RT05" => "Contributo ENPAM",
        _ => "Altro contributo",
    }
}

pub fn view(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, year: i32) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut st.purchases, true, "Compensi pagati (sostituto d'imposta)");
        ui.selectable_value(&mut st.purchases, false, "Ritenute subite (fatture emesse)");
    });
    ui.label(RichText::new(if st.purchases {
        format!("Parcelle e provvigioni ricevute, nell'anno del pagamento ({}): sono i dati per le certificazioni dei percipienti e per il commercialista che prepara la CU e il 770. I documenti dell'anno non ancora pagati sono indicati a parte.", year)
    } else {
        format!("Ritenute e contributi trattenuti dai clienti sulle fatture incassate nel {}: da ritrovare nelle certificazioni ricevute.", year)
    }).color(theme::MUTED));
    ui.add_space(6.0);
    let purchases = st.purchases;
    let (rows, version) = super::memo(ui.ctx(), "ritenute", cx.store.generation(), (year, purchases), || cx.store.withholding_rows(year, purchases).unwrap_or_default());
    let totals: Vec<PercipientTotal> = percipient_totals(&rows);
    theme::card(ui, &format!("Per percipiente · {}", year), |_| {}, |ui| {
        if totals.is_empty() {
            ui.label(theme::muted("Nessuna ritenuta nell'anno."));
            return;
        }
        let cols: Vec<Col<&PercipientTotal>> = vec![
            Col::text("name", if purchases { "Percipiente" } else { "Sostituto (cliente)" }, 260.0, |t: &&PercipientTotal| t.party_name.clone()),
            Col::text("cf", "C.F. / P.IVA", 150.0, |t: &&PercipientTotal| t.tax_code.clone()),
            Col::new("docs", "Documenti", 80.0, |t: &&PercipientTotal| Val::Num(Decimal::from(t.documents as i64), 0)).right(),
            Col::money("fee", "Compensi", 110.0, |t: &&PercipientTotal| t.fee).total(),
            Col::money("base", "Imponibile ritenuta", 130.0, |t: &&PercipientTotal| t.tax_base).total(),
            Col::money("tax", "Ritenute", 100.0, |t: &&PercipientTotal| t.tax).total(),
            Col::money("contr", "Contributi", 100.0, |t: &&PercipientTotal| t.contributions).total(),
            Col::text("unpaid", "Da pagare", 90.0, |t: &&PercipientTotal| if t.unpaid == 0 { String::new() } else { format!("{} doc.", t.unpaid) }),
        ];
        let refs: Vec<&PercipientTotal> = totals.iter().collect();
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                st.totals_grid.toolbar(ui, cx.store, cx.msgs, &cols, &refs, &format!("Ritenute per percipiente {}", year));
            });
        });
        st.totals_grid.version = version;
        st.totals_grid.show(ui, cx.store, &cols, &refs, |t| t.party_id.unwrap_or(0), 200.0, false, 26.0);
    });
    ui.add_space(8.0);
    theme::card(ui, "Dettaglio per documento", |_| {}, |ui| {
        if rows.is_empty() {
            ui.label(theme::muted("Nessun documento."));
            return;
        }
        let open_req = std::cell::Cell::new(None);
        let open_ref = &open_req;
        let cols: Vec<Col<&WithholdingRow>> = vec![
            Col::text("doc", "Documento", 200.0, |r: &&WithholdingRow| r.doc_label.clone()).render(move |ui: &mut egui::Ui, r: &&WithholdingRow| {
                if ui.link(&r.doc_label).clicked() {
                    open_ref.set(Some(r.document_id));
                }
            }),
            Col::new("date", "Data", 90.0, |r: &&WithholdingRow| Val::Date(Some(r.date))),
            Col::new("paid", "Pagato il", 90.0, |r: &&WithholdingRow| Val::Date(r.paid_on)),
            Col::text("party", "Soggetto", 220.0, |r: &&WithholdingRow| r.party_name.clone()).group(),
            Col::text("kind", "Tipo", 180.0, |r: &&WithholdingRow| format!("{} {}", r.kind, kind_label(&r.kind))).group(),
            Col::text("reason", "Causale", 60.0, |r: &&WithholdingRow| r.reason.clone()),
            Col::money("fee", "Compenso", 100.0, |r: &&WithholdingRow| r.fee),
            Col::money("rate", "Aliquota %", 80.0, |r: &&WithholdingRow| r.rate),
            Col::money("base", "Imponibile", 100.0, |r: &&WithholdingRow| r.base).total(),
            Col::money("amount", "Importo", 100.0, |r: &&WithholdingRow| r.amount).total(),
        ];
        let refs: Vec<&WithholdingRow> = rows.iter().collect();
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                st.grid.toolbar(ui, cx.store, cx.msgs, &cols, &refs, &format!("Ritenute {}", year));
            });
        });
        st.grid.version = version;
        let h = (ui.available_height() - 20.0).max(220.0);
        st.grid.show(ui, cx.store, &cols, &refs, |r| r.document_id.wrapping_mul(10).wrapping_add(r.kind.len() as i64 + r.kind.bytes().last().unwrap_or(0) as i64), h, false, 26.0);
        if let Some(id) = open_req.get() {
            cx.nav.push(Nav::OpenDocument(id));
        }
    });
    ui.add_space(8.0);
    enasarco_card(ui, cx, st);
}

/// Tabella Enasarco: aliquote e massimali per anno (da aggiornare ogni anno con i valori
/// pubblicati dalla Fondazione), mono o plurimandatario, società di capitali.
fn enasarco_card(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let s = st.enasarco.get_or_insert_with(|| cx.store.enasarco_settings().unwrap_or_default());
    let mut save = false;
    theme::card(ui, "Contributo Enasarco (agenti di commercio)", |_| {}, |ui| {
        ui.label(RichText::new("Nelle fatture al preponente il contributo a carico dell'agente si trattiene come ritenuta RT04, fino al massimale provvigionale dell'anno per ciascun preponente. Aggiornare ogni anno con i valori pubblicati dalla Fondazione Enasarco.").color(theme::MUTED));
        ui.horizontal(|ui| {
            save |= ui.radio_value(&mut s.multi, false, "Monomandatario").changed();
            save |= ui.radio_value(&mut s.multi, true, "Plurimandatario").changed();
            ui.add_space(12.0);
            save |= ui.checkbox(&mut s.no_cap, "Società di capitali (nessun massimale)").changed();
        });
        let mut remove = None;
        egui::Grid::new("enasarco_years").striped(true).num_columns(6).spacing([10.0, 4.0]).show(ui, |ui| {
            for h in ["Anno", "Aliquota totale %", "A carico dell'agente %", "Massimale mono €", "Massimale pluri €", ""] {
                ui.label(theme::medium(h).size(12.0).color(theme::MUTED));
            }
            ui.end_row();
            for (i, y) in s.years.iter_mut().enumerate() {
                if ui.push_id(("ena_y", i), |ui| ui.add(egui::DragValue::new(&mut y.year).range(2000..=2100).speed(0.1))).inner.changed() {
                    save = true;
                }
                save |= widgets::dec_field(ui, ("ena_r", i), &mut y.rate, 60.0, 2);
                save |= widgets::dec_field(ui, ("ena_a", i), &mut y.agent_rate, 60.0, 2);
                save |= widgets::dec_field(ui, ("ena_m", i), &mut y.cap_single, 100.0, 2);
                save |= widgets::dec_field(ui, ("ena_p", i), &mut y.cap_multi, 100.0, 2);
                if ui.small_button(icon::TRASH).clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            s.years.remove(i);
            save = true;
        }
        ui.horizontal(|ui| {
            if ui.button(format!("{}  Anno", icon::PLUS)).clicked() {
                let last = s.years.iter().max_by_key(|y| y.year).cloned().unwrap_or_default();
                s.years.push(EnasarcoYear { year: if last.year == 0 { chrono::Datelike::year(&chrono::Local::now().date_naive()) } else { last.year + 1 }, ..last });
                save = true;
            }
            ui.label(RichText::new(format!("Valori 2026: {}% di cui {}% a carico dell'agente, massimali € {} e € {}.", eur(Decimal::new(17, 0)), eur(Decimal::new(85, 1)), eur(Decimal::new(45717, 0)), eur(Decimal::new(30478, 0)))).small().color(theme::MUTED));
        });
    });
    if save {
        s.years.sort_by_key(|y| y.year);
        cx.msgs.check(cx.store.set_enasarco_settings(s), "");
    }
}
