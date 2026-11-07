//! Scheda "Firma digitale" delle impostazioni e azioni di firma sulle fatture.

use eframe::egui::{self, RichText};
use openfatt_sign::SignSource;

use super::widgets::{GRAY, RED};
use super::Ctx;
use crate::services;

/// Stato della scheda (contenuto nello stato delle impostazioni).
pub struct SignTab<'a> {
    pub cfg: &'a mut services::SignConfig,
    pub certs: &'a mut Option<Result<Vec<openfatt_sign::CertInfo>, String>>,
    pub pfx_pw: &'a mut String,
    pub show_all: &'a mut bool,
}

pub fn settings_tab(ui: &mut egui::Ui, cx: &mut Ctx, t: SignTab) {
    let c = t.cfg;
    ui.label("La firma digitale (CAdES-BES, file .p7m con SHA-256) è obbligatoria per le fatture verso la Pubblica Amministrazione ed è facoltativa verso i privati.");
    ui.label(
        RichText::new(
            "Con smart card o token USB del certificatore (Aruba, InfoCert, Namirial...) installare i driver del dispositivo e collegarlo: il certificato compare nell'elenco e il PIN viene chiesto da Windows al momento della firma. In alternativa si può firmare con un programma esterno e caricare il file .p7m.",
        )
        .small()
        .color(GRAY),
    );
    ui.add_space(6.0);
    let mut mode = match c.source {
        SignSource::None => 0,
        SignSource::Store { .. } => 1,
        SignSource::Pfx { .. } => 2,
    };
    ui.horizontal(|ui| {
        ui.radio_value(&mut mode, 0, "Firma con programma esterno");
        ui.radio_value(&mut mode, 1, "Smart card, token o certificato installato");
        ui.radio_value(&mut mode, 2, "File di certificato .pfx / .p12");
    });
    let current = match c.source {
        SignSource::None => 0,
        SignSource::Store { .. } => 1,
        SignSource::Pfx { .. } => 2,
    };
    if mode != current {
        c.source = match mode {
            0 => SignSource::None,
            1 => SignSource::Store { thumbprint: String::new(), label: String::new() },
            _ => SignSource::Pfx { path: String::new() },
        };
    }
    ui.add_space(6.0);
    match &mut c.source {
        SignSource::None => {
            ui.label("Dalla schermata 'Fattura elettronica > Inviate': salvare l'XML, firmarlo con il proprio programma (es. Dike, ArubaSign, GoSign) e ricaricare il file .p7m con 'Carica file firmato'. Il programma verifica che il contenuto firmato coincida con la fattura.");
        }
        SignSource::Store { thumbprint, label } => {
            if t.certs.is_none() {
                *t.certs = Some(openfatt_sign::list_certificates().map_err(|e| e.0));
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(if label.is_empty() { "Nessun certificato scelto".to_string() } else { format!("Scelto: {}", label) }).strong());
                if ui.button("\u{E036} Aggiorna elenco").clicked() {
                    *t.certs = Some(openfatt_sign::list_certificates().map_err(|e| e.0));
                }
            });
            match t.certs.as_ref().unwrap() {
                Err(e) => {
                    ui.label(RichText::new(e).color(RED));
                }
                Ok(all) => {
                    let hidden = all.iter().filter(|c| !c.non_repudiation).count();
                    let list: Vec<&openfatt_sign::CertInfo> = all.iter().filter(|c| *t.show_all || c.non_repudiation || c.thumbprint == *thumbprint).collect();
                    if list.is_empty() {
                        ui.label(RichText::new("Nessun certificato di firma trovato. Collegare la smart card/il token (con i driver del certificatore installati) e premere 'Aggiorna elenco'.").color(RED));
                    }
                    if hidden > 0 {
                        ui.checkbox(t.show_all, format!("mostra anche {} certificati non destinati alla firma", hidden));
                    }
                    if !list.is_empty() {
                        egui::Grid::new("certs").striped(true).num_columns(5).show(ui, |ui| {
                            for h in ["", "Titolare", "Emesso da", "Scadenza", "Uso"] {
                                ui.strong(h);
                            }
                            ui.end_row();
                            for cert in list {
                                let sel = *thumbprint == cert.thumbprint;
                                if ui.radio(sel, "").clicked() {
                                    *thumbprint = cert.thumbprint.clone();
                                    *label = cert.label();
                                }
                                ui.label(&cert.subject);
                                ui.label(&cert.issuer);
                                let color = if cert.expired { RED } else { ui.visuals().text_color() };
                                ui.label(RichText::new(&cert.valid_to).color(color));
                                ui.label(if cert.non_repudiation { "firma (non ripudio)" } else { "autenticazione / altro" });
                                ui.end_row();
                            }
                        });
                    }
                    ui.label(RichText::new("Per la fattura elettronica usare un certificato di firma qualificata (uso 'non ripudio').").small().color(GRAY));
                }
            }
        }
        SignSource::Pfx { path } => {
            ui.horizontal(|ui| {
                ui.label("File");
                ui.add(egui::TextEdit::singleline(path).desired_width(420.0));
                if ui.button("Scegli...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Certificato PKCS#12", &["pfx", "p12"]).pick_file() {
                        *path = p.display().to_string();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Password (solo per le prove, non viene salvata)");
                ui.add(egui::TextEdit::singleline(t.pfx_pw).password(true).desired_width(160.0));
                if ui.button("Verifica certificato").clicked() {
                    let r = std::fs::read(path.as_str()).map_err(|e| e.to_string()).and_then(|b| openfatt_sign::pfx_info(&b, t.pfx_pw).map_err(|e| e.0));
                    match r {
                        Ok(i) => cx.msgs.info_dialog(
                            "Certificato",
                            format!(
                                "{}\nNon ripudio: {}\nValido dal {} al {}{}",
                                i.label(),
                                if i.non_repudiation { "sì" } else { "no" },
                                i.valid_from,
                                i.valid_to,
                                if i.expired { "\nSCADUTO" } else { "" }
                            ),
                        ),
                        Err(e) => cx.msgs.err(e),
                    }
                }
            });
        }
    }
    ui.add_space(6.0);
    let auto_possible = matches!(c.source, SignSource::Store { .. });
    ui.add_enabled_ui(auto_possible, |ui| {
        ui.checkbox(&mut c.auto_pa, "Firma automaticamente le fatture verso la PA appena generate");
        ui.checkbox(&mut c.auto_all, "Firma automaticamente anche le fatture verso i privati");
    });
    if !auto_possible {
        ui.label(RichText::new("La firma automatica è disponibile con smart card/token o certificato installato.").small().color(GRAY));
    }
    ui.horizontal(|ui| {
        if ui.button("\u{E248} Salva").clicked() {
            cx.msgs.check(cx.store.set_setting("signature", &*c), "Impostazioni di firma salvate");
        }
        if !matches!(c.source, SignSource::None) && ui.button("\u{EBAC} Prova di firma").clicked() {
            let sample = format!("Prova di firma openFatt {}", chrono::Local::now().format("%d/%m/%Y %H:%M:%S"));
            match openfatt_sign::sign(sample.as_bytes(), &c.source, t.pfx_pw).and_then(|p| openfatt_sign::verify(&p)) {
                Ok(v) => cx.msgs.info_dialog(
                    "Prova di firma riuscita",
                    format!(
                        "Firmatario: {}\nCAdES-BES (signing-certificate-v2): {}\nAlgoritmo SHA-256: {}\nIl contenuto firmato coincide: {}",
                        v.signer.subject,
                        if v.cades { "sì" } else { "no" },
                        if v.sha256 { "sì" } else { "no" },
                        if v.content == sample.as_bytes() { "sì" } else { "no" }
                    ),
                ),
                Err(e) => cx.msgs.err(e.0),
            }
        }
    });
}

/// Mostra l'esito della verifica di un file firmato.
pub fn verify_dialog(cx: &mut Ctx, data: &[u8]) {
    match openfatt_sign::verify(data) {
        Ok(v) => cx.msgs.info_dialog(
            "Verifica della firma",
            format!(
                "Firma crittograficamente valida: il documento non è stato alterato.\n\nFirmatario: {}\nEmesso da: {}\nValidità del certificato: {} - {}{}\nFormato CAdES-BES: {}\nAlgoritmo SHA-256: {}\n\nNota: non vengono controllate la catena dei certificatori e la revoca (lo SdI le verifica alla ricezione).",
                v.signer.subject,
                v.signer.issuer,
                v.signer.valid_from,
                v.signer.valid_to,
                if v.signer.expired { " (SCADUTO)" } else { "" },
                if v.cades { "sì" } else { "no (firma PKCS#7 semplice)" },
                if v.sha256 { "sì" } else { "no" }
            ),
        ),
        Err(e) => cx.msgs.err(e.0),
    }
}

pub fn sign_now(cx: &mut Ctx, id: i64, password: &str) {
    match services::sign_einvoice(cx.store, id, password) {
        Ok(e) => cx.msgs.ok(format!("Fattura firmata: {}", e.file_name)),
        Err(e) => cx.msgs.err(e),
    }
}
