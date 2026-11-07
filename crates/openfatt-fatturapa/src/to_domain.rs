//! Conversione di una fattura ricevuta nel modello del gestionale (fattura di acquisto).

use openfatt_core::model::*;
use rust_decimal::Decimal;

use crate::model::*;

/// Soggetto fornitore ricavato dal cedente/prestatore.
pub fn supplier_from(c: &CedentePrestatore) -> Party {
    let a = &c.anagrafica;
    Party {
        role: PartyRole::Supplier,
        name: a.denominazione.clone().unwrap_or_default(),
        first_name: if a.denominazione.is_none() { a.nome.clone().unwrap_or_default() } else { String::new() },
        last_name: if a.denominazione.is_none() { a.cognome.clone().unwrap_or_default() } else { String::new() },
        vat_country: c.id_fiscale_iva.paese.clone(),
        vat_number: c.id_fiscale_iva.codice.clone(),
        tax_code: c.codice_fiscale.clone().unwrap_or_default(),
        address: c.sede.indirizzo.clone(),
        civic: c.sede.numero_civico.clone().unwrap_or_default(),
        zip: c.sede.cap.clone(),
        city: c.sede.comune.clone(),
        province: c.sede.provincia.clone().unwrap_or_default(),
        country: c.sede.nazione.clone(),
        email: c.contatti.as_ref().and_then(|x| x.email.clone()).unwrap_or_default(),
        phone: c.contatti.as_ref().and_then(|x| x.telefono.clone()).unwrap_or_default(),
        ..Default::default()
    }
}

/// Tipo di documento del gestionale per un TipoDocumento ricevuto.
pub fn purchase_kind(td: &str) -> DocKind {
    match td {
        "TD04" => DocKind::PurchaseCreditNote,
        _ => DocKind::PurchaseInvoice,
    }
}

/// Documento di acquisto da un corpo fattura. Le righe riportano importi e aliquote
/// così come ricevuti; le scadenze derivano dai DettaglioPagamento.
pub fn purchase_document(f: &FatturaElettronica, body: usize) -> Option<Document> {
    let b = f.bodies.get(body)?;
    let d = &b.dati_generali.documento;
    let mut doc = Document::new(purchase_kind(&d.tipo_documento));
    doc.date = d.data.unwrap_or_else(|| chrono::Local::now().date_naive());
    doc.external_number = d.numero.clone();
    doc.party = supplier_from(&f.header.cedente);
    doc.fe_type = d.tipo_documento.clone();
    doc.causale = d.causale.join(" ");
    doc.stamp.mode = StampMode::No;
    for l in &b.linee {
        let qty = l.quantita.unwrap_or(Decimal::ONE);
        // si usa il prezzo unitario già scontato, così il totale riga coincide con il PrezzoTotale
        let unit_price = if qty.is_zero() { l.prezzo_totale } else { l.prezzo_totale / qty };
        let unit_price = openfatt_core::money::round_dp(unit_price, 8);
        doc.lines.push(DocLine {
            kind: LineKind::Item,
            code: l.codice_articolo.first().map(|c| c.valore.clone()).unwrap_or_default(),
            description: l.descrizione.clone(),
            qty: if qty.is_zero() { Decimal::ONE } else { qty },
            unit: l.unita_misura.clone().unwrap_or_default(),
            unit_price,
            vat: VatRef { vat_id: None, rate: l.aliquota_iva, natura: l.natura.clone(), legal_ref: String::new() },
            withholding: l.ritenuta,
            period_start: l.data_inizio_periodo,
            period_end: l.data_fine_periodo,
            ..Default::default()
        });
    }
    for r in &d.dati_ritenuta {
        doc.withholdings.push(Withholding {
            kind: r.tipo_ritenuta.clone(),
            rate: r.aliquota_ritenuta,
            base_percent: Decimal::ONE_HUNDRED,
            payment_reason: r.causale_pagamento.clone(),
        });
    }
    if let Some(c) = d.dati_cassa_previdenziale.first() {
        doc.pension_fund = Some(PensionFund {
            kind: c.tipo_cassa.clone(),
            rate: c.al_cassa,
            vat: VatRef { vat_id: None, rate: c.aliquota_iva, natura: c.natura.clone(), legal_ref: String::new() },
            subject_to_withholding: c.ritenuta,
        });
        for l in doc.lines.iter_mut() {
            l.pension_fund = true;
        }
    }
    if b.riepilogo.iter().any(|r| r.esigibilita_iva.as_deref() == Some("S")) {
        doc.vat_liability = "S".into();
    }
    for p in &b.pagamenti {
        for dp in &p.dettagli {
            doc.due_dates.push(DueDate {
                id: 0,
                date: dp.data_scadenza.unwrap_or(doc.date),
                amount: dp.importo,
                mp_code: dp.modalita_pagamento.clone(),
                paid: Decimal::ZERO,
                writeoff: Decimal::ZERO,
                paid_date: None,
            });
        }
    }
    for o in &b.dati_generali.ordini_acquisto {
        doc.related.push(RelatedDoc { kind: "ORDINE".into(), number: o.id_documento.clone(), date: o.data, cig: o.codice_cig.clone().unwrap_or_default(), cup: o.codice_cup.clone().unwrap_or_default(), ..Default::default() });
    }
    Some(doc)
}

/// Totale dichiarato del corpo fattura (ImportoTotaleDocumento o somma del riepilogo).
pub fn declared_total(b: &Body) -> Decimal {
    b.dati_generali
        .documento
        .importo_totale_documento
        .unwrap_or_else(|| b.riepilogo.iter().map(|r| r.imponibile_importo + r.imposta).sum())
}
