//! Impostazioni: azienda, PEC, aliquote, pagamenti, fattura elettronica.

use eframe::egui::{self, RichText};
use openfatt_core::codes;
use openfatt_core::fiscal;
use openfatt_core::model::{Company, Installment, PaymentMethod, VatCode};
use openfatt_pec::{PecConfig, TlsMode};
use rust_decimal::Decimal;

use super::widgets::{self, GRAY, RED};
use super::{Ctx, Nav};
use crate::config;
use crate::services::{self, FeOptions};

#[derive(PartialEq, Clone, Copy, Default)]
enum Tab {
    #[default]
    Company,
    Pec,
    Mail,
    Bank,
    Print,
    Vat,
    Payments,
    Fe,
    Sign,
    Rt,
    Users,
}

#[derive(Default)]
pub struct State {
    tab: Tab,
    users: super::users::State,
    company: Option<Company>,
    pec: Option<PecConfig>,
    pec_password: String,
    fe: Option<FeOptions>,
    fe_counter: Option<String>,
    vat_edit: Option<VatCode>,
    pay_edit: Option<PaymentMethod>,
    sign: Option<services::SignConfig>,
    certs: Option<Result<Vec<openfatt_sign::CertInfo>, String>>,
    pfx_pw: String,
    show_all_certs: bool,
    rt: Option<openfatt_pos::RtConfig>,
    mail: super::mail::SettingsState,
    bank: Option<openfatt_store::BankSettings>,
    print: super::print::SettingsState,
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    super::title(ui, "Impostazioni");
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut st.tab, Tab::Company, "Azienda");
        ui.selectable_value(&mut st.tab, Tab::Pec, "PEC e Sistema di Interscambio");
        ui.selectable_value(&mut st.tab, Tab::Mail, "Email e solleciti");
        ui.selectable_value(&mut st.tab, Tab::Bank, "Banca e distinte");
        ui.selectable_value(&mut st.tab, Tab::Print, "Stampe");
        ui.selectable_value(&mut st.tab, Tab::Vat, "Aliquote IVA");
        ui.selectable_value(&mut st.tab, Tab::Payments, "Pagamenti");
        ui.selectable_value(&mut st.tab, Tab::Fe, "Fattura elettronica");
        ui.selectable_value(&mut st.tab, Tab::Sign, "Firma digitale");
        ui.selectable_value(&mut st.tab, Tab::Rt, "Registratore di cassa");
        ui.selectable_value(&mut st.tab, Tab::Users, "Dati, utenti e postazioni");
    });
    ui.separator();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match st.tab {
        Tab::Company => company(ui, cx, st),
        Tab::Pec => pec(ui, cx, st),
        Tab::Mail => super::mail::settings_tab(ui, cx, &mut st.mail),
        Tab::Bank => super::bank::settings_tab(ui, cx, &mut st.bank),
        Tab::Print => super::print::settings_tab(ui, cx, &mut st.print),
        Tab::Vat => vat(ui, cx, st),
        Tab::Payments => payments(ui, cx, st),
        Tab::Fe => fe(ui, cx, st),
        Tab::Sign => sign(ui, cx, st),
        Tab::Users => super::users::settings_tab(ui, cx, &mut st.users),
        Tab::Rt => {
            let cfg = st.rt.get_or_insert_with(|| super::counter::rt_config(cx.store));
            super::counter::settings_tab(ui, cx, cfg);
        }
    });
}

fn company(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let c = st.company.get_or_insert_with(|| cx.store.company().unwrap_or_default());
    egui::Grid::new("company").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Ragione sociale");
        ui.add(egui::TextEdit::singleline(&mut c.name).desired_width(320.0).hint_text("vuoto per ditta individuale"));
        ui.label("Titolo");
        ui.add(egui::TextEdit::singleline(&mut c.title).desired_width(80.0).hint_text("Dott., Ing."));
        ui.end_row();
        ui.label("Nome");
        ui.add(egui::TextEdit::singleline(&mut c.first_name).desired_width(200.0));
        ui.label("Cognome");
        ui.add(egui::TextEdit::singleline(&mut c.last_name).desired_width(200.0));
        ui.end_row();
        ui.label("Partita IVA");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut c.vat_country).desired_width(28.0));
            ui.add(egui::TextEdit::singleline(&mut c.vat_number).desired_width(140.0));
        });
        ui.label("Codice fiscale");
        ui.add(egui::TextEdit::singleline(&mut c.tax_code).desired_width(200.0));
        ui.end_row();
        ui.label("Regime fiscale");
        widgets::code_combo(ui, "rf", &mut c.regime_fiscale, codes::REGIME_FISCALE, 320.0, false);
        ui.label("");
        ui.label("");
        ui.end_row();
        ui.label("Indirizzo sede");
        ui.add(egui::TextEdit::singleline(&mut c.address).desired_width(320.0));
        ui.label("N. civico");
        ui.add(egui::TextEdit::singleline(&mut c.civic).desired_width(70.0));
        ui.end_row();
        ui.label("CAP / Comune");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut c.zip).desired_width(60.0));
            ui.add(egui::TextEdit::singleline(&mut c.city).desired_width(250.0));
        });
        ui.label("Provincia / Nazione");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut c.province).desired_width(36.0).char_limit(2));
            ui.add(egui::TextEdit::singleline(&mut c.country).desired_width(36.0).char_limit(2));
        });
        ui.end_row();
        ui.label("Telefono");
        ui.add(egui::TextEdit::singleline(&mut c.phone).desired_width(160.0));
        ui.label("Email");
        ui.add(egui::TextEdit::singleline(&mut c.email).desired_width(220.0));
        ui.end_row();
        ui.label("PEC aziendale");
        ui.add(egui::TextEdit::singleline(&mut c.pec).desired_width(220.0));
        ui.label("Sito web");
        ui.add(egui::TextEdit::singleline(&mut c.website).desired_width(220.0));
        ui.end_row();
        ui.label("REA ufficio / numero");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut c.rea_office).desired_width(36.0).char_limit(2));
            ui.add(egui::TextEdit::singleline(&mut c.rea_number).desired_width(120.0));
        });
        ui.label("Capitale sociale");
        let mut cap = c.share_capital.unwrap_or_default();
        if widgets::dec_field(ui, "cap", &mut cap, 120.0, 2) {
            c.share_capital = if cap.is_zero() { None } else { Some(cap) };
        }
        ui.end_row();
        ui.label("Soci");
        egui::ComboBox::from_id_salt("socio").selected_text(match c.sole_shareholder.as_str() {
            "SU" => "Socio unico",
            "SM" => "Più soci",
            _ => "-",
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut c.sole_shareholder, String::new(), "-");
            ui.selectable_value(&mut c.sole_shareholder, "SU".to_string(), "Socio unico");
            ui.selectable_value(&mut c.sole_shareholder, "SM".to_string(), "Più soci");
        });
        ui.checkbox(&mut c.in_liquidation, "In liquidazione");
        ui.label("");
        ui.end_row();
        ui.label("Albo professionale");
        ui.add(egui::TextEdit::singleline(&mut c.albo).desired_width(220.0));
        ui.label("Prov. / n. iscrizione");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut c.albo_province).desired_width(36.0).char_limit(2));
            ui.add(egui::TextEdit::singleline(&mut c.albo_number).desired_width(120.0));
        });
        ui.end_row();
        ui.label("Banca");
        ui.add(egui::TextEdit::singleline(&mut c.bank_name).desired_width(220.0));
        ui.label("IBAN / BIC");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut c.iban).desired_width(250.0));
            ui.add(egui::TextEdit::singleline(&mut c.bic).desired_width(100.0));
        });
        ui.end_row();
        ui.label("Trasmittente (se diverso)");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut c.transmitter_country).desired_width(28.0));
            ui.add(egui::TextEdit::singleline(&mut c.transmitter_code).desired_width(170.0).hint_text("CF/P.IVA intermediario"));
        });
        ui.label("Piè di pagina in stampa");
        ui.add(egui::TextEdit::singleline(&mut c.invoice_footer).desired_width(260.0));
        ui.end_row();
    });
    let mut warn = Vec::new();
    if !c.vat_number.is_empty() && c.vat_country == "IT" && !fiscal::partita_iva_valida(&c.vat_number) {
        warn.push("Partita IVA non valida");
    }
    if !c.tax_code.is_empty() && !fiscal::codice_fiscale_valido(&c.tax_code) {
        warn.push("Codice fiscale non valido");
    }
    if !c.iban.is_empty() && !fiscal::iban_valido(&c.iban) {
        warn.push("IBAN non valido");
    }
    if c.display_name().is_empty() {
        warn.push("Indicare ragione sociale oppure nome e cognome");
    }
    for w in warn {
        ui.label(RichText::new(format!("\u{E4E0} {}", w)).color(RED));
    }
    ui.add_space(8.0);
    if ui.button("\u{E248} Salva dati azienda").clicked() {
        c.vat_number = c.vat_number.trim().replace(' ', "");
        c.tax_code = c.tax_code.trim().to_uppercase();
        c.province = c.province.trim().to_uppercase();
        c.rea_office = c.rea_office.trim().to_uppercase();
        c.iban = c.iban.replace(' ', "").to_uppercase();
        cx.msgs.check(cx.store.set_company(c), "Dati azienda salvati");
    }
}

fn pec(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let c = st.pec.get_or_insert_with(|| services::pec_config(cx.store));
    ui.label("Il canale PEC è gratuito e disponibile a chiunque: le fatture vengono inviate dalla casella PEC dell'azienda all'indirizzo del Sistema di Interscambio; ricevute di consegna, scarti e fatture dei fornitori arrivano nella stessa casella.");
    ui.label(RichText::new("Nota: per ricevere le fatture passive via PEC, registrare l'indirizzo PEC nel servizio 'Registrazione indirizzo telematico' del portale Fatture e Corrispettivi, oppure comunicarlo ai fornitori.").small().color(GRAY));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("Gestore:");
        for (name, smtp, sp, imap, ip) in openfatt_pec::provider_presets() {
            if ui.small_button(name).clicked() {
                c.smtp_host = smtp.into();
                c.smtp_port = sp;
                c.imap_host = imap.into();
                c.imap_port = ip;
                c.smtp_tls = TlsMode::Implicit;
            }
        }
    });
    egui::Grid::new("pec").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Indirizzo PEC");
        ui.add(egui::TextEdit::singleline(&mut c.address).desired_width(260.0));
        ui.label("Utente (se diverso)");
        ui.add(egui::TextEdit::singleline(&mut c.username).desired_width(200.0));
        ui.end_row();
        ui.label("Server SMTP / porta");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut c.smtp_host).desired_width(200.0));
            let mut p = Decimal::from(c.smtp_port);
            if widgets::dec_field(ui, "smtp_port", &mut p, 50.0, 0) {
                c.smtp_port = rust_decimal::prelude::ToPrimitive::to_u16(&p).unwrap_or(465);
            }
        });
        ui.label("Sicurezza");
        ui.horizontal(|ui| {
            ui.radio_value(&mut c.smtp_tls, TlsMode::Implicit, "SSL/TLS");
            ui.radio_value(&mut c.smtp_tls, TlsMode::StartTls, "STARTTLS");
        });
        ui.end_row();
        ui.label("Server IMAP / porta");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut c.imap_host).desired_width(200.0));
            let mut p = Decimal::from(c.imap_port);
            if widgets::dec_field(ui, "imap_port", &mut p, 50.0, 0) {
                c.imap_port = rust_decimal::prelude::ToPrimitive::to_u16(&p).unwrap_or(993);
            }
        });
        ui.label("Cartella");
        ui.add(egui::TextEdit::singleline(&mut c.imap_folder).desired_width(120.0));
        ui.end_row();
        ui.label("Indirizzo SdI");
        ui.add(egui::TextEdit::singleline(&mut c.sdi_address).desired_width(260.0));
        ui.label("Giorni da rileggere");
        let mut d = Decimal::from(c.fetch_days);
        if widgets::dec_field(ui, "fetch_days", &mut d, 50.0, 0) {
            c.fetch_days = rust_decimal::prelude::ToPrimitive::to_u32(&d).unwrap_or(30).max(1);
        }
        ui.end_row();
        ui.label("Password");
        ui.add(egui::TextEdit::singleline(&mut st.pec_password).password(true).desired_width(200.0).hint_text(if config::load_password(&c.address).is_some() { "memorizzata" } else { "non memorizzata" }));
        ui.label("");
        ui.label("");
        ui.end_row();
    });
    ui.label(RichText::new("Il primo invio va a sdi01@pec.fatturapa.it; lo SdI risponde da un indirizzo dedicato che il programma memorizza automaticamente.").small().color(GRAY));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("\u{E248} Salva").clicked() {
            if cx.msgs.check(cx.store.set_setting("pec", c), "Impostazioni PEC salvate").is_some() && !st.pec_password.is_empty() {
                match config::store_password(&c.address, &st.pec_password) {
                    Ok(_) => cx.msgs.ok("Password memorizzata nel Gestore credenziali di Windows"),
                    Err(e) => cx.msgs.err(format!("Password non memorizzata: {}", e)),
                }
                st.pec_password.clear();
            }
        }
        if ui.button("\u{EB56} Prova connessione").clicked() {
            let pw = if st.pec_password.is_empty() { config::load_password(&c.address).unwrap_or_default() } else { st.pec_password.clone() };
            if pw.is_empty() {
                cx.msgs.warn("Inserire la password");
            } else {
                cx.nav.push(Nav::PecTest(c.clone(), pw));
            }
        }
        if ui.button("Dimentica password").clicked() {
            config::delete_password(&c.address);
            cx.msgs.ok("Password rimossa");
        }
    });
}

fn vat(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let list = cx.store.vat_codes().unwrap_or_default();
    if let Some(v) = &mut st.vat_edit {
        let mut close = false;
        egui::Window::new("Aliquota IVA").collapsible(false).show(ui.ctx(), |ui| {
            egui::Grid::new("vat_e").num_columns(2).show(ui, |ui| {
                ui.label("Codice");
                ui.text_edit_singleline(&mut v.code);
                ui.end_row();
                ui.label("Descrizione");
                ui.text_edit_singleline(&mut v.description);
                ui.end_row();
                ui.label("Aliquota %");
                widgets::dec_field(ui, "vat_rate", &mut v.rate, 70.0, 2);
                ui.end_row();
                ui.label("Natura (se 0%)");
                let mut n = v.natura.clone().unwrap_or_default();
                if widgets::code_combo(ui, "vat_nat", &mut n, codes::NATURA, 360.0, true) {
                    v.natura = if n.is_empty() { None } else { Some(n) };
                }
                ui.end_row();
                ui.label("Riferimento normativo");
                ui.add(egui::TextEdit::singleline(&mut v.legal_ref).desired_width(360.0));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut v.active, "Attiva");
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut v.split_payment, "Scissione dei pagamenti (fatture alla Pubblica Amministrazione)");
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if ui.button("Salva").clicked() && cx.msgs.check(cx.store.save_vat_code(v), "Aliquota salvata").is_some() {
                    close = true;
                }
                if ui.button("Annulla").clicked() {
                    close = true;
                }
            });
        });
        if close {
            st.vat_edit = None;
        }
    }
    if ui.button("\u{E3D4} Nuova aliquota").clicked() {
        st.vat_edit = Some(VatCode { active: true, ..Default::default() });
    }
    egui::Grid::new("vat_list").striped(true).num_columns(6).show(ui, |ui| {
        for h in ["Codice", "Descrizione", "Aliquota", "Natura", "Riferimento normativo", ""] {
            ui.strong(h);
        }
        ui.end_row();
        for v in list {
            ui.label(&v.code);
            ui.label(&v.description);
            ui.label(format!("{}%{}", openfatt_core::money::fmt_it_trim(v.rate, 0, 2), if v.split_payment { " sp" } else { "" }));
            ui.label(v.natura.clone().unwrap_or_default());
            ui.label(widgets::truncate(&v.legal_ref, 60));
            if ui.small_button("Modifica").clicked() {
                st.vat_edit = Some(v.clone());
            }
            ui.end_row();
        }
    });
}

fn payments(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let list = cx.store.payment_methods().unwrap_or_default();
    if let Some(p) = &mut st.pay_edit {
        let mut close = false;
        egui::Window::new("Modalità di pagamento").collapsible(false).default_width(560.0).show(ui.ctx(), |ui| {
            egui::Grid::new("pay_e").num_columns(2).show(ui, |ui| {
                ui.label("Nome");
                ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(260.0));
                ui.end_row();
                ui.label("Modalità FatturaPA");
                widgets::code_combo(ui, "pay_mp", &mut p.mp_code, codes::MODALITA_PAGAMENTO, 260.0, false);
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut p.show_iban, "Indica l'IBAN aziendale in fattura");
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut p.active, "Attiva");
                ui.end_row();
            });
            ui.label(RichText::new("Rate").strong());
            let mut remove = None;
            for (i, it) in p.installments.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("{}.", i + 1));
                    let mut days = Decimal::from(it.days);
                    if widgets::dec_field(ui, ("days", i), &mut days, 50.0, 0) {
                        it.days = rust_decimal::prelude::ToPrimitive::to_u32(&days).unwrap_or(0);
                    }
                    ui.label("giorni");
                    ui.checkbox(&mut it.end_of_month, "fine mese");
                    ui.label("+");
                    let mut extra = Decimal::from(it.extra_days);
                    if widgets::dec_field(ui, ("extra", i), &mut extra, 40.0, 0) {
                        it.extra_days = rust_decimal::prelude::ToPrimitive::to_u32(&extra).unwrap_or(0);
                    }
                    ui.label("gg  quota %");
                    widgets::dec_field(ui, ("pct", i), &mut it.percent, 60.0, 2);
                    if ui.small_button("\u{E4A6}").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                p.installments.remove(i);
            }
            ui.horizontal(|ui| {
                if ui.button("\u{E3D4} Rata").clicked() {
                    p.installments.push(Installment { days: 30 * (p.installments.len() as u32 + 1), end_of_month: false, extra_days: 0, percent: Decimal::ZERO });
                }
                if ui.button("Ripartisci in parti uguali").clicked() && !p.installments.is_empty() {
                    let n = Decimal::from(p.installments.len());
                    let each = openfatt_core::money::round2(Decimal::ONE_HUNDRED / n);
                    let len = p.installments.len();
                    for (i, it) in p.installments.iter_mut().enumerate() {
                        it.percent = if i + 1 == len { Decimal::ONE_HUNDRED - each * Decimal::from(len - 1) } else { each };
                    }
                }
            });
            ui.label(RichText::new(openfatt_core::calc::describe_installments(p)).color(GRAY));
            ui.horizontal(|ui| {
                if ui.button("Salva").clicked() && cx.msgs.check(cx.store.save_payment_method(p), "Pagamento salvato").is_some() {
                    close = true;
                }
                if ui.button("Annulla").clicked() {
                    close = true;
                }
            });
        });
        if close {
            st.pay_edit = None;
        }
    }
    if ui.button("\u{E3D4} Nuova modalità").clicked() {
        st.pay_edit = Some(PaymentMethod { mp_code: "MP05".into(), installments: vec![Installment::default()], show_iban: true, active: true, ..Default::default() });
    }
    egui::Grid::new("pay_list").striped(true).num_columns(5).show(ui, |ui| {
        for h in ["Nome", "Modalità", "Rate", "Attiva", ""] {
            ui.strong(h);
        }
        ui.end_row();
        for p in list {
            ui.label(&p.name);
            ui.label(format!("{} {}", p.mp_code, codes::describe(codes::MODALITA_PAGAMENTO, &p.mp_code)));
            ui.label(openfatt_core::calc::describe_installments(&p));
            ui.label(if p.active { "sì" } else { "no" });
            ui.horizontal(|ui| {
                if ui.small_button("Modifica").clicked() {
                    st.pay_edit = Some(p.clone());
                }
                if ui.small_button("\u{E4A6}").clicked() {
                    cx.msgs.check(cx.store.delete_payment_method(p.id), "Eliminato");
                }
            });
            ui.end_row();
        }
    });
}

fn fe(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let o = st.fe.get_or_insert_with(|| services::fe_options(cx.store));
    ui.checkbox(&mut o.attach_pdf, "Allega la copia PDF di cortesia alla fattura elettronica");
    ui.horizontal(|ui| {
        ui.label("Tipo codice articolo (CodiceTipo)");
        ui.add(egui::TextEdit::singleline(&mut o.article_code_type).desired_width(120.0).hint_text("INTERNO"));
    });
    if ui.button("\u{E248} Salva").clicked() {
        cx.msgs.check(cx.store.set_setting("fe_options", o), "Opzioni salvate");
    }
    ui.separator();
    let counter = st.fe_counter.get_or_insert_with(|| cx.store.peek_counter("fe_progressivo").unwrap_or(0).to_string());
    ui.label("Ultimo progressivo di invio usato (determina il nome dei file IT<id>_<progressivo>.xml). Modificarlo solo se si proviene da un altro programma, per non riusare nomi già trasmessi.");
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(counter).desired_width(100.0));
        ui.label(RichText::new(format!("prossimo file: ..._{}.xml", openfatt_fatturapa::naming::progressive(counter.parse::<u64>().unwrap_or(0) + 1))).color(GRAY));
        if ui.button("Imposta").clicked() {
            match counter.parse::<u64>() {
                Ok(v) => {
                    cx.msgs.check(cx.store.set_counter("fe_progressivo", v), "Progressivo aggiornato");
                }
                Err(_) => cx.msgs.err("Numero non valido"),
            }
        }
    });
}

fn sign(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    let cfg = st.sign.get_or_insert_with(|| services::sign_config(cx.store));
    super::signature::settings_tab(ui, cx, super::signature::SignTab { cfg, certs: &mut st.certs, pfx_pw: &mut st.pfx_pw, show_all: &mut st.show_all_certs });
}

impl State {
    /// Stato con la scheda "Firma digitale" già aperta.
    pub fn on_sign_tab() -> Self {
        State { tab: Tab::Sign, ..Default::default() }
    }
    pub fn print_state(&mut self) -> &mut super::print::SettingsState {
        &mut self.print
    }

    pub fn on_print_tab() -> Self {
        State { tab: Tab::Print, ..Default::default() }
    }
    pub fn on_bank_tab() -> Self {
        State { tab: Tab::Bank, ..Default::default() }
    }
    pub fn on_mail_tab() -> Self {
        State { tab: Tab::Mail, ..Default::default() }
    }

    pub fn on_users_tab() -> Self {
        State { tab: Tab::Users, ..Default::default() }
    }
}
