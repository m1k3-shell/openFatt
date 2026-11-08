//! Costruzione del tracciato FatturaPA a partire da un documento del gestionale.

use chrono::NaiveDate;
use openfatt_core::calc::Totals;
use openfatt_core::codes;
use openfatt_core::model::{Company, DocLine, Document, LineKind, Party};
use rust_decimal::Decimal;

use crate::model::*;
use crate::text::{basic_latin, chunks, latin, opt_basic, opt_latin, truncate};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildError(pub String);

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for BuildError {}

/// DDT d'origine da indicare nei DatiDDT di una fattura differita.
#[derive(Debug, Clone, Default)]
pub struct DdtRef {
    pub number: String,
    pub date: Option<NaiveDate>,
    /// Indici (0-based) delle righe del documento che provengono da questo DDT.
    pub line_indexes: Vec<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// ProgressivoInvio (max 10 caratteri alfanumerici).
    pub progressivo: String,
    pub ddt_refs: Vec<DdtRef>,
    /// PDF di cortesia da allegare (nome file, contenuto).
    pub attachment: Option<(String, Vec<u8>)>,
    /// Codice tipo da usare per CodiceArticolo.
    pub article_code_type: String,
}

/// Ruolo dei soggetti in base al tipo documento.
enum Roles {
    /// L'azienda emette verso il soggetto (caso normale).
    Normal,
    /// Integrazione/autofattura: il fornitore è cedente, l'azienda cessionario.
    SupplierIsCedente,
    /// Autoconsumo/splafonamento: azienda cedente e cessionario.
    SelfBoth,
}

fn roles_for(td: &str) -> Roles {
    match td {
        "TD16" | "TD17" | "TD18" | "TD19" | "TD20" | "TD28" | "TD29" => Roles::SupplierIsCedente,
        "TD21" | "TD27" => Roles::SelfBoth,
        _ => Roles::Normal,
    }
}

fn anagrafica(denominazione: &str, nome: &str, cognome: &str, titolo: &str, eori: &str) -> Anagrafica {
    let den = latin(denominazione);
    if !den.is_empty() {
        Anagrafica {
            denominazione: Some(truncate(&den, 80)),
            nome: None,
            cognome: None,
            titolo: opt_basic(titolo, 10),
            cod_eori: opt_basic(eori, 17),
        }
    } else {
        Anagrafica {
            denominazione: None,
            nome: Some(truncate(&latin(nome), 60)),
            cognome: Some(truncate(&latin(cognome), 60)),
            titolo: opt_basic(titolo, 10),
            cod_eori: opt_basic(eori, 17),
        }
    }
}

fn indirizzo(address: &str, civic: &str, zip: &str, city: &str, province: &str, country: &str) -> Indirizzo {
    let nazione = if country.trim().is_empty() { "IT".to_string() } else { country.trim().to_ascii_uppercase() };
    let italian = nazione == "IT";
    let cap = zip.trim();
    let cap = if cap.len() == 5 && cap.bytes().all(|b| b.is_ascii_digit()) { cap.to_string() } else { "00000".into() };
    let prov = province.trim().to_ascii_uppercase();
    Indirizzo {
        indirizzo: truncate(&latin(address), 60),
        numero_civico: opt_basic(civic, 8),
        cap,
        comune: truncate(&latin(city), 60),
        provincia: if italian && prov.len() == 2 { Some(prov) } else { None },
        nazione,
    }
}

fn clean_code(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase()
}

fn company_as_cedente(c: &Company) -> Result<CedentePrestatore, BuildError> {
    if c.vat_number.trim().is_empty() {
        return Err(BuildError("Partita IVA dell'azienda mancante (Opzioni > Azienda)".into()));
    }
    if c.address.trim().is_empty() || c.city.trim().is_empty() {
        return Err(BuildError("Indirizzo della sede dell'azienda incompleto".into()));
    }
    let rea = if !c.rea_office.trim().is_empty() && !c.rea_number.trim().is_empty() {
        Some(IscrizioneRea {
            ufficio: c.rea_office.trim().to_ascii_uppercase(),
            numero_rea: truncate(&basic_latin(&c.rea_number), 20),
            capitale_sociale: c.share_capital,
            socio_unico: if c.sole_shareholder == "SU" || c.sole_shareholder == "SM" { Some(c.sole_shareholder.clone()) } else { None },
            stato_liquidazione: if c.in_liquidation { "LS".into() } else { "LN".into() },
        })
    } else {
        None
    };
    let contatti = {
        let tel = opt_basic(&c.phone.replace(' ', ""), 12).filter(|t| t.len() >= 5);
        let email = opt_basic(&c.email, 256).filter(|e| e.contains('@'));
        if tel.is_some() || email.is_some() {
            Some(Contatti { telefono: tel, fax: None, email })
        } else {
            None
        }
    };
    Ok(CedentePrestatore {
        id_fiscale_iva: IdFiscale {
            paese: if c.vat_country.trim().is_empty() { "IT".into() } else { c.vat_country.trim().to_ascii_uppercase() },
            codice: clean_code(&c.vat_number),
        },
        codice_fiscale: Some(clean_code(&c.tax_code)).filter(|s| !s.is_empty()),
        anagrafica: anagrafica(&c.name, &c.first_name, &c.last_name, &c.title, ""),
        albo_professionale: opt_basic(&c.albo, 60),
        provincia_albo: Some(c.albo_province.trim().to_ascii_uppercase()).filter(|s| s.len() == 2),
        numero_iscrizione_albo: opt_basic(&c.albo_number, 60),
        data_iscrizione_albo: c.albo_date,
        regime_fiscale: if c.regime_fiscale.is_empty() { "RF01".into() } else { c.regime_fiscale.clone() },
        sede: indirizzo(&c.address, &c.civic, &c.zip, &c.city, &c.province, &c.country),
        stabile_organizzazione: None,
        iscrizione_rea: rea,
        contatti,
        riferimento_amministrazione: None,
    })
}

fn company_as_cessionario(c: &Company) -> CessionarioCommittente {
    CessionarioCommittente {
        id_fiscale_iva: Some(IdFiscale {
            paese: if c.vat_country.trim().is_empty() { "IT".into() } else { c.vat_country.trim().to_ascii_uppercase() },
            codice: clean_code(&c.vat_number),
        }),
        codice_fiscale: Some(clean_code(&c.tax_code)).filter(|s| !s.is_empty()),
        anagrafica: anagrafica(&c.name, &c.first_name, &c.last_name, &c.title, ""),
        sede: indirizzo(&c.address, &c.civic, &c.zip, &c.city, &c.province, &c.country),
        stabile_organizzazione: None,
        rappresentante_fiscale: None,
    }
}

fn party_vat_id(p: &Party) -> Option<IdFiscale> {
    let code = clean_code(&p.vat_number);
    if code.is_empty() {
        return None;
    }
    let paese = if p.vat_country.trim().is_empty() {
        if p.country.trim().is_empty() { "IT".to_string() } else { p.country.trim().to_ascii_uppercase() }
    } else {
        p.vat_country.trim().to_ascii_uppercase()
    };
    // un eventuale prefisso del paese nella partita IVA va rimosso
    let code = code.strip_prefix(&paese).map(|s| s.to_string()).filter(|s| !s.is_empty()).unwrap_or(code);
    Some(IdFiscale { paese, codice: truncate(&code, 28) })
}

fn party_as_cessionario(p: &Party) -> Result<CessionarioCommittente, BuildError> {
    let mut id = party_vat_id(p);
    let foreign = !p.country.trim().is_empty() && p.country.trim().to_ascii_uppercase() != "IT";
    let cf = clean_code(&p.tax_code);
    let mut codice_fiscale = if !foreign && !cf.is_empty() { Some(cf) } else { None };
    if id.is_none() && codice_fiscale.is_none() {
        if foreign {
            // cliente estero senza identificativo: convenzione Agenzia Entrate
            id = Some(IdFiscale { paese: p.country.trim().to_ascii_uppercase(), codice: "99999999999".into() });
        } else {
            return Err(BuildError(format!(
                "Il cliente '{}' non ha né partita IVA né codice fiscale (errore SdI 00417)",
                p.display_name()
            )));
        }
    }
    if p.display_name().is_empty() {
        return Err(BuildError("Denominazione o nome/cognome del cliente mancanti".into()));
    }
    if p.address.trim().is_empty() || p.city.trim().is_empty() {
        return Err(BuildError(format!("Indirizzo del cliente '{}' incompleto", p.display_name())));
    }
    if foreign {
        codice_fiscale = None;
    }
    Ok(CessionarioCommittente {
        id_fiscale_iva: id,
        codice_fiscale,
        anagrafica: anagrafica(&p.name, &p.first_name, &p.last_name, "", &p.eori),
        sede: indirizzo(&p.address, &p.civic, &p.zip, &p.city, &p.province, &p.country),
        stabile_organizzazione: None,
        rappresentante_fiscale: None,
    })
}

fn party_as_cedente(p: &Party) -> Result<CedentePrestatore, BuildError> {
    let id = party_vat_id(p).ok_or_else(|| {
        BuildError(format!("Il fornitore '{}' non ha un identificativo IVA, obbligatorio come cedente", p.display_name()))
    })?;
    let foreign = id.paese != "IT";
    Ok(CedentePrestatore {
        id_fiscale_iva: id,
        codice_fiscale: if foreign { None } else { Some(clean_code(&p.tax_code)).filter(|s| !s.is_empty()) },
        anagrafica: anagrafica(&p.name, &p.first_name, &p.last_name, "", &p.eori),
        regime_fiscale: "RF01".into(),
        sede: indirizzo(&p.address, &p.civic, &p.zip, &p.city, &p.province, &p.country),
        ..Default::default()
    })
}

fn destinatario(p: &Party, foreign: bool) -> (String, Option<String>) {
    if foreign {
        return (codes::CODICE_DESTINATARIO_ESTERO.into(), None);
    }
    let code = p.sdi_code.trim().to_ascii_uppercase();
    let valid_len = if p.is_pa { 6 } else { 7 };
    if code.len() == valid_len && code != codes::CODICE_DESTINATARIO_PEC_O_CASSETTO {
        (code, None)
    } else {
        let pec = p.pec.trim();
        ("0000000".into(), if pec.contains('@') { Some(pec.to_string()) } else { None })
    }
}

fn line_discounts(lc: &openfatt_core::calc::LineCalc) -> Vec<ScontoMaggiorazione> {
    if lc.discounts.is_empty() {
        return Vec::new();
    }
    let representable = lc.discounts.iter().all(|d| d.abs().scale() <= 2 || d.abs().normalize().scale() <= 2);
    if representable {
        lc.discounts
            .iter()
            .filter(|d| !d.is_zero())
            .map(|d| ScontoMaggiorazione {
                tipo: if d.is_sign_negative() { "MG".into() } else { "SC".into() },
                percentuale: Some(d.abs().normalize()),
                importo: None,
            })
            .collect()
    } else {
        // percentuali con più di due decimali: si indica l'importo unitario dello sconto
        let diff = lc.unit_net - lc.unit_discounted;
        if diff.is_zero() {
            return Vec::new();
        }
        vec![ScontoMaggiorazione {
            tipo: if diff.is_sign_negative() { "MG".into() } else { "SC".into() },
            percentuale: None,
            importo: Some(openfatt_core::money::round_dp(diff.abs(), 8)),
        }]
    }
}

/// Costruisce la fattura elettronica dal documento e dai suoi totali calcolati.
pub fn build(company: &Company, doc: &Document, totals: &Totals, opt: &BuildOptions) -> Result<FatturaElettronica, BuildError> {
    let td = doc
        .fe_type_effective()
        .ok_or_else(|| BuildError(format!("Il tipo '{}' non è un documento da trasmettere allo SdI", doc.kind.label())))?;
    if !codes::is_valid(codes::TIPO_DOCUMENTO, &td) {
        return Err(BuildError(format!("Tipo documento FatturaPA non valido: {}", td)));
    }
    let number = doc
        .number
        .ok_or_else(|| BuildError("Il documento deve essere emesso (numerato) prima della generazione XML".into()))?;
    let _ = number;

    let roles = roles_for(&td);
    let p = &doc.party;
    let foreign_party = !p.country.trim().is_empty() && p.country.trim().to_ascii_uppercase() != "IT";

    let (cedente, cessionario) = match roles {
        Roles::Normal => (company_as_cedente(company)?, party_as_cessionario(p)?),
        Roles::SupplierIsCedente => (party_as_cedente(p)?, company_as_cessionario(company)),
        Roles::SelfBoth => (company_as_cedente(company)?, company_as_cessionario(company)),
    };

    let is_pa = matches!(roles, Roles::Normal) && p.is_pa;
    let (codice_destinatario, pec_destinatario) = match roles {
        Roles::Normal => destinatario(p, foreign_party),
        // le integrazioni sono recapitate a se stessi
        _ => ("0000000".to_string(), None),
    };

    let id_trasmittente = if !company.transmitter_code.trim().is_empty() {
        IdFiscale {
            paese: if company.transmitter_country.trim().is_empty() { "IT".into() } else { company.transmitter_country.trim().to_ascii_uppercase() },
            codice: clean_code(&company.transmitter_code),
        }
    } else {
        // per le persone fisiche l'identificativo del trasmittente è il codice fiscale
        let cf = clean_code(&company.tax_code);
        let code = if cf.len() == 16 { cf } else { clean_code(&company.vat_number) };
        IdFiscale { paese: "IT".into(), codice: code }
    };

    let header = Header {
        dati_trasmissione: DatiTrasmissione {
            id_trasmittente,
            progressivo_invio: truncate(&basic_latin(&opt.progressivo), 10),
            formato_trasmissione: if is_pa { "FPA12".into() } else { "FPR12".into() },
            codice_destinatario,
            telefono: None,
            email: None,
            pec_destinatario,
        },
        cedente,
        rappresentante_fiscale: None,
        cessionario,
        terzo_intermediario: None,
        soggetto_emittente: if matches!(roles, Roles::SupplierIsCedente) { Some("CC".into()) } else { None },
    };

    // --- Dati generali ---
    let mut dgd = DatiGeneraliDocumento {
        tipo_documento: td.clone(),
        divisa: doc.currency_code().to_uppercase(),
        data: Some(doc.date),
        numero: truncate(&basic_latin(&doc.number_label()), 20),
        ..Default::default()
    };
    for w in &totals.withholdings {
        dgd.dati_ritenuta.push(DatiRitenuta {
            tipo_ritenuta: w.kind.clone(),
            importo_ritenuta: w.amount,
            aliquota_ritenuta: w.rate,
            causale_pagamento: w.payment_reason.clone(),
        });
    }
    if let Some(s) = totals.stamp {
        dgd.dati_bollo = Some(DatiBollo { bollo_virtuale: true, importo_bollo: Some(s) });
    }
    if let Some(pf) = &totals.pension_fund {
        if !pf.amount.is_zero() {
            dgd.dati_cassa_previdenziale.push(DatiCassaPrevidenziale {
                tipo_cassa: pf.kind.clone(),
                al_cassa: pf.rate,
                importo_contributo_cassa: pf.amount,
                imponibile_cassa: Some(pf.base),
                aliquota_iva: pf.vat.rate,
                ritenuta: pf.subject_to_withholding && !totals.withholdings.is_empty(),
                natura: pf.vat.natura.clone().filter(|n| !n.is_empty() && pf.vat.rate.is_zero()),
                riferimento_amministrazione: None,
            });
        }
    }
    dgd.importo_totale_documento = Some(totals.total);
    let causale_text = latin(&doc.causale);
    dgd.causale = chunks(&causale_text, 200);

    let mut generali = DatiGenerali { documento: dgd, ..Default::default() };
    for r in &doc.related {
        if r.number.trim().is_empty() {
            continue;
        }
        let d = DatiDocumentiCorrelati {
            riferimento_numero_linea: vec![],
            id_documento: truncate(&basic_latin(&r.number), 20),
            data: r.date,
            num_item: None,
            codice_commessa_convenzione: opt_latin(&r.commessa, 100),
            codice_cup: opt_basic(&r.cup, 15),
            codice_cig: opt_basic(&r.cig, 15),
        };
        match r.kind.as_str() {
            "CONTRATTO" => generali.contratti.push(d),
            "CONVENZIONE" => generali.convenzioni.push(d),
            "RICEZIONE" => generali.ricezioni.push(d),
            "FATTURA" => generali.fatture_collegate.push(d),
            _ => generali.ordini_acquisto.push(d),
        }
    }

    // --- Righe ---
    let mut linee = Vec::new();
    let mut index_to_line_number = std::collections::HashMap::new();
    for lc in &totals.lines {
        let src: Option<&DocLine> = lc.index.and_then(|i| doc.lines.get(i));
        let n = (linee.len() + 1) as u32;
        if let Some(i) = lc.index {
            index_to_line_number.insert(i, n);
        }
        let description = {
            let full = match src {
                Some(l) if !lc.is_text => l.description_with_tracking(),
                _ => match lc.subtotal {
                    Some(v) => format!("{} {} {}", lc.description.trim(), doc.currency_code(), openfatt_core::money::fmt_it(v, 2)),
                    None => lc.description.clone(),
                },
            };
            let d = latin(&full);
            if d.is_empty() { "-".to_string() } else { truncate(&d, 1000) }
        };
        let mut codici = Vec::new();
        if let Some(l) = src {
            let code = truncate(&latin(&l.code), 35);
            if !code.is_empty() {
                codici.push(CodiceArticolo {
                    tipo: if opt.article_code_type.is_empty() { "INTERNO".into() } else { truncate(&basic_latin(&opt.article_code_type), 35) },
                    valore: code,
                });
            }
        }
        let natura = lc.vat.natura.clone().filter(|n| !n.is_empty() && lc.vat.rate.is_zero());
        if lc.is_text {
            linee.push(DettaglioLinee {
                numero_linea: n,
                descrizione: description,
                prezzo_unitario: Decimal::ZERO,
                prezzo_totale: Decimal::ZERO,
                aliquota_iva: lc.vat.rate,
                natura,
                codice_articolo: codici,
                ..Default::default()
            });
            continue;
        }
        // La quantità non può essere negativa: i resi si esprimono con prezzo negativo
        let (qty, sign) = if lc.qty.is_sign_negative() { (lc.qty.abs(), Decimal::NEGATIVE_ONE) } else { (lc.qty, Decimal::ONE) };
        let unit = src.map(|l| l.unit.clone()).unwrap_or_default();
        let supply_type = src
            .map(|l| l.supply_type.trim().to_string())
            .filter(|s| codes::is_valid(codes::TIPO_CESSIONE_PRESTAZIONE, s));
        let supply_type = if lc.is_generated { None } else { supply_type };
        let mut altri = Vec::new();
        // dichiarazione d'intento: protocollo e data della ricevuta sulle righe N3.5
        if natura.as_deref() == Some("N3.5") && !doc.intent_protocol.trim().is_empty() {
            let decl = doc.party.intents.iter().find(|i| i.protocol == doc.intent_protocol);
            altri.push(AltriDatiGestionali {
                tipo_dato: "INTENTO".into(),
                riferimento_testo: Some(truncate(&basic_latin(doc.intent_protocol.trim()), 60)),
                riferimento_numero: None,
                riferimento_data: decl.and_then(|d| d.date),
            });
        }
        // documento in valuta: controvalore in euro della riga (FAQ AdE sulla valuta estera)
        if doc.is_foreign_currency() && linee.is_empty() {
            altri.push(AltriDatiGestionali {
                tipo_dato: "CAMBIO".into(),
                riferimento_testo: Some(format!("1 EUR = {} {}", doc.exchange_rate.normalize(), doc.currency_code())),
                riferimento_numero: Some(openfatt_core::money::round_dp(doc.exchange_rate, 8)),
                riferimento_data: Some(doc.date),
            });
            altri.push(AltriDatiGestionali {
                tipo_dato: "IMPONEUR".into(),
                riferimento_testo: Some("Imponibile del documento in euro".into()),
                riferimento_numero: Some(doc.to_eur(totals.taxable)),
                riferimento_data: None,
            });
            altri.push(AltriDatiGestionali {
                tipo_dato: "IVAEUR".into(),
                riferimento_testo: Some("Imposta del documento in euro".into()),
                riferimento_numero: Some(doc.to_eur(totals.tax)),
                riferimento_data: None,
            });
        }
        linee.push(DettaglioLinee {
            numero_linea: n,
            tipo_cessione_prestazione: supply_type,
            codice_articolo: codici,
            descrizione: description,
            quantita: Some(qty),
            unita_misura: opt_basic(&unit, 10),
            data_inizio_periodo: src.and_then(|l| l.period_start),
            data_fine_periodo: src.and_then(|l| l.period_end),
            prezzo_unitario: openfatt_core::money::round_dp(lc.unit_net * sign, 8),
            sconto_maggiorazione: line_discounts(lc),
            prezzo_totale: lc.total,
            aliquota_iva: lc.vat.rate,
            ritenuta: lc.withholding && !totals.withholdings.is_empty(),
            natura,
            riferimento_amministrazione: None,
            altri_dati_gestionali: altri,
        });
    }

    for r in &opt.ddt_refs {
        if let Some(date) = r.date {
            let mut lines: Vec<u32> = r.line_indexes.iter().filter_map(|i| index_to_line_number.get(i).copied()).collect();
            lines.sort();
            generali.ddt.push(DatiDdt { numero_ddt: truncate(&basic_latin(&r.number), 20), data_ddt: date, riferimento_numero_linea: lines });
        }
    }

    if doc.kind == openfatt_core::model::DocKind::AccompanyingInvoice {
        if let Some(t) = &doc.transport {
            generali.trasporto = Some(DatiTrasporto {
                vettore: None,
                mezzo_trasporto: opt_latin(&t.by, 80),
                causale_trasporto: opt_latin(&t.reason, 100),
                numero_colli: t.packages.filter(|n| *n >= 1 && *n <= 9999),
                descrizione: opt_latin(&t.appearance, 100),
                unita_misura_peso: if t.gross_weight.is_some() || t.net_weight.is_some() { Some("KG".into()) } else { None },
                peso_lordo: t.gross_weight,
                peso_netto: t.net_weight,
                data_ora_ritiro: None,
                data_inizio_trasporto: t.start.map(|d| d.date()),
                tipo_resa: None,
                indirizzo_resa: None,
                data_ora_consegna: None,
            });
        }
    }

    // --- Riepilogo ---
    let mut riepilogo = Vec::new();
    for r in &totals.vat_rows {
        let natura = r.natura.clone().filter(|n| !n.is_empty() && r.rate.is_zero());
        let rif = if natura.is_some() {
            let lr = latin(&r.legal_ref);
            let rif = if lr.is_empty() { codes::describe(codes::NATURA, natura.as_deref().unwrap_or("")).to_string() } else { lr };
            Some(truncate(&rif, 100)).filter(|s| !s.is_empty())
        } else {
            None
        };
        riepilogo.push(DatiRiepilogo {
            aliquota_iva: r.rate,
            natura,
            spese_accessorie: None,
            arrotondamento: if r.rounding.is_zero() { None } else { Some(r.rounding) },
            imponibile_importo: r.taxable,
            imposta: r.tax,
            esigibilita_iva: if r.liability.is_empty() { None } else { Some(r.liability.clone()) },
            riferimento_normativo: rif,
        });
    }

    // --- Pagamento ---
    let mut pagamenti = Vec::new();
    if !doc.due_dates.is_empty() && !totals.net_to_pay.is_zero() {
        let iban = Some(company.iban.replace(' ', "").to_ascii_uppercase()).filter(|s| openfatt_core::fiscal::iban_valido(s));
        let show_iban = doc.payment.as_ref().map(|p| p.show_iban).unwrap_or(true);
        let dettagli = doc
            .due_dates
            .iter()
            .map(|d| {
                let bank = matches!(d.mp_code.as_str(), "MP05" | "MP07" | "MP19" | "MP20" | "MP21");
                DettaglioPagamento {
                    beneficiario: None,
                    modalita_pagamento: d.mp_code.clone(),
                    data_riferimento_termini: None,
                    giorni_termini: None,
                    data_scadenza: Some(d.date),
                    importo: d.amount,
                    istituto_finanziario: if bank && show_iban { opt_latin(&company.bank_name, 80) } else { None },
                    iban: if bank && show_iban { iban.clone() } else { None },
                    abi: None,
                    cab: None,
                    bic: if bank && show_iban { Some(company.bic.trim().to_ascii_uppercase()).filter(|b| b.len() == 8 || b.len() == 11) } else { None },
                    codice_pagamento: None,
                }
            })
            .collect::<Vec<_>>();
        pagamenti.push(DatiPagamento { condizioni: if dettagli.len() > 1 { "TP01".into() } else { "TP02".into() }, dettagli });
    }

    let mut allegati = Vec::new();
    if let Some((name, data)) = &opt.attachment {
        allegati.push(Allegato {
            nome: truncate(&latin(name), 60),
            algoritmo_compressione: None,
            formato: Some("PDF".into()),
            descrizione: Some("Copia di cortesia".into()),
            dati: data.clone(),
        });
    }

    // righe solo descrittive senza importi: verifica che il riepilogo copra la loro aliquota
    let _ = doc.lines.iter().filter(|l| l.kind == LineKind::Text).count();

    Ok(FatturaElettronica {
        versione: header.dati_trasmissione.formato_trasmissione.clone(),
        header,
        bodies: vec![Body { dati_generali: generali, linee, riepilogo, dati_veicoli: None, pagamenti, allegati }],
    })
}
