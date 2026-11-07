//! Anagrafica clienti e fornitori.

use eframe::egui::{self, RichText};
use openfatt_core::codes;
use openfatt_core::fiscal;
use openfatt_core::model::{DocKind, Party, PartyRole};
use openfatt_store::PartyFilter;

use super::grid::{Col, Grid, Val};
use super::widgets::{self, GRAY, RED};
use super::{Ctx, Nav};

pub struct State {
    new_rule: Option<openfatt_core::pricing::PartyPrice>,
    search: String,
    selected: Option<i64>,
    editing: Option<Party>,
    confirm_delete: Option<i64>,
    show_inactive: bool,
    grid: Grid,
    data: super::Cache<(PartyRole, String, bool), Vec<Party>>,
    bulk: Option<Bulk>,
    attachments: Option<super::linetools::Attachments>,
}

impl Default for State {
    fn default() -> Self {
        State { new_rule: None, search: String::new(), selected: None, editing: None, confirm_delete: None, show_inactive: false, grid: Grid::new("soggetti"), data: Default::default(), bulk: None, attachments: None }
    }
}

/// Modifica di più soggetti: ogni campo si applica solo se spuntato.
#[derive(Default)]
struct Bulk {
    payment: (bool, Option<i64>),
    vat: (bool, Option<i64>),
    price_list: (bool, Option<i64>),
    discount: (bool, String),
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

fn checks(p: &Party) -> Vec<String> {
    let mut v = Vec::new();
    let it = p.country.is_empty() || p.country == "IT";
    if it && !p.vat_number.is_empty() && !fiscal::partita_iva_valida(&p.vat_number) {
        v.push("Partita IVA non valida (controllo del carattere finale)".into());
    }
    if it && !p.tax_code.is_empty() && !fiscal::codice_fiscale_valido(&p.tax_code) {
        v.push("Codice fiscale non valido".into());
    }
    if it && !p.zip.is_empty() && !fiscal::cap_valido(&p.zip) {
        v.push("CAP di 5 cifre".into());
    }
    if !p.sdi_code.is_empty() && !fiscal::codice_destinatario_valido(&p.sdi_code, p.is_pa) {
        v.push(if p.is_pa { "Codice ufficio PA di 6 caratteri".into() } else { "Codice destinatario di 7 caratteri".into() });
    }
    if p.vat_number.is_empty() && p.tax_code.is_empty() && it {
        v.push("Per la fattura elettronica serve partita IVA o codice fiscale".into());
    }
    if !p.iban.is_empty() && !fiscal::iban_valido(&p.iban) {
        v.push("IBAN non valido".into());
    }
    v
}

pub fn edit_form(ui: &mut egui::Ui, p: &mut Party) {
    egui::Grid::new("party_form").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Tipo");
        egui::ComboBox::from_id_salt("role").selected_text(p.role.label()).show_ui(ui, |ui| {
            for r in [PartyRole::Customer, PartyRole::Supplier, PartyRole::Both] {
                ui.selectable_value(&mut p.role, r, r.label());
            }
        });
        ui.label("Codice");
        ui.add(egui::TextEdit::singleline(&mut p.code).hint_text("automatico").desired_width(120.0));
        ui.end_row();

        ui.label("Ragione sociale");
        ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(320.0).hint_text("per le società"));
        ui.checkbox(&mut p.is_pa, "Pubblica Amministrazione");
        ui.checkbox(&mut p.active, "Attivo");
        ui.end_row();

        ui.label("Nome");
        ui.add(egui::TextEdit::singleline(&mut p.first_name).desired_width(200.0).hint_text("persona fisica"));
        ui.label("Cognome");
        ui.add(egui::TextEdit::singleline(&mut p.last_name).desired_width(200.0));
        ui.end_row();

        ui.label("Partita IVA");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut p.vat_country).desired_width(28.0));
            ui.add(egui::TextEdit::singleline(&mut p.vat_number).desired_width(150.0));
        });
        ui.label("Codice fiscale");
        ui.add(egui::TextEdit::singleline(&mut p.tax_code).desired_width(200.0));
        ui.end_row();

        ui.label("Indirizzo");
        ui.add(egui::TextEdit::singleline(&mut p.address).desired_width(320.0));
        ui.label("N. civico");
        ui.add(egui::TextEdit::singleline(&mut p.civic).desired_width(80.0));
        ui.end_row();

        ui.label("CAP / Comune");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut p.zip).desired_width(60.0));
            ui.add(egui::TextEdit::singleline(&mut p.city).desired_width(250.0));
        });
        ui.label("Prov. / Nazione");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut p.province).desired_width(36.0).char_limit(2));
            ui.add(egui::TextEdit::singleline(&mut p.country).desired_width(36.0).char_limit(2));
        });
        ui.end_row();

        ui.label("Codice destinatario");
        ui.add(egui::TextEdit::singleline(&mut p.sdi_code).desired_width(100.0).char_limit(7).hint_text("0000000"));
        ui.label("PEC");
        ui.add(egui::TextEdit::singleline(&mut p.pec).desired_width(220.0));
        ui.end_row();

        ui.label("Email");
        ui.add(egui::TextEdit::singleline(&mut p.email).desired_width(250.0));
        ui.label("Telefono / cell.");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut p.phone).desired_width(110.0));
            ui.add(egui::TextEdit::singleline(&mut p.mobile).desired_width(110.0));
        });
        ui.end_row();

        ui.label("Sconto abituale");
        ui.add(egui::TextEdit::singleline(&mut p.default_discount).desired_width(80.0).hint_text("es. 10+5"));
        ui.label("IBAN");
        ui.add(egui::TextEdit::singleline(&mut p.iban).desired_width(250.0));
        ui.end_row();

        ui.label("Banca d'appoggio");
        ui.add(egui::TextEdit::singleline(&mut p.bank_name).desired_width(250.0).hint_text("es. Banca di Trento, filiale centro"));
        ui.label("ABI / CAB");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut p.bank_abi).desired_width(60.0).char_limit(5)).on_hover_text("Solo se manca l'IBAN: servono per le Ri.Ba.");
            ui.add(egui::TextEdit::singleline(&mut p.bank_cab).desired_width(60.0).char_limit(5));
        });
        ui.end_row();

        ui.label("Codice EORI");
        ui.add(egui::TextEdit::singleline(&mut p.eori).desired_width(160.0));
        ui.label("");
        ui.label("");
        ui.end_row();
    });
    egui::CollapsingHeader::new("Addebito diretto SEPA (SDD)").default_open(!p.sdd_mandate_id.is_empty()).show(ui, |ui| {
        egui::Grid::new("party_sdd").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Riferimento mandato");
            ui.add(egui::TextEdit::singleline(&mut p.sdd_mandate_id).desired_width(200.0).char_limit(35));
            ui.label("Firmato il");
            ui.horizontal(|ui| widgets::opt_date_field(ui, "sdd_date", &mut p.sdd_mandate_date));
            ui.end_row();
            ui.label("Schema");
            ui.horizontal(|ui| {
                let mut b2b = p.sdd_scheme == "B2B";
                ui.radio_value(&mut b2b, false, "Core (consumatori e imprese)");
                ui.radio_value(&mut b2b, true, "B2B (solo imprese)");
                p.sdd_scheme = if b2b { "B2B".into() } else { "CORE".into() };
            });
            ui.label("");
            ui.checkbox(&mut p.sdd_first_done, "primo addebito già presentato").on_hover_text("Il programma lo spunta da solo dopo la prima distinta SDD");
            ui.end_row();
        });
        ui.label(RichText::new("L'addebito usa l'IBAN dell'anagrafica. Il mandato firmato dal cliente va conservato: la banca può chiederlo.").small().color(GRAY));
    });
    ui.label("Note");
    ui.add(egui::TextEdit::multiline(&mut p.notes).desired_rows(3).desired_width(f32::INFINITY));
    let issues = checks(p);
    for i in issues {
        ui.label(RichText::new(format!("\u{E4E0} {}", i)).color(RED));
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State, role: PartyRole) {
    let title = if role == PartyRole::Supplier { "Fornitori" } else { "Clienti" };
    super::title(ui, title);

    // finestra di modifica
    if let Some(p) = &mut st.editing {
        let mut open = true;
        let mut save = false;
        let mut close = false;
        egui::Window::new(if p.id == 0 { "Nuovo soggetto" } else { "Modifica soggetto" })
            .open(&mut open)
            .default_width(820.0)
            .collapsible(false)
            .show(ui.ctx(), |ui| {
                egui::ScrollArea::vertical().max_height((ui.ctx().screen_rect().height() - 180.0).max(300.0)).show(ui, |ui| {
                    payment_and_vat(ui, cx, p);
                    edit_form(ui, p);
                    extra_sections(ui, cx, p, &mut st.attachments);
                    conditions(ui, cx, p, &mut st.new_rule);
                });
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
            if let Some(saved) = cx.msgs.check(cx.store.save_party(p), "Soggetto salvato") {
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
        match widgets::confirm(ui.ctx(), "Elimina", "Eliminare definitivamente il soggetto?") {
            Some(true) => {
                cx.msgs.check(cx.store.delete_party(id), "Soggetto eliminato");
                st.confirm_delete = None;
                st.selected = None;
            }
            Some(false) => st.confirm_delete = None,
            None => {}
        }
    }

    let key = (role, st.search.clone(), st.show_inactive);
    let list_rc = st.data.get(cx.store.generation(), &key, || cx.store.parties(&PartyFilter { role: Some(role), search: key.1.clone(), include_inactive: key.2 }).unwrap_or_default());
    let list: &Vec<Party> = &list_rc;
    ui.horizontal(|ui| {
        if ui.button("\u{E3D4} Nuovo").clicked() {
            st.editing = Some(Party { role, ..Default::default() });
        }
        ui.add(egui::TextEdit::singleline(&mut st.search).hint_text("\u{E30C} cerca nome, codice, P.IVA, città, email").desired_width(320.0));
        ui.checkbox(&mut st.show_inactive, "mostra non attivi");
        if let Some(id) = st.selected {
            if ui.button("\u{E3B4} Modifica").clicked() {
                st.editing = cx.store.party(id).ok().flatten();
            }
            let new_kind = if role == PartyRole::Supplier { DocKind::SupplierOrder } else { DocKind::Invoice };
            if ui.button(format!("\u{E23A} Nuovo/a {}", new_kind.label().to_lowercase())).clicked() {
                if let Ok(Some(p)) = cx.store.party(id) {
                    let mut d = openfatt_core::model::Document::new(new_kind);
                    d.party_id = Some(p.id);
                    d.payment_id = p.default_payment_id;
                    d.party = p;
                    cx.nav.push(Nav::EditDocument(d));
                }
            }
            if ui.button("\u{E4A6} Elimina").clicked() {
                st.confirm_delete = Some(id);
            }
        }
        let addr: Vec<&Party> = if st.grid.selected.is_empty() { list.iter().filter(|p| !p.city.trim().is_empty()).collect() } else { list.iter().filter(|p| st.grid.selected.contains(&p.id)).collect() };
        if ui.button(format!("{} Etichette indirizzi ({})", super::theme::icon::TAG, addr.len())).on_hover_text("Del soggetto selezionato, oppure di tutti quelli dell'elenco con un indirizzo").clicked() {
            cx.nav.push(Nav::Labels(addr.iter().map(|p| crate::printing::address_item(p)).collect()));
        }
        let with_mail: Vec<&Party> = list.iter().filter(|p| !p.email.trim().is_empty() || !p.pec.trim().is_empty()).collect();
        egui::menu::menu_custom_button(ui, super::theme::button_widget(super::theme::icon::ENVELOPE_SIMPLE, "Comunicazione"), |ui| {
            let tpl = crate::mailer::templates(cx.store).communication;
            if !st.grid.selected.is_empty() {
                let sel: Vec<&Party> = list.iter().filter(|p| st.grid.selected.contains(&p.id)).collect();
                if ui.button(format!("Ai {} selezionati...", sel.len())).clicked() {
                    ui.close_menu();
                    cx.nav.push(Nav::Mail(sel.iter().map(|p| crate::mailer::for_communication(cx.store, p, &tpl.subject, &tpl.body)).collect()));
                }
            }
            let n = with_mail.len();
            if ui
                .add_enabled(n > 0, egui::Button::new(format!("A tutti quelli dell'elenco con un indirizzo ({})...", n)))
                .on_hover_text("Un messaggio separato per ciascuno: nessuno vede gli indirizzi degli altri. Per scegliere i destinatari, filtrare prima l'elenco con la ricerca.")
                .clicked()
            {
                ui.close_menu();
                cx.nav.push(Nav::Mail(with_mail.iter().map(|p| crate::mailer::for_communication(cx.store, p, &tpl.subject, &tpl.body)).collect()));
            }
        });
    });
    ui.add_space(4.0);
    // elenco con colonne, ordinamento, gruppi ed esportazione
    let pays = cx.store.payment_methods().unwrap_or_default();
    let lists = cx.store.price_lists().unwrap_or_default();
    let vats = cx.store.vat_codes().unwrap_or_default();
    if let Some(id) = st.selected {
        if !st.grid.selected.contains(&id) {
            st.grid.select_only(id);
        }
    }
    let pays_c = pays.clone();
    let lists_c = lists.clone();
    let cols: Vec<Col<Party>> = vec![
        Col::text("code", "Codice", 80.0, |p: &Party| p.code.clone()),
        Col::text("name", "Denominazione", 260.0, |p: &Party| p.display_name()),
        Col::text("vat", "P.IVA", 120.0, |p: &Party| p.vat_number.clone()),
        Col::text("cf", "Cod. fiscale", 150.0, |p: &Party| p.tax_code.clone()),
        Col::text("address", "Indirizzo", 200.0, |p: &Party| p.full_address()).hidden(),
        Col::text("zip", "CAP", 60.0, |p: &Party| p.zip.clone()).hidden(),
        Col::text("city", "Comune", 150.0, |p: &Party| p.city.clone()).group(),
        Col::text("prov", "Prov.", 55.0, |p: &Party| p.province.clone()).group(),
        Col::text("country", "Nazione", 70.0, |p: &Party| p.country.clone()).hidden().group(),
        Col::text("sdi", "SdI", 90.0, |p: &Party| p.sdi_code.clone()),
        Col::text("email", "Email", 210.0, |p: &Party| p.email.clone()),
        Col::text("pec", "PEC", 210.0, |p: &Party| p.pec.clone()).hidden(),
        Col::text("phone", "Telefono", 120.0, |p: &Party| p.phone.clone()).hidden(),
        Col::text("mobile", "Cellulare", 120.0, |p: &Party| p.mobile.clone()).hidden(),
        Col::text("pay", "Pagamento", 180.0, move |p: &Party| p.default_payment_id.and_then(|id| pays_c.iter().find(|x| x.id == id)).map(|x| x.name.clone()).unwrap_or_default()).hidden().group(),
        Col::text("list", "Listino", 140.0, move |p: &Party| p.price_list_id.and_then(|id| lists_c.iter().find(|x| x.id == id)).map(|x| x.name.clone()).unwrap_or_default()).hidden().group(),
        Col::text("discount", "Sconto", 70.0, |p: &Party| p.default_discount.clone()).hidden(),
        Col::text("iban", "IBAN", 230.0, |p: &Party| p.iban.clone()).hidden(),
        Col::new("pa", "PA", 45.0, |p: &Party| Val::Bool(p.is_pa)).hidden().group(),
        Col::new("active", "Attivo", 60.0, |p: &Party| Val::Bool(p.active)).hidden().group(),
    ];
    let title = if role == PartyRole::Supplier { "Fornitori" } else { "Clienti" };
    ui.horizontal(|ui| {
        let n = st.grid.selected.len();
        ui.label(RichText::new(if n > 1 { format!("{} soggetti, {} selezionati (Ctrl o Maiusc + clic)", list.len(), n) } else { format!("{} soggetti", list.len()) }).color(GRAY));
        if n > 1 && ui.button(format!("{} Modifica i {} selezionati", super::theme::icon::PENCIL_SIMPLE_LINE, n)).clicked() {
            st.bulk = Some(Bulk { active: (false, true), ..Default::default() });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            st.grid.toolbar(ui, cx.store, cx.msgs, &cols, list, title);
        });
    });
    ui.add_space(4.0);
    let avail = ui.available_height();
    st.grid.version = st.data.version;
    let ev = st.grid.show(ui, cx.store, &cols, list, |p| p.id, avail, false, 26.0);
    st.selected = st.grid.single();
    if let Some(id) = ev.opened {
        st.editing = list.iter().find(|p| p.id == id).cloned();
    }

    // modifica multipla
    let mut close = false;
    if let Some(b) = &mut st.bulk {
        let ids: Vec<i64> = st.grid.selected.iter().copied().collect();
        egui::Window::new(format!("Modifica {} soggetti", ids.len())).collapsible(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
            ui.label(RichText::new("Spuntare i dati da cambiare: gli altri restano come sono.").color(GRAY));
            ui.add_space(4.0);
            egui::Grid::new("bulk_party").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.checkbox(&mut b.payment.0, "Pagamento abituale");
                widgets::payment_combo(ui, "bulk_pay", &mut b.payment.1, &pays, 240.0);
                ui.end_row();
                ui.checkbox(&mut b.vat.0, "IVA abituale");
                let text = b.vat.1.and_then(|id| vats.iter().find(|v| v.id == id)).map(|v| format!("{} {}", v.code, v.description)).unwrap_or_else(|| "-".into());
                egui::ComboBox::from_id_salt("bulk_vat").width(240.0).selected_text(text).show_ui(ui, |ui| {
                    ui.selectable_value(&mut b.vat.1, None, "-");
                    for v in &vats {
                        ui.selectable_value(&mut b.vat.1, Some(v.id), format!("{} {}", v.code, v.description));
                    }
                });
                ui.end_row();
                ui.checkbox(&mut b.price_list.0, "Listino");
                let text = b.price_list.1.and_then(|id| lists.iter().find(|l| l.id == id)).map(|l| l.name.clone()).unwrap_or_else(|| "nessuno".into());
                egui::ComboBox::from_id_salt("bulk_list").width(240.0).selected_text(text).show_ui(ui, |ui| {
                    ui.selectable_value(&mut b.price_list.1, None, "nessuno");
                    for l in &lists {
                        ui.selectable_value(&mut b.price_list.1, Some(l.id), &l.name);
                    }
                });
                ui.end_row();
                ui.checkbox(&mut b.discount.0, "Sconto abituale");
                ui.add(egui::TextEdit::singleline(&mut b.discount.1).desired_width(100.0).hint_text("es. 10+5"));
                ui.end_row();
                ui.checkbox(&mut b.active.0, "Stato");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut b.active.1, true, "attivi");
                    ui.radio_value(&mut b.active.1, false, "non attivi");
                });
                ui.end_row();
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let any = b.payment.0 || b.vat.0 || b.price_list.0 || b.discount.0 || b.active.0;
                if super::theme::primary_enabled(ui, any, super::theme::icon::CHECK, &format!("Applica a {} soggetti", ids.len())).clicked() {
                    let (pay, vat, pl, disc, act) = (b.payment.clone(), b.vat.clone(), b.price_list.clone(), b.discount.clone(), b.active);
                    let r = cx.store.update_parties(&ids, |p| {
                        if pay.0 {
                            p.default_payment_id = pay.1;
                        }
                        if vat.0 {
                            p.default_vat_id = vat.1;
                        }
                        if pl.0 {
                            p.price_list_id = pl.1;
                        }
                        if disc.0 {
                            p.default_discount = disc.1.trim().to_string();
                        }
                        if act.0 {
                            p.active = act.1;
                        }
                    });
                    if let Some(n) = cx.msgs.check(r, "") {
                        cx.msgs.ok(format!("{} soggetti aggiornati", n));
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
        st.bulk = None;
    }
}

fn payment_and_vat(ui: &mut egui::Ui, cx: &mut Ctx, p: &mut Party) {
    let pays = cx.store.payment_methods().unwrap_or_default();
    let vats = cx.store.vat_codes().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label("Pagamento abituale");
        widgets::payment_combo(ui, "party_pay", &mut p.default_payment_id, &pays, 220.0);
        ui.label("IVA abituale");
        let text = p.default_vat_id.and_then(|id| vats.iter().find(|v| v.id == id)).map(|v| format!("{} {}", v.code, v.description)).unwrap_or_else(|| "-".into());
        egui::ComboBox::from_id_salt("party_vat").width(240.0).selected_text(text).show_ui(ui, |ui| {
            ui.selectable_value(&mut p.default_vat_id, None, "-");
            for v in &vats {
                ui.selectable_value(&mut p.default_vat_id, Some(v.id), format!("{} {}", v.code, v.description));
            }
        });
    });
    let lists = cx.store.price_lists().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label("Listino prezzi");
        let text = p.price_list_id.and_then(|id| lists.iter().find(|l| l.id == id)).map(|l| l.name.clone()).unwrap_or_else(|| "prezzo base degli articoli".into());
        egui::ComboBox::from_id_salt("party_list").width(240.0).selected_text(text).show_ui(ui, |ui| {
            ui.selectable_value(&mut p.price_list_id, None, "prezzo base degli articoli");
            for l in lists.iter().filter(|l| l.active) {
                ui.selectable_value(&mut p.price_list_id, Some(l.id), &l.name);
            }
        });
    });
    if p.role.is_customer() {
        let agents = cx.store.agents(false).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label("Agente");
            let name = p.agent_id.and_then(|id| agents.iter().find(|a| a.id == id)).map(|a| a.name.clone()).unwrap_or_else(|| "nessuno".into());
            egui::ComboBox::from_id_salt("party_agent").width(200.0).selected_text(name).show_ui(ui, |ui| {
                ui.selectable_value(&mut p.agent_id, None, "nessuno");
                for a in &agents {
                    ui.selectable_value(&mut p.agent_id, Some(a.id), &a.name);
                }
            });
            ui.label("Carta fedeltà");
            ui.add(egui::TextEdit::singleline(&mut p.loyalty_card).desired_width(150.0).hint_text("numero della carta"));
        });
    }
    let _ = codes::NATURA;
    ui.separator();
}

/// Referenti, dichiarazioni d'intento e allegati del soggetto.
fn extra_sections(ui: &mut egui::Ui, cx: &mut Ctx, p: &mut Party, attachments: &mut Option<super::linetools::Attachments>) {
    egui::CollapsingHeader::new(format!("Referenti ({})", p.contacts.len())).default_open(false).show(ui, |ui| {
        let mut remove = None;
        egui::Grid::new("contacts").striped(true).num_columns(6).spacing([8.0, 4.0]).show(ui, |ui| {
            for h in ["Nome", "Ruolo", "Telefono", "Email", "Documenti", ""] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for (i, c) in p.contacts.iter_mut().enumerate() {
                ui.add(egui::TextEdit::singleline(&mut c.name).desired_width(150.0));
                ui.add(egui::TextEdit::singleline(&mut c.role).desired_width(110.0).hint_text("amministrazione"));
                ui.add(egui::TextEdit::singleline(&mut c.phone).desired_width(110.0));
                ui.add(egui::TextEdit::singleline(&mut c.email).desired_width(190.0));
                ui.checkbox(&mut c.gets_documents, "").on_hover_text("Riceve in copia i documenti inviati per email");
                if ui.small_button(super::theme::icon::TRASH).clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            p.contacts.remove(i);
        }
        if ui.button("\u{E3D4} Referente").clicked() {
            p.contacts.push(Default::default());
        }
    });
    if p.role.is_customer() {
        egui::CollapsingHeader::new(format!("Dichiarazioni d'intento ({})", p.intents.len())).default_open(!p.intents.is_empty()).show(ui, |ui| {
            ui.label(RichText::new("Esportatore abituale: le fatture con natura N3.5 riportano protocollo e data della ricevuta (AltriDatiGestionali INTENTO). Il plafond si controlla sulle fatture emesse.").small().color(GRAY));
            let mut remove = None;
            egui::Grid::new("intents").striped(true).num_columns(7).spacing([8.0, 4.0]).show(ui, |ui| {
                for h in ["Protocollo", "Data ricevuta", "Anno", "Importo massimo", "Utilizzato", "Note", ""] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for (i, d) in p.intents.iter_mut().enumerate() {
                    ui.add(egui::TextEdit::singleline(&mut d.protocol).desired_width(190.0).hint_text("17 cifre-6 cifre"));
                    ui.horizontal(|ui| widgets::opt_date_field(ui, &format!("int_d{}", i), &mut d.date));
                    let mut y = d.year;
                    if ui.add(egui::DragValue::new(&mut y).range(2000..=2100)).changed() {
                        d.year = y;
                    }
                    widgets::dec_field(ui, ("int_l", i), &mut d.limit, 100.0, 2);
                    let used = if p.id != 0 && !d.protocol.is_empty() { cx.store.intent_usage(p.id, &d.protocol, None).ok() } else { None };
                    match used {
                        Some(u) => {
                            let over = u.residual().map(|r| r < rust_decimal::Decimal::ZERO).unwrap_or(false);
                            ui.label(RichText::new(format!("€ {} ({} doc.)", widgets::eur(u.used), u.documents)).color(if over { RED } else { GRAY }));
                        }
                        None => {
                            ui.label("-");
                        }
                    }
                    ui.add(egui::TextEdit::singleline(&mut d.note).desired_width(120.0));
                    if ui.small_button(super::theme::icon::TRASH).clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
            if let Some(i) = remove {
                p.intents.remove(i);
            }
            if ui.button("\u{E3D4} Dichiarazione").clicked() {
                p.intents.push(openfatt_core::model::IntentDeclaration { year: chrono::Datelike::year(&chrono::Local::now().date_naive()), ..Default::default() });
            }
        });
    }
    if p.id != 0 {
        let n = cx.store.attachments("party", p.id).map(|a| a.len()).unwrap_or(0);
        if ui.button(format!("{} Allegati ({})...", super::theme::icon::PAPERCLIP, n)).on_hover_text("Contratti, mandati SDD firmati, visure").clicked() {
            *attachments = Some(super::linetools::Attachments::new("party", p.id, p.display_name()));
        }
    }
}

/// Condizioni particolari (prezzi e sconti) del soggetto, per articolo o per categoria.
fn conditions(ui: &mut egui::Ui, cx: &mut Ctx, p: &Party, new_rule: &mut Option<openfatt_core::pricing::PartyPrice>) {
    use openfatt_core::pricing::PartyPrice;
    use rust_decimal::Decimal;
    egui::CollapsingHeader::new("Prezzi e sconti particolari").default_open(false).show(ui, |ui| {
        if p.id == 0 {
            ui.label(RichText::new("Salvare il soggetto per inserire condizioni particolari").color(GRAY));
            return;
        }
        let rules = cx.store.party_prices(p.id).unwrap_or_default();
        egui::Grid::new("rules").striped(true).num_columns(6).show(ui, |ui| {
            for h in ["Articolo / categoria", "Da q.tà", "Prezzo netto", "Sconto %", "Validità", ""] {
                ui.strong(h);
            }
            ui.end_row();
            for r in &rules {
                let what = match r.product_id {
                    Some(id) => cx.store.product(id).ok().flatten().map(|x| format!("{} {}", x.code, x.description)).unwrap_or_default(),
                    None => format!("Categoria {}", r.category),
                };
                ui.label(what);
                ui.label(if r.min_qty > Decimal::ONE { r.min_qty.normalize().to_string() } else { String::new() });
                ui.label(r.price.map(widgets::eur).unwrap_or_default());
                ui.label(&r.discount);
                ui.label(format!(
                    "{}{}",
                    r.valid_from.map(|d| format!("dal {} ", d.format("%d/%m/%Y"))).unwrap_or_default(),
                    r.valid_to.map(|d| format!("al {}", d.format("%d/%m/%Y"))).unwrap_or_default()
                ));
                if ui.small_button("\u{E4A6}").clicked() {
                    cx.msgs.check(cx.store.delete_party_price(r.id), "Condizione eliminata");
                }
                ui.end_row();
            }
        });
        let mut saved = false;
        let r = new_rule.get_or_insert_with(|| PartyPrice { party_id: p.id, ..Default::default() });
        r.party_id = p.id;
        ui.horizontal_wrapped(|ui| {
            let current = r.product_id.and_then(|id| cx.store.product(id).ok().flatten()).map(|x| format!("{} {}", x.code, x.description));
            let search_id = ui.make_persistent_id("rule_prod_search");
            let mut search: String = ui.data_mut(|d| d.get_temp(search_id)).unwrap_or_default();
            egui::ComboBox::from_id_salt("rule_prod").width(220.0).selected_text(current.unwrap_or_else(|| if r.category.is_empty() { "articolo...".into() } else { format!("Categoria {}", r.category) })).show_ui(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut search).hint_text("cerca").desired_width(200.0)).request_focus();
                for c in cx.store.product_categories().unwrap_or_default() {
                    if c.to_lowercase().contains(&search.to_lowercase()) && ui.selectable_label(false, format!("Categoria {}", c)).clicked() {
                        r.category = c.clone();
                        r.product_id = None;
                    }
                }
                for x in cx.store.products(&search, false).unwrap_or_default().into_iter().take(40) {
                    if ui.selectable_label(false, format!("{} {}", x.code, x.description)).clicked() {
                        r.product_id = Some(x.id);
                        r.category.clear();
                    }
                }
            });
            ui.data_mut(|d| d.insert_temp(search_id, search));
            ui.label("da q.tà");
            widgets::dec_field(ui, "rule_q", &mut r.min_qty, 50.0, 3);
            ui.label("prezzo netto");
            let mut price = r.price.unwrap_or_default();
            if widgets::dec_field(ui, "rule_p", &mut price, 80.0, 4) {
                r.price = if price.is_zero() { None } else { Some(price) };
            }
            ui.label("sconto");
            ui.add(egui::TextEdit::singleline(&mut r.discount).desired_width(60.0).hint_text("10+5"));
            ui.label("fino al");
            widgets::opt_date_field(ui, "rule_to", &mut r.valid_to);
            if ui.button("\u{E3D4} Aggiungi").clicked() && cx.msgs.check(cx.store.save_party_price(r), "Condizione aggiunta").is_some() {
                saved = true;
            }
        });
        if saved {
            *new_rule = None;
        }
        ui.label(RichText::new("Le condizioni per articolo prevalgono su quelle per categoria, che prevalgono sul listino.").small().color(GRAY));
    });
}
