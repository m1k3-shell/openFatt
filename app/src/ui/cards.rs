//! Carte fedeltà (raccolta punti) e buoni regalo / gift card.

use eframe::egui::{self, RichText};
use openfatt_core::money::fmt_it_trim;
use openfatt_store::{Card, CardKind, LoyaltySettings};
use rust_decimal::Decimal;

use super::grid::{Col, Grid, Val};
use super::theme::{self, icon, Tone};
use super::widgets::{self, eur, GRAY};
use super::Ctx;

#[derive(Default)]
pub struct State {
    search: String,
    selected: Option<i64>,
    editing: Option<(Card, Decimal)>,
    mv_points: Decimal,
    mv_amount: Decimal,
    mv_note: String,
    settings: Option<LoyaltySettings>,
    grid: Grid,
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let today = chrono::Local::now().date_naive();
    theme::title_row(ui, &["Vendite"], "Carte fedeltà e buoni regalo", None, |ui| {
        if theme::primary(ui, icon::GIFT, "Emetti buono regalo").clicked() {
            st.editing = Some((Card { kind: CardKind::Gift, active: true, issued: Some(today), expiry: Some(openfatt_store::add_months(today, 12)), ..Default::default() }, Decimal::from(50)));
        }
        if theme::button(ui, icon::IDENTIFICATION_CARD, "Nuova carta fedeltà").clicked() {
            st.editing = Some((Card { kind: CardKind::Loyalty, active: true, issued: Some(today), ..Default::default() }, Decimal::ZERO));
        }
    });
    ui.label(RichText::new("I buoni regalo sono buoni multiuso (art. 6-ter DPR 633/72): la vendita del buono non è soggetta a IVA, che si applica quando il buono si spende. Al banco il buono si registra come pagamento con buoni.").small().color(GRAY));
    ui.add_space(6.0);
    edit_window(ui, cx, st);
    let s: &mut LoyaltySettings = st.settings.get_or_insert_with(|| cx.store.get_setting("loyalty").unwrap_or_default());
    egui::CollapsingHeader::new("Regole della raccolta punti").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Punti per ogni euro speso");
            widgets::dec_field(ui, "lp_ppe", &mut s.points_per_euro, 60.0, 2);
            ui.label("Valore di un punto (€)");
            widgets::dec_field(ui, "lp_epp", &mut s.euro_per_point, 60.0, 4);
            ui.label("Punti minimi per lo sconto");
            widgets::dec_field(ui, "lp_min", &mut s.min_redeem, 60.0, 0);
            if ui.button("Salva").clicked() {
                cx.msgs.check(cx.store.set_setting("loyalty", s), "Regole salvate");
            }
        });
    });
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut st.search).hint_text("codice o intestatario").desired_width(260.0));
    });
    let list = cx.store.cards(None, &st.search).unwrap_or_default();
    ui.horizontal_top(|ui| {
        type CardRow = (Card, Decimal, Decimal, String);
        let rows: Vec<CardRow> = list.iter().map(|c| {
            let (p, a) = cx.store.card_balance(c.id).unwrap_or_default();
            let holder = c.party_id.and_then(|id| cx.store.party(id).ok().flatten()).map(|p| p.display_name()).unwrap_or_else(|| c.holder.clone());
            (c.clone(), p, a, holder)
        }).collect();
        let cols: Vec<Col<CardRow>> = vec![
            Col::text("code", "Codice", 110.0, |r: &CardRow| r.0.code.clone()),
            Col::text("kind", "Tipo", 70.0, |r: &CardRow| if r.0.kind == CardKind::Gift { "Buono".to_string() } else { "Fedeltà".to_string() }).group(),
            Col::text("holder", "Intestatario", 170.0, |r: &CardRow| r.3.clone()).opens(|r: &CardRow| r.0.party_id.map(super::quick::Ref::Party)),
            Col::new("balance", "Saldo", 90.0, |r: &CardRow| if r.0.kind == CardKind::Gift { Val::Money(r.2) } else { Val::Num(r.1, 0) }).right(),
            Col::new("expiry", "Scadenza", 90.0, |r: &CardRow| Val::Date(r.0.expiry)),
            Col::new("active", "Attiva", 60.0, |r: &CardRow| Val::Bool(r.0.active)).hidden().group(),
        ];
        st.grid.name = "carte";
        let h = ui.available_height();
        ui.allocate_ui_with_layout(egui::vec2(590.0, h), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.horizontal(|ui| st.grid.toolbar(ui, cx.store, cx.msgs, &cols, &rows, "Carte e buoni"));
            st.grid.version = 0;
            let h = ui.available_height() - 4.0;
            st.grid.show(ui, cx.store, &cols, &rows, |r| r.0.id, h, false, 22.0);
        });
        st.selected = st.grid.single();
        ui.add_space(12.0);
        let Some(c) = st.selected.and_then(|id| list.iter().find(|c| c.id == id)).cloned() else { return };
        theme::card_frame().show(ui, |ui| ui.vertical(|ui| {
            ui.set_width(420.0);
            let (p, a) = cx.store.card_balance(c.id).unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(theme::bold(&c.code).size(18.0));
                theme::pill(ui, c.kind.label(), Tone::Neutral);
                if !c.active {
                    theme::pill(ui, "Non attiva", Tone::Negative);
                }
            });
            let s: LoyaltySettings = cx.store.get_setting("loyalty").unwrap_or_default();
            if c.kind == CardKind::Gift {
                ui.label(theme::bold(format!("Credito € {}", eur(a))).size(22.0));
            } else {
                ui.label(theme::bold(format!("{} punti", fmt_it_trim(p, 0, 0))).size(22.0));
                ui.label(RichText::new(format!("valgono € {} di sconto", eur(p * s.euro_per_point))).color(GRAY));
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if c.kind == CardKind::Gift {
                    ui.label("Importo");
                    widgets::dec_field(ui, "mv_a", &mut st.mv_amount, 80.0, 2);
                } else {
                    ui.label("Punti");
                    widgets::dec_field(ui, "mv_p", &mut st.mv_points, 80.0, 0);
                }
                ui.add(egui::TextEdit::singleline(&mut st.mv_note).desired_width(140.0).hint_text("nota"));
            });
            ui.horizontal(|ui| {
                let (pts, amt) = (st.mv_points, st.mv_amount);
                if ui.button("Accredita").clicked() {
                    cx.msgs.check(cx.store.add_card_move(c.id, today, pts.abs(), amt.abs(), None, &st.mv_note), "Movimento registrato");
                }
                if ui.button("Utilizza").clicked() {
                    cx.msgs.check(cx.store.add_card_move(c.id, today, -pts.abs(), -amt.abs(), None, &st.mv_note), "Movimento registrato");
                }
                if ui.button(format!("{} Modifica", icon::PENCIL_SIMPLE)).clicked() {
                    st.editing = Some((c.clone(), Decimal::ZERO));
                }
            });
            ui.separator();
            for m in cx.store.card_moves(c.id).unwrap_or_default().iter().rev().take(30) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(m.date.format("%d/%m/%Y").to_string()).color(GRAY));
                    if !m.amount.is_zero() {
                        ui.label(format!("€ {}", eur(m.amount)));
                    }
                    if !m.points.is_zero() {
                        ui.label(format!("{} punti", fmt_it_trim(m.points, 0, 0)));
                    }
                    ui.label(RichText::new(&m.note).color(GRAY));
                });
            }
        }));
    });
}

fn edit_window(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let Some((c, initial)) = &mut st.editing else { return };
    let mut close = false;
    let today = chrono::Local::now().date_naive();
    egui::Window::new(c.kind.label()).collapsible(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
        egui::Grid::new("card_form").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Codice");
            ui.add(egui::TextEdit::singleline(&mut c.code).desired_width(180.0).hint_text("automatico o letto col lettore"));
            ui.end_row();
            ui.label("Cliente");
            let current = c.party_id.and_then(|id| cx.store.party(id).ok().flatten()).map(|p| p.display_name()).unwrap_or_default();
            if let Some(p) = widgets::party_picker(ui, cx.store, "card_party", &current, Some(openfatt_core::model::PartyRole::Customer), 240.0) {
                c.party_id = Some(p.id);
            }
            ui.end_row();
            ui.label("Intestatario");
            ui.add(egui::TextEdit::singleline(&mut c.holder).desired_width(240.0).hint_text("se non è un cliente in anagrafica"));
            ui.end_row();
            ui.label("Scadenza");
            widgets::opt_date_field(ui, "card_exp", &mut c.expiry);
            ui.end_row();
            if c.id == 0 && c.kind == CardKind::Gift {
                ui.label("Importo del buono");
                widgets::dec_field(ui, "card_amt", initial, 80.0, 2);
                ui.end_row();
            }
            ui.label("");
            ui.checkbox(&mut c.active, "Attiva");
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if theme::primary(ui, icon::CHECK, "Salva").clicked() {
                let is_new = c.id == 0;
                if let Some(saved) = cx.msgs.check(cx.store.save_card(c), "Carta salvata") {
                    if is_new && saved.kind == CardKind::Gift && !initial.is_zero() {
                        let _ = cx.store.add_card_move(saved.id, today, Decimal::ZERO, *initial, None, "Emissione del buono");
                        // incasso del buono in prima nota: operazione fuori campo IVA (art. 6-ter)
                        let e = openfatt_core::model::CashEntry { date: today, description: format!("Vendita buono multiuso {} (fuori campo IVA art. 6-ter DPR 633/72)", saved.code), amount: *initial, category: "Buoni regalo".into(), party_id: saved.party_id, ..Default::default() };
                        cx.msgs.check(cx.store.add_cash_entry(&e), "");
                    }
                    st.selected = Some(saved.id);
                    close = true;
                }
            }
            if theme::button(ui, icon::X, "Annulla").clicked() {
                close = true;
            }
        });
    });
    if close {
        st.editing = None;
    }
}
