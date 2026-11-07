//! Registri IVA, liquidazione periodica e Comunicazione delle liquidazioni (LIPE).

use chrono::Datelike;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use openfatt_core::model::CashEntry;
use openfatt_core::vat::{self, Liquidation, Period, PeriodInputs, Periodicity, Register, RegisterRow};
use openfatt_store::VatSettings;
use rust_decimal::Decimal;

use super::theme::{self, icon, Tone};
use super::widgets::{self, eur};
use super::{Ctx, Nav};
use crate::services;
use crate::vatfiles;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tab {
    #[default]
    Liquidation,
    Sales,
    Purchases,
    Retail,
    Annual,
}

impl Tab {
    pub fn from_name(s: &str) -> Tab {
        match s {
            "vendite" => Tab::Sales,
            "acquisti" => Tab::Purchases,
            "corrispettivi" => Tab::Retail,
            "annuale" => Tab::Annual,
            _ => Tab::Liquidation,
        }
    }
}

pub struct State {
    tab: Tab,
    year: i32,
    index: u32,
    periodicity: Option<Periodicity>,
    inputs: Option<(Period, PeriodInputs)>,
    settings: Option<VatSettings>,
    show_settings: bool,
    problems: Option<Vec<String>>,
}

impl Default for State {
    fn default() -> Self {
        State { tab: Tab::Liquidation, year: 0, index: 0, periodicity: None, inputs: None, settings: None, show_settings: false, problems: None }
    }
}

impl State {
    pub fn on_tab(t: Tab) -> Self {
        State { tab: t, ..Default::default() }
    }
}

fn money(d: Decimal) -> String {
    if d.is_zero() {
        String::new()
    } else {
        eur(d)
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let settings = cx.store.vat_settings().unwrap_or_default();
    if st.periodicity != Some(settings.periodicity) || st.year == 0 {
        // periodo proposto: l'ultimo chiuso
        let today = chrono::Local::now().date_naive();
        let cur = Period::containing(today, settings.periodicity);
        let (y, i) = if cur.index > 1 { (cur.year, cur.index - 1) } else { (cur.year - 1, cur.count()) };
        if st.year == 0 || st.periodicity.is_some() {
            st.year = y;
            st.index = i;
        }
        st.periodicity = Some(settings.periodicity);
    }
    let period = Period::new(st.year, st.index, settings.periodicity);
    let sub = theme::subline(&[
        (if settings.periodicity == Periodicity::Monthly { "Liquidazione mensile" } else { "Liquidazione trimestrale" }, false),
        (" · ", false),
        (&format!("versamento entro il {}, Comunicazione entro il {}", period.payment_due().format("%d/%m/%Y"), period.lipe_due().format("%d/%m/%Y")), false),
    ]);
    theme::title_row(ui, &["Contabilità"], "Registri e liquidazione IVA", Some(sub), |ui| {
        if theme::ghost(ui, true, icon::GEAR, "Impostazioni IVA").clicked() {
            st.show_settings = !st.show_settings;
        }
        egui::ComboBox::from_id_salt("vat_period").width(170.0).selected_text(period.label()).show_ui(ui, |ui| {
            for i in 1..=period.count() {
                let p = Period::new(st.year, i, settings.periodicity);
                ui.selectable_value(&mut st.index, i, p.label());
            }
        });
        egui::ComboBox::from_id_salt("vat_year").width(80.0).selected_text(st.year.to_string()).show_ui(ui, |ui| {
            let now = chrono::Local::now().year();
            for y in (now - 5..=now).rev() {
                ui.selectable_value(&mut st.year, y, y.to_string());
            }
        });
    });
    if st.show_settings {
        settings_card(ui, cx, st);
        ui.add_space(10.0);
    }
    ui.horizontal(|ui| {
        for (t, label) in [(Tab::Liquidation, "Liquidazione"), (Tab::Sales, "Registro vendite"), (Tab::Purchases, "Registro acquisti"), (Tab::Retail, "Registro corrispettivi"), (Tab::Annual, "Dichiarazione annuale")] {
            ui.selectable_value(&mut st.tab, t, label);
        }
    });
    ui.add_space(8.0);
    if let Some(list) = st.problems.clone() {
        let mut close = false;
        egui::Window::new(format!("{}  Dati da completare", icon::WARNING)).collapsible(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
            ui.label("La Comunicazione non è stata creata. Correggere:");
            for p in &list {
                ui.label(format!("•  {}", p));
            }
            ui.add_space(6.0);
            if theme::primary(ui, icon::CHECK, "Ho capito").clicked() {
                close = true;
            }
        });
        if close {
            st.problems = None;
        }
    }
    match st.tab {
        Tab::Liquidation => {
            egui::ScrollArea::vertical().id_salt("vat_liq").auto_shrink([false, false]).show(ui, |ui| liquidation(ui, cx, st, period));
        }
        Tab::Sales => register_view(ui, cx, Register::Sales, period),
        Tab::Purchases => register_view(ui, cx, Register::Purchases, period),
        Tab::Retail => retail_view(ui, cx, period),
        Tab::Annual => {
            egui::ScrollArea::vertical().id_salt("vat_annual").auto_shrink([false, false]).show(ui, |ui| annual_view(ui, cx, st.year));
        }
    }
}

/// Prospetto della dichiarazione IVA annuale per il commercialista.
fn annual_view(ui: &mut egui::Ui, cx: &mut Ctx, year: i32) {
    let key = egui::Id::new(("annual_vat", year, cx.store.generation()));
    let cached: Option<openfatt_store::AnnualVat> = ui.data(|d| d.get_temp(key));
    let res = match cached {
        Some(a) => Ok(a),
        None => cx.store.annual_vat(year).inspect(|a| ui.data_mut(|d| d.insert_temp(key, a.clone()))),
    };
    let a = match res {
        Ok(a) => a,
        Err(e) => {
            ui.label(RichText::new(e.to_string()).color(theme::NEGATIVE));
            return;
        }
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Valori per la dichiarazione IVA {} (anno d'imposta {}), calcolati dai registri e dalle liquidazioni. La dichiarazione la presenta il commercialista: questo prospetto gli dà i righi già compilati.", year + 1, year)).color(theme::MUTED));
    });
    ui.horizontal(|ui| {
        if theme::primary(ui, icon::FILE_PDF, "Prospetto PDF per il commercialista").clicked() {
            match crate::reports::annual_vat_pdf(cx.store, &a).and_then(|pdf| crate::services::open_bytes(&format!("prospetto_IVA_{}.pdf", year), &pdf)) {
                Ok(_) => {}
                Err(e) => cx.msgs.err(e),
            }
        }
        if theme::button(ui, icon::FILE_XLS, "Excel").clicked() {
            if let Some(p) = rfd::FileDialog::new().add_filter("Excel", &["xlsx"]).set_file_name(format!("prospetto_IVA_{}.xlsx", year)).save_file() {
                cx.msgs.check(crate::reports::annual_vat_xlsx(&p, &a), "Prospetto esportato");
            }
        }
    });
    ui.add_space(8.0);
    for q in &a.quadri {
        theme::card(ui, &format!("Quadro {} - {}", q.code, q.title), |_| {}, |ui| {
            egui::Grid::new(("annual", q.code.clone())).striped(true).num_columns(3).spacing([18.0, 4.0]).show(ui, |ui| {
                for r in &q.righi {
                    ui.label(theme::medium(&r.code));
                    ui.label(&r.label);
                    ui.horizontal_wrapped(|ui| {
                        for (t, v) in &r.values {
                            ui.label(RichText::new(format!("{}:", t)).color(theme::MUTED));
                            ui.label(super::widgets::eur(*v));
                            ui.add_space(8.0);
                        }
                    });
                    ui.end_row();
                }
            });
        });
        ui.add_space(8.0);
    }
    if !a.warnings.is_empty() {
        theme::card(ui, "Da verificare con il commercialista", |_| {}, |ui| {
            for w in &a.warnings {
                ui.label(RichText::new(format!("•  {}", w)).color(theme::WARNING));
            }
        });
    }
}

fn settings_card(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let s = st.settings.get_or_insert_with(|| cx.store.vat_settings().unwrap_or_default());
    let mut save = false;
    theme::card(ui, "Impostazioni IVA", |_| {}, |ui| {
        egui::Grid::new("vat_cfg").num_columns(3).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Liquidazione");
            ui.horizontal(|ui| {
                ui.radio_value(&mut s.periodicity, Periodicity::Monthly, "mensile");
                ui.radio_value(&mut s.periodicity, Periodicity::Quarterly, "trimestrale (art. 7 d.P.R. 542/1999)");
            });
            ui.label(theme::muted("i trimestrali pagano l'1% di interessi; il 4° trimestre si chiude con la dichiarazione annuale"));
            ui.end_row();
            ui.label("Dichiarante");
            ui.add(egui::TextEdit::singleline(&mut s.declarant_cf).desired_width(200.0).char_limit(16).hint_text("codice fiscale"));
            ui.label(theme::muted("per le società: chi firma la Comunicazione (di solito il legale rappresentante)"));
            ui.end_row();
            ui.label("Codice carica");
            egui::ComboBox::from_id_salt("vat_role").selected_text(s.declarant_role.to_string()).show_ui(ui, |ui| {
                for (c, d) in [(1u8, "1 - rappresentante legale"), (2, "2 - rappresentante di minore"), (5, "5 - liquidatore"), (6, "6 - rappresentante fiscale"), (9, "9 - società beneficiaria/incorporante")] {
                    ui.selectable_value(&mut s.declarant_role, c, d);
                }
            });
            ui.label("");
            ui.end_row();
            ui.label("Credito dell'anno precedente");
            widgets::dec_field(ui, "vat_prev_credit", &mut s.prev_year_credit, 110.0, 2);
            ui.label(theme::muted("dalla dichiarazione annuale: promemoria; l'utilizzo si indica nel rigo VP9 di ogni liquidazione"));
            ui.end_row();
        });
        ui.add_space(6.0);
        if theme::primary(ui, icon::FLOPPY_DISK, "Salva").clicked() {
            save = true;
        }
    });
    if save {
        s.declarant_cf = s.declarant_cf.trim().to_ascii_uppercase();
        if cx.msgs.check(cx.store.set_vat_settings(s), "Impostazioni IVA salvate").is_some() {
            st.show_settings = false;
            st.settings = None;
        }
    }
}

fn outcome_tone(l: &Liquidation) -> Tone {
    if l.annual {
        Tone::Neutral
    } else if l.inputs.paid_on.is_some() {
        Tone::Positive
    } else if l.pay_now {
        Tone::Warning
    } else {
        Tone::Neutral
    }
}

fn liquidation(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, period: Period) {
    let year = match cx.store.vat_liquidations(period.year, period.count()) {
        Ok(y) => y,
        Err(e) => {
            ui.label(RichText::new(e.to_string()).color(theme::NEGATIVE));
            return;
        }
    };
    let Some((_, l)) = year.iter().find(|(p, _)| *p == period).cloned() else { return };
    if st.inputs.as_ref().map(|x| x.0) != Some(period) {
        st.inputs = Some((period, l.inputs.clone()));
    }
    let mut print = false;
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        theme::card(
            ui,
            &format!("Liquidazione di {}", period.label()),
            |ui| {
                if theme::ghost(ui, true, icon::FILE_PDF, "Stampa").clicked() {
                    print = true;
                }
            },
            |ui| {
                ui.label(theme::muted(format!(
                    "Operazioni attive (VP2) € {}  ·  operazioni passive (VP3) € {}",
                    eur(l.totals.sales_taxable),
                    eur(l.totals.purchases_taxable)
                )));
                ui.add_space(6.0);
                egui::Grid::new("vp").num_columns(4).spacing([14.0, 7.0]).striped(false).show(ui, |ui| {
                    for t in ["Rigo", "", "Debito", "Credito"] {
                        ui.label(theme::medium(t).size(12.0).color(theme::MUTED));
                    }
                    ui.end_row();
                    let (_, inputs) = st.inputs.as_mut().unwrap();
                    let mut changed = false;
                    for (rigo, desc, a, b) in vatfiles::liquidation_lines(&l).into_iter().filter(|x| x.0 != "VP2" && x.0 != "VP3") {
                        let strong = rigo == "VP6" || rigo == "VP14";
                        ui.label(theme::medium(rigo).size(13.0));
                        ui.label(if strong { theme::medium(desc) } else { RichText::new(desc) });
                        let editable = matches!(rigo, "VP9" | "VP10" | "VP11") || (rigo == "VP13" && (period.index == period.count()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(if strong { theme::bold(money(a)) } else { RichText::new(money(a)) });
                        });
                        if editable {
                            let v = match rigo {
                                "VP9" => &mut inputs.prev_year_credit,
                                "VP10" => &mut inputs.car_payments,
                                "VP11" => &mut inputs.tax_credits,
                                _ => &mut inputs.advance,
                            };
                            changed |= widgets::dec_field(ui, ("vp", rigo), v, 100.0, 2);
                        } else {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(if strong { theme::bold(money(b)) } else { RichText::new(money(b)) });
                            });
                        }
                        ui.end_row();
                    }
                    if period.index == period.count() && !inputs.advance.is_zero() {
                        ui.label("");
                        ui.label(theme::muted("Metodo di calcolo dell'acconto"));
                        egui::ComboBox::from_id_salt("adv_m").selected_text(match inputs.advance_method {
                            2 => "previsionale",
                            3 => "analitico",
                            4 => "settori speciali",
                            _ => "storico",
                        }).show_ui(ui, |ui| {
                            for (m, t) in [(1u8, "storico"), (2, "previsionale"), (3, "analitico"), (4, "settori speciali")] {
                                changed |= ui.selectable_value(&mut inputs.advance_method, m, t).changed();
                            }
                        });
                        ui.label("");
                        ui.end_row();
                    }
                    if changed {
                        if inputs.advance_method == 0 {
                            inputs.advance_method = 1;
                        }
                        let _ = cx.store.set_vat_inputs(period, inputs);
                    }
                });
                ui.add_space(10.0);
                let tone = outcome_tone(&l);
                let (bg, fg) = match tone {
                    Tone::Warning => (egui::Color32::from_rgb(0xff, 0xf4, 0xd6), theme::WARNING),
                    Tone::Positive => (egui::Color32::from_rgb(0xdc, 0xfa, 0xe5), theme::POSITIVE),
                    _ => (theme::RAISED, theme::INK),
                };
                egui::Frame::new().fill(bg).corner_radius(8.0).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(theme::medium(l.outcome(period)).color(fg));
                    if let Some(d) = l.inputs.paid_on {
                        ui.label(RichText::new(format!("Versamento registrato il {}", d.format("%d/%m/%Y"))).color(fg));
                    }
                });
                if !l.totals.split_payment.is_zero() || !l.totals.deferred_pending.is_zero() {
                    ui.add_space(6.0);
                    if !l.totals.split_payment.is_zero() {
                        ui.label(theme::muted(format!("IVA in scissione dei pagamenti (la versa la PA): € {}", eur(l.totals.split_payment))));
                    }
                    if !l.totals.deferred_pending.is_zero() {
                        ui.label(theme::muted(format!("IVA ad esigibilità differita sorta nel periodo, esigibile all'incasso: € {}", eur(l.totals.deferred_pending))));
                    }
                }
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    if l.pay_now && l.inputs.paid_on.is_none() && theme::primary(ui, icon::HAND_COINS, "Registra il versamento").on_hover_text("Uscita in prima nota sul conto banca e liquidazione segnata come versata").clicked() {
                        let account = { let b = cx.store.bank_settings().unwrap_or_default(); if b.account.trim().is_empty() { "Banca".to_string() } else { b.account } };
                        let today = chrono::Local::now().date_naive();
                        let e = CashEntry { date: today, description: format!("Versamento IVA {} - F24 codice {}", period.label(), period.f24_code()), account, amount: -l.to_pay, category: "Imposte".into(), ..Default::default() };
                        if cx.msgs.check(cx.store.add_cash_entry(&e), "Versamento registrato in prima nota").is_some() {
                            let mut i = l.inputs.clone();
                            i.paid_on = Some(today);
                            let _ = cx.store.set_vat_inputs(period, &i);
                            st.inputs = None;
                        }
                    }
                    let q = period.quarter();
                    if theme::button(ui, icon::FILE_ARROW_DOWN, &format!("Comunicazione LIPE {}° trimestre", q)).on_hover_text("File XML da inviare dal portale Fatture e Corrispettivi o con Entratel").clicked() {
                        match vatfiles::lipe(cx.store, period.year, q) {
                            Ok((name, xml)) => {
                                if let Some(p) = rfd::FileDialog::new().set_title("Salva la Comunicazione liquidazioni periodiche IVA").add_filter("XML", &["xml"]).set_file_name(&name).save_file() {
                                    match std::fs::write(&p, xml.as_bytes()) {
                                        Ok(_) => cx.msgs.info_dialog(
                                            "Comunicazione creata",
                                            format!(
                                                "File salvato in {}.\n\nSi invia dal portale Fatture e Corrispettivi (Comunicazioni > Liquidazioni periodiche IVA > Invia file), che vi appone il sigillo dell'Agenzia, oppure firmato con Entratel/Fisconline. Scadenza: {}.\n\nPrima dell'invio confrontare gli importi con il prospetto di liquidazione.",
                                                p.display(),
                                                period.lipe_due().format("%d/%m/%Y")
                                            ),
                                        ),
                                        Err(e) => cx.msgs.err(e.to_string()),
                                    }
                                }
                            }
                            Err(list) => st.problems = Some(list),
                        }
                    }
                    if l.inputs.paid_on.is_some() && theme::ghost(ui, true, icon::ARROW_COUNTER_CLOCKWISE, "Segna come non versata").clicked() {
                        let mut i = l.inputs.clone();
                        i.paid_on = None;
                        let _ = cx.store.set_vat_inputs(period, &i);
                        st.inputs = None;
                        cx.msgs.warn("Liquidazione segnata come non versata: il movimento in prima nota va eliminato a mano");
                    }
                });
            },
        );
        let ui = &mut cols[1];
        theme::card(ui, &format!("Anno {}", period.year), |_| {}, |ui| {
            egui::Grid::new("vat_year").num_columns(4).spacing([14.0, 8.0]).show(ui, |ui| {
                for t in ["Periodo", "IVA a debito", "IVA a credito", "Risultato"] {
                    ui.label(theme::medium(t).size(12.0).color(theme::MUTED));
                }
                ui.end_row();
                for (p, l) in &year {
                    let today = chrono::Local::now().date_naive();
                    let future = p.first() > today;
                    let running = !future && p.last() >= today;
                    let label = if *p == period { theme::bold(p.label()) } else { RichText::new(p.label()) };
                    if ui.add(egui::Label::new(label).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        st.index = p.index;
                    }
                    ui.label(money(l.totals.vat_due));
                    ui.label(money(l.totals.vat_deducted));
                    if future {
                        ui.label(theme::muted("-"));
                    } else if running {
                        theme::pill(ui, "in corso", Tone::Neutral);
                    } else if l.annual {
                        theme::pill(ui, &if l.to_pay > Decimal::ZERO { format!("Annuale € {}", eur(l.to_pay)) } else { format!("Credito € {}", eur(l.credit)) }, Tone::Neutral);
                    } else if l.inputs.paid_on.is_some() {
                        theme::pill(ui, &format!("Versata € {}", eur(l.to_pay)), Tone::Positive);
                    } else if l.pay_now {
                        theme::pill(ui, &format!("Da versare € {}", eur(l.to_pay)), Tone::Warning);
                    } else if l.to_pay > Decimal::ZERO {
                        theme::pill(ui, &format!("Riporto € {}", eur(l.to_pay)), Tone::Neutral);
                    } else if l.credit > Decimal::ZERO {
                        theme::pill(ui, &format!("Credito € {}", eur(l.credit)), Tone::Neutral);
                    } else {
                        ui.label(theme::muted("zero"));
                    }
                    ui.end_row();
                }
            });
        });
    });
    if print {
        if let Err(e) = services::open_bytes(&format!("Liquidazione IVA {}.pdf", period.label()), &vatfiles::liquidation_pdf(cx.store, period, &l)) {
            cx.msgs.err(e);
        }
    }
}

fn register_view(ui: &mut egui::Ui, cx: &mut Ctx, register: Register, period: Period) {
    let (from, to) = (period.first(), period.last());
    let rows = cx.store.vat_register(register, from, to).unwrap_or_default();
    let counted: Vec<RegisterRow> = rows.iter().filter(|r| register == Register::Sales || !r.reverse_charge).cloned().collect();
    let name = if register == Register::Sales { "Registro IVA vendite" } else { "Registro IVA acquisti" };
    ui.horizontal(|ui| {
        ui.label(theme::muted(format!("{} · imponibile € {} · imposta € {}", if rows.len() == 1 { "1 documento".to_string() } else { format!("{} documenti", rows.len()) }, eur(counted.iter().map(|r| r.taxable()).sum()), eur(counted.iter().map(|r| r.tax()).sum()))));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::button(ui, icon::FILE_CSV, "Esporta CSV").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name(format!("{} {}.csv", name, period.label())).save_file() {
                    match std::fs::write(&p, vatfiles::register_csv(register, &rows)) {
                        Ok(_) => cx.msgs.ok(format!("Registro esportato in {}", p.display())),
                        Err(e) => cx.msgs.err(e.to_string()),
                    }
                }
            }
            if theme::button(ui, icon::FILE_PDF, "Stampa registro").clicked() {
                if let Err(e) = services::open_bytes(&format!("{} {}.pdf", name, period.label()), &vatfiles::register_pdf(cx.store, register, from, to, &rows)) {
                    cx.msgs.err(e);
                }
            }
        });
    });
    ui.add_space(6.0);
    let avail = ui.available_height();
    ui.columns(1, |cols| {
        let ui = &mut cols[0];
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if rows.is_empty() {
                ui.label(theme::muted("Nessun documento nel periodo."));
                return;
            }
            TableBuilder::new(ui)
                .id_salt(("reg", register == Register::Sales))
                .striped(false)
                .resizable(false)
                .max_scroll_height((avail - 230.0).max(160.0))
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::initial(95.0))
                .column(Column::initial(70.0))
                .column(Column::initial(if register == Register::Sales { 160.0 } else { 170.0 }))
                .column(Column::remainder().at_least(180.0))
                .column(Column::initial(140.0))
                .column(Column::initial(110.0))
                .column(Column::initial(110.0))
                .column(Column::initial(100.0))
                .header(28.0, |mut h| {
                    let heads = if register == Register::Sales { ["Data", "Numero", "Tipo", "Cliente", "P.IVA / C.F.", "Aliquota", "Imponibile", "Imposta"] } else { ["Registr.", "Prot.", "Doc. fornitore", "Fornitore", "P.IVA / C.F.", "Aliquota", "Imponibile", "Imposta"] };
                    for t in heads {
                        h.col(|ui| {
                            ui.label(theme::medium(t).size(13.0).color(theme::MUTED));
                        });
                    }
                })
                .body(|mut body| {
                    for r in &rows {
                        for (i, a) in r.rates.iter().enumerate() {
                            body.row(30.0, |mut row| {
                                let first = i == 0;
                                row.col(|ui| {
                                    if first {
                                        ui.label(r.date.format("%d/%m/%Y").to_string());
                                    }
                                });
                                row.col(|ui| {
                                    if first && ui.link(&r.number).clicked() {
                                        cx.nav.push(Nav::OpenDocument(r.document_id));
                                    }
                                });
                                row.col(|ui| {
                                    if first {
                                        if register == Register::Sales {
                                            ui.label(format!("{} {}", r.kind_label, r.fe_type));
                                        } else {
                                            ui.label(format!("{} del {}", if r.external_number.is_empty() { "-" } else { &r.external_number }, r.doc_date.format("%d/%m/%Y")));
                                        }
                                    }
                                });
                                row.col(|ui| {
                                    if first {
                                        ui.add(egui::Label::new(&r.party).truncate());
                                        if r.reverse_charge {
                                            theme::pill(ui, if register == Register::Sales { "reverse charge" } else { "integrazione" }, Tone::Neutral);
                                        }
                                    }
                                });
                                row.col(|ui| {
                                    if first {
                                        ui.label(theme::muted(&r.party_vat));
                                    }
                                });
                                row.col(|ui| {
                                    ui.label(a.label());
                                    match a.liability.as_str() {
                                        "S" => {
                                            theme::pill(ui, "scissione", Tone::Neutral);
                                        }
                                        "D" => {
                                            theme::pill(ui, "differita", Tone::Neutral);
                                        }
                                        _ => {}
                                    }
                                });
                                row.col(|ui| {
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(eur(a.taxable));
                                    });
                                });
                                row.col(|ui| {
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(eur(a.tax));
                                    });
                                });
                            });
                        }
                    }
                });
        });
        ui.add_space(10.0);
        theme::card(ui, "Riepilogo per aliquota", |_| {}, |ui| {
            egui::Grid::new("reg_sum").num_columns(3).spacing([28.0, 6.0]).show(ui, |ui| {
                for t in ["Aliquota / natura", "Imponibile", "Imposta"] {
                    ui.label(theme::medium(t).size(12.0).color(theme::MUTED));
                }
                ui.end_row();
                for a in vat::summary(&counted) {
                    ui.label(format!("{}{}", a.label(), match a.liability.as_str() {
                        "S" => " · scissione dei pagamenti",
                        "D" => " · esigibilità differita",
                        _ => "",
                    }));
                    ui.label(eur(a.taxable));
                    ui.label(eur(a.tax));
                    ui.end_row();
                }
            });
        });
    });
}

fn retail_view(ui: &mut egui::Ui, cx: &mut Ctx, period: Period) {
    let (from, to) = (period.first(), period.last());
    let rows = cx.store.retail_register(from, to).unwrap_or_default();
    let gross: Decimal = rows.iter().map(|r| r.gross).sum();
    ui.horizontal(|ui| {
        ui.label(theme::muted(format!("{} giorni con vendite · corrispettivi € {}", rows.len(), eur(gross))));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::button(ui, icon::FILE_PDF, "Stampa registro").clicked() {
                if let Err(e) = services::open_bytes(&format!("Registro corrispettivi {}.pdf", period.label()), &vatfiles::retail_pdf(cx.store, from, to, &rows)) {
                    cx.msgs.err(e);
                }
            }
        });
    });
    ui.add_space(6.0);
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        if rows.is_empty() {
            ui.label(theme::muted("Nessuna vendita al banco nel periodo. I corrispettivi sono trasmessi all'Agenzia dal registratore telematico; qui servono per la liquidazione."));
            return;
        }
        egui::Grid::new("retail_reg").num_columns(6).spacing([28.0, 8.0]).striped(true).show(ui, |ui| {
            for t in ["Data", "Vendite", "Aliquota", "Imponibile", "Imposta", "Corrispettivo"] {
                ui.label(theme::medium(t).size(12.0).color(theme::MUTED));
            }
            ui.end_row();
            for d in &rows {
                for (i, (l, taxable, tax)) in d.rates.iter().enumerate() {
                    ui.label(if i == 0 { d.date.format("%d/%m/%Y").to_string() } else { String::new() });
                    ui.label(if i == 0 { d.count.to_string() } else { String::new() });
                    ui.label(l);
                    ui.label(eur(*taxable));
                    ui.label(eur(*tax));
                    ui.label(eur(*taxable + *tax));
                    ui.end_row();
                }
            }
        });
    });
}
