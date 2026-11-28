//! Operazioni di fine anno: controlli, inventario al 31/12, copia di sicurezza e chiusura.

use chrono::Datelike;
use eframe::egui::{self, RichText};
use openfatt_store::YearCheck;

use super::theme::{self, icon, Tone};
use super::widgets::{self, eur, GRAY, ORANGE, RED};
use super::Ctx;

pub struct State {
    year: i32,
    check: Option<YearCheck>,
    confirm: bool,
    reopen: Option<i32>,
    fresh: (bool, bool, bool),
    reports: Option<Vec<openfatt_store::ScheduledReport>>,
}

impl Default for State {
    fn default() -> Self {
        let t = chrono::Local::now().date_naive();
        // a inizio anno si chiude l'anno precedente
        State { year: if t.month() <= 6 { t.year() - 1 } else { t.year() }, check: None, confirm: false, reopen: None, fresh: (true, true, true), reports: None }
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    theme::title_row(ui, &["Sistema"], "Operazioni di fine anno", None, |_| {});
    ui.label(RichText::new("La numerazione dei documenti riparte da sola con il nuovo anno; giacenze, scadenzario e saldi di cassa proseguono senza riporti. La chiusura fa una copia di sicurezza dell'archivio e blocca i documenti dell'anno, che restano consultabili.").color(GRAY));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("Anno");
        if ui.add(egui::DragValue::new(&mut st.year).range(2000..=2100)).changed() {
            st.check = None;
        }
        if theme::button(ui, icon::LIST_CHECKS, "Esegui i controlli").clicked() {
            st.check = cx.msgs.check(cx.store.year_check(st.year), "");
        }
    });
    let closed = cx.store.closed_years().unwrap_or_default();
    if closed.contains(&st.year) {
        ui.horizontal(|ui| {
            theme::pill(ui, &format!("Anno {} chiuso", st.year), Tone::Positive);
            if let Ok(Some(c)) = cx.store.year_closure(st.year) {
                ui.label(RichText::new(format!("il {} · copia: {}", &c.closed_at[..10.min(c.closed_at.len())], c.backup)).color(GRAY));
            }
            if ui.button("Riapri l'anno").clicked() {
                st.reopen = Some(st.year);
            }
        });
    }
    if st.check.is_none() {
        st.check = cx.store.year_check(st.year).ok();
    }
    let Some(c) = st.check.clone() else { return };
    ui.add_space(8.0);
    egui::ScrollArea::vertical().id_salt("ye").show(ui, |ui| {
        theme::card(ui, "Controlli", |_| {}, |ui| {
            let line = |ui: &mut egui::Ui, ok: bool, blocking: bool, text: String, items: &[String]| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(if ok { icon::CHECK_CIRCLE } else { icon::WARNING }).color(if ok { theme::POSITIVE } else if blocking { RED } else { ORANGE }).size(18.0));
                    ui.label(text);
                });
                if !items.is_empty() {
                    egui::CollapsingHeader::new(format!("vedi ({})", items.len())).id_salt(items[0].clone()).show(ui, |ui| {
                        for i in items.iter().take(200) {
                            ui.label(RichText::new(i).small().color(GRAY));
                        }
                    });
                }
            };
            line(ui, c.drafts.is_empty(), true, if c.drafts.is_empty() { "Nessun documento fiscale in bozza".into() } else { if c.drafts.len() == 1 { "1 documento fiscale in bozza: emetterlo o eliminarlo".to_string() } else { format!("{} documenti fiscali in bozza: emetterli o eliminarli", c.drafts.len()) } }, &c.drafts);
            line(ui, c.rejected.is_empty(), true, if c.rejected.is_empty() { "Nessuna fattura elettronica scartata".into() } else { if c.rejected.len() == 1 { "1 fattura elettronica scartata".to_string() } else { format!("{} fatture elettroniche scartate", c.rejected.len()) } }, &c.rejected);
            line(ui, c.not_sent.is_empty(), false, if c.not_sent.is_empty() { "Tutte le fatture sono state trasmesse allo SdI".into() } else { if c.not_sent.len() == 1 { "1 fattura non ancora trasmessa allo SdI".to_string() } else { format!("{} fatture non ancora trasmesse allo SdI", c.not_sent.len()) } }, &c.not_sent);
            line(ui, c.retail_pending == 0, false, if c.retail_pending == 0 { "Tutte le vendite al banco hanno il documento commerciale".into() } else { format!("{} vendite al banco senza documento commerciale", c.retail_pending) }, &[]);
            line(ui, c.open_inventories == 0, true, if c.open_inventories == 0 { "Nessun inventario aperto".into() } else { if c.open_inventories == 1 { "1 inventario fisico aperto: chiuderlo o applicarlo".to_string() } else { format!("{} inventari fisici aperti: chiuderli o applicarli", c.open_inventories) } }, &[]);
            line(ui, c.negative_stock.is_empty(), false, if c.negative_stock.is_empty() { "Nessuna giacenza negativa al 31/12".into() } else { format!("{} articoli con giacenza negativa al 31/12", c.negative_stock.len()) }, &c.negative_stock);
        });
        ui.add_space(8.0);
        theme::card(ui, &format!("Situazione al 31/12/{}", st.year), |_| {}, |ui| {
            egui::Grid::new("ye_sum").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                ui.label("Valore del magazzino al costo");
                ui.label(theme::medium(format!("€ {} ({} articoli)", eur(c.stock_value), c.stock_items)));
                ui.end_row();
                ui.label("Crediti verso clienti aperti");
                ui.label(format!("€ {}", eur(c.open_receivables)));
                ui.end_row();
                ui.label("Debiti verso fornitori aperti");
                ui.label(format!("€ {}", eur(c.open_payables)));
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if theme::button(ui, icon::FILE_PDF, "Inventario al 31/12 (PDF)").on_hover_text("Elenco valorizzato per il libro degli inventari").clicked() {
                    match crate::reports::inventory_valuation_pdf(cx.store, st.year).and_then(|pdf| crate::services::open_bytes(&format!("inventario_{}.pdf", st.year), &pdf)) {
                        Ok(_) => {}
                        Err(e) => cx.msgs.err(e),
                    }
                }
                if theme::button(ui, icon::FILE_XLS, "Excel").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Excel", &["xlsx"]).set_file_name(format!("inventario_{}.xlsx", st.year)).save_file() {
                        cx.msgs.check(crate::reports::inventory_valuation_xlsx(cx.store, st.year, &p), "Esportazione completata");
                    }
                }
            });
        });
        ui.add_space(8.0);
        let block = c.blocking();
        if !closed.contains(&st.year) {
            ui.horizontal(|ui| {
                if theme::primary_enabled(ui, block.is_empty(), icon::LOCK_SIMPLE, &format!("Chiudi l'anno {}", st.year)).clicked() {
                    st.confirm = true;
                }
                if !block.is_empty() {
                    ui.label(RichText::new(format!("Prima: {}", block.join("; "))).color(RED));
                }
            });
        }
    });
    if st.confirm {
        match widgets::confirm(ui.ctx(), "Chiusura dell'anno", &format!("Fare la copia di sicurezza e bloccare i documenti del {}? L'anno si potrà riaprire.", st.year)) {
            Some(true) => {
                st.confirm = false;
                let name = format!("{}_chiusura_{}.gadb", cx.store.path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "archivio".into()), st.year);
                let dir = crate::config::backup_dir();
                if let Some(c) = cx.msgs.check(cx.store.close_year(st.year, &dir.join(name)), "") {
                    cx.msgs.ok(format!("Anno {} chiuso; copia di sicurezza in {}", c.year, c.backup));
                }
            }
            Some(false) => st.confirm = false,
            None => {}
        }
    }
    ui.add_space(10.0);
    fresh_and_reports(ui, cx, st);
    if let Some(y) = st.reopen {
        match widgets::confirm(ui.ctx(), "Riapri l'anno", &format!("Riaprire il {}? I documenti tornano modificabili.", y)) {
            Some(true) => {
                cx.msgs.check(cx.store.reopen_year(y), "Anno riaperto");
                st.reopen = None;
            }
            Some(false) => st.reopen = None,
            None => {}
        }
    }
}

/// Archivio pulito per ripartire e report programmati.
fn fresh_and_reports(ui: &mut egui::Ui, cx: &mut Ctx, st: &mut State) {
    ui.columns(2, |cols| {
        theme::card(&mut cols[0], "Archivio pulito", |_| {}, |ui| {
            ui.label(RichText::new("Un nuovo archivio con clienti, fornitori, articoli, listini, impostazioni e utenti, ma senza documenti né movimenti: per ricominciare da zero o separare gli anni. Questo archivio resta com'è.").color(GRAY));
            ui.checkbox(&mut st.fresh.0, "Giacenze di magazzino come carico iniziale");
            ui.checkbox(&mut st.fresh.1, "Saldi di cassa e banche come saldo iniziale");
            ui.checkbox(&mut st.fresh.2, "Tieni gli utenti e le password");
            if theme::button(ui, icon::FILE_PLUS, "Crea archivio pulito...").clicked() {
                let start = chrono::NaiveDate::from_ymd_opt(st.year + 1, 1, 1).unwrap_or_else(|| chrono::Local::now().date_naive());
                if let Some(p) = rfd::FileDialog::new().add_filter("Archivio openFatt", &["gadb"]).set_directory(crate::config::documents_dir()).set_file_name(format!("Archivio {}.gadb", st.year + 1)).save_file() {
                    let o = openfatt_store::FreshOptions { start, opening_stock: st.fresh.0, opening_balances: st.fresh.1, keep_users: st.fresh.2 };
                    match cx.store.create_fresh_archive(&p, &o) {
                        Ok(r) => {
                            cx.msgs.info_dialog(
                                "Archivio pulito creato",
                                format!(
                                    "{}\n\n{} clienti e fornitori, {} articoli; {} giacenze iniziali e {} saldi iniziali al {}.{}\n\nSi apre da Archivio > Apri.",
                                    p.display(),
                                    r.parties,
                                    r.products,
                                    r.stock_rows,
                                    r.balances,
                                    start.format("%d/%m/%Y"),
                                    if r.open_dues > 0 { format!("\n{} scadenze ancora aperte restano in questo archivio: incassarle qui.", r.open_dues) } else { String::new() }
                                ),
                            );
                        }
                        Err(e) => cx.msgs.err(e.to_string()),
                    }
                }
            }
        });
        theme::card(&mut cols[1], "Report programmati", |_| {}, |ui| {
            ui.label(RichText::new("All'apertura dell'archivio si preparano da soli nella cartella scelta, sul periodo appena chiuso (settimana o mese).").color(GRAY));
            let list = st.reports.get_or_insert_with(|| cx.store.scheduled_reports().unwrap_or_default());
            let mut save = false;
            let mut remove = None;
            for (i, r) in list.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    save |= ui.checkbox(&mut r.enabled, "").changed();
                    egui::ComboBox::from_id_salt(("rep_kind", i)).width(250.0).selected_text(r.kind.label()).show_ui(ui, |ui| {
                        for k in openfatt_store::ReportKind::ALL {
                            save |= ui.selectable_value(&mut r.kind, k, k.label()).changed();
                        }
                    });
                    egui::ComboBox::from_id_salt(("rep_every", i)).width(90.0).selected_text(if r.every == openfatt_store::Every::Weekly { "ogni settimana" } else { "ogni mese" }).show_ui(ui, |ui| {
                        save |= ui.selectable_value(&mut r.every, openfatt_store::Every::Weekly, "ogni settimana").changed();
                        save |= ui.selectable_value(&mut r.every, openfatt_store::Every::Monthly, "ogni mese").changed();
                    });
                    if ui.button(if r.folder.is_empty() { "cartella...".to_string() } else { widgets::truncate(&r.folder, 28) }).on_hover_text(&r.folder).clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            r.folder = d.display().to_string();
                            save = true;
                        }
                    }
                    if ui.small_button(icon::TRASH).clicked() {
                        remove = Some(i);
                    }
                });
                if let Some(d) = r.last_period_end {
                    ui.label(RichText::new(format!("    ultimo periodo preparato: fino al {}", d.format("%d/%m/%Y"))).small().color(GRAY));
                }
            }
            if let Some(i) = remove {
                list.remove(i);
                save = true;
            }
            ui.horizontal(|ui| {
                if ui.small_button(format!("{}  Report", icon::PLUS)).clicked() {
                    list.push(openfatt_store::ScheduledReport { enabled: true, ..Default::default() });
                    save = true;
                }
                if ui.small_button("Prepara ora quelli in scadenza").clicked() {
                    let (files, errs) = crate::reports::run_scheduled(cx.store, chrono::Local::now().date_naive());
                    cx.msgs.ok(format!("{} file preparati", files.len()));
                    for e in errs {
                        cx.msgs.warn(e);
                    }
                    st_reload(list, cx);
                }
            });
            if save {
                cx.msgs.check(cx.store.set_scheduled_reports(list), "");
            }
        });
    });
    let _ = (eur, ORANGE, RED, Tone::Neutral);
}

fn st_reload(list: &mut Vec<openfatt_store::ScheduledReport>, cx: &mut Ctx) {
    *list = cx.store.scheduled_reports().unwrap_or_default();
}
