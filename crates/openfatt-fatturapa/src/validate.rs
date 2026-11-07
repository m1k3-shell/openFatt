//! Controlli preventivi sul contenuto, ricalcati sull'"Elenco dei controlli effettuati
//! sul file fattura dal Sistema di Interscambio" (versione 2.0 del 31/01/2025).
//! Ogni anomalia riporta il codice di errore SdI corrispondente, quando esiste.
//!
//! I controlli che richiedono l'Anagrafe Tributaria o l'IPA (esistenza della partita IVA,
//! codice ufficio attivo, duplicati) non sono riproducibili localmente.

use std::collections::{BTreeMap, BTreeSet};

use openfatt_core::codes;
use openfatt_core::fiscal;
use openfatt_core::money::{pct_of, round2};
use rust_decimal::Decimal;

use crate::model::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Scarto certo da parte dello SdI.
    Error,
    /// Possibile problema o dato consigliato.
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    /// Codice SdI (es. "00423") o codice interno ("GA-...").
    pub code: String,
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self.severity {
            Severity::Error => "ERRORE",
            Severity::Warning => "Avviso",
        };
        write!(f, "[{} {}] {}: {}", s, self.code, self.path, self.message)
    }
}

struct Ctx {
    issues: Vec<Issue>,
}

impl Ctx {
    fn err(&mut self, code: &str, path: &str, msg: impl Into<String>) {
        self.issues.push(Issue { severity: Severity::Error, code: code.into(), path: path.into(), message: msg.into() });
    }
    fn warn(&mut self, code: &str, path: &str, msg: impl Into<String>) {
        self.issues.push(Issue { severity: Severity::Warning, code: code.into(), path: path.into(), message: msg.into() });
    }
    fn len(&mut self, path: &str, value: &str, min: usize, max: usize) {
        let n = value.chars().count();
        if n < min || n > max {
            self.err("GA-LEN", path, format!("lunghezza {} non ammessa (consentita {}..{})", n, min, max));
        }
    }
    fn latin(&mut self, path: &str, value: &str, max: usize) {
        self.len(path, value, 1, max);
        if value.chars().any(|c| (c as u32) > 0xFF || (c as u32) < 0x20) {
            self.err("GA-CHR", path, "contiene caratteri non ammessi (solo Latin-1)");
        }
    }
    fn basic(&mut self, path: &str, value: &str, max: usize) {
        self.len(path, value, 1, max);
        if value.chars().any(|c| !(' '..='~').contains(&c)) {
            self.err("GA-CHR", path, "contiene caratteri non ammessi (solo ASCII)");
        }
    }
}

fn natura_key(n: &Option<String>) -> String {
    n.clone().unwrap_or_default()
}

fn check_indirizzo(c: &mut Ctx, path: &str, i: &Indirizzo) {
    c.latin(&format!("{}/Indirizzo", path), &i.indirizzo, 60);
    if let Some(n) = &i.numero_civico {
        c.basic(&format!("{}/NumeroCivico", path), n, 8);
    }
    if !fiscal::cap_valido(&i.cap) {
        c.err("GA-CAP", &format!("{}/CAP", path), "il CAP deve essere di 5 cifre (per l'estero 00000)");
    }
    c.latin(&format!("{}/Comune", path), &i.comune, 60);
    if let Some(p) = &i.provincia {
        if !codes::PROVINCE.contains(&p.as_str()) {
            c.warn("GA-PROV", &format!("{}/Provincia", path), format!("sigla provincia '{}' non riconosciuta", p));
        }
    }
    if i.nazione.len() != 2 || !i.nazione.bytes().all(|b| b.is_ascii_uppercase()) {
        c.err("GA-NAZ", &format!("{}/Nazione", path), "codice nazione ISO di 2 lettere maiuscole");
    }
}

fn check_anagrafica(c: &mut Ctx, path: &str, a: &Anagrafica) {
    match (&a.denominazione, &a.nome, &a.cognome) {
        (Some(d), _, _) => c.latin(&format!("{}/Denominazione", path), d, 80),
        (None, Some(n), Some(cg)) => {
            c.latin(&format!("{}/Nome", path), n, 60);
            c.latin(&format!("{}/Cognome", path), cg, 60);
        }
        _ => c.err("GA-ANA", path, "indicare la denominazione oppure nome e cognome"),
    }
}

fn check_id_iva(c: &mut Ctx, code_err: &str, path: &str, id: &IdFiscale) {
    if id.paese.len() != 2 {
        c.err(code_err, &format!("{}/IdPaese", path), "paese non valido");
    }
    c.len(&format!("{}/IdCodice", path), &id.codice, 1, 28);
    if id.paese == "IT" && !fiscal::partita_iva_valida(&id.codice) {
        // per il trasmittente persona fisica l'IdCodice è il codice fiscale
        if !(id.codice.len() == 16 && fiscal::codice_fiscale_valido(&id.codice)) {
            c.err(code_err, &format!("{}/IdCodice", path), format!("partita IVA '{}' non valida", id.codice));
        }
    }
}

/// Esegue tutti i controlli. La fattura è inviabile se non ci sono `Severity::Error`.
pub fn validate(f: &FatturaElettronica) -> Vec<Issue> {
    let mut c = Ctx { issues: Vec::new() };
    let h = &f.header;
    let t = &h.dati_trasmissione;

    // --- Trasmissione ---
    if f.versione != "FPR12" && f.versione != "FPA12" {
        c.err("00428", "FatturaElettronica/@versione", "versione deve essere FPR12 o FPA12");
    }
    if t.formato_trasmissione != f.versione {
        c.err("00428", "DatiTrasmissione/FormatoTrasmissione", "non coerente con l'attributo versione");
    }
    check_id_iva(&mut c, "00300", "DatiTrasmissione/IdTrasmittente", &t.id_trasmittente);
    if t.progressivo_invio.is_empty() || t.progressivo_invio.len() > 10 || !t.progressivo_invio.bytes().all(|b| b.is_ascii_alphanumeric()) {
        c.err("GA-PRG", "DatiTrasmissione/ProgressivoInvio", "da 1 a 10 caratteri alfanumerici");
    }
    let pa = t.formato_trasmissione == "FPA12";
    if t.codice_destinatario.len() == 7 && pa || t.codice_destinatario.len() == 6 && !pa {
        c.err("00427", "DatiTrasmissione/CodiceDestinatario", "lunghezza non coerente con il formato (6 PA, 7 privati)");
    }
    if !t.codice_destinatario.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()) || t.codice_destinatario.len() < 6 {
        c.err("00311", "DatiTrasmissione/CodiceDestinatario", "formato non valido");
    }
    if let Some(p) = &t.pec_destinatario {
        if t.codice_destinatario != "0000000" {
            c.warn("GA-PEC", "DatiTrasmissione/PECDestinatario", "la PEC si indica solo con codice destinatario 0000000");
        }
        if !p.contains('@') {
            c.err("GA-PEC", "DatiTrasmissione/PECDestinatario", "indirizzo PEC non valido");
        }
    }

    // --- Cedente ---
    let ced = &h.cedente;
    check_id_iva(&mut c, "00301", "CedentePrestatore/IdFiscaleIVA", &ced.id_fiscale_iva);
    if let Some(cf) = &ced.codice_fiscale {
        if ced.id_fiscale_iva.paese == "IT" && !fiscal::codice_fiscale_valido(cf) {
            c.err("00302", "CedentePrestatore/CodiceFiscale", format!("codice fiscale '{}' non valido", cf));
        }
    }
    check_anagrafica(&mut c, "CedentePrestatore/Anagrafica", &ced.anagrafica);
    if !codes::is_valid(codes::REGIME_FISCALE, &ced.regime_fiscale) {
        c.err("GA-RF", "CedentePrestatore/RegimeFiscale", "regime fiscale non valido");
    }
    check_indirizzo(&mut c, "CedentePrestatore/Sede", &ced.sede);
    if let Some(rea) = &ced.iscrizione_rea {
        if !codes::PROVINCE.contains(&rea.ufficio.as_str()) {
            c.err("GA-REA", "IscrizioneREA/Ufficio", "sigla provincia dell'ufficio REA non valida");
        }
    }

    // --- Cessionario ---
    let cess = &h.cessionario;
    if cess.id_fiscale_iva.is_none() && cess.codice_fiscale.is_none() {
        c.err("00417", "CessionarioCommittente/DatiAnagrafici", "indicare almeno partita IVA o codice fiscale");
    }
    if let Some(id) = &cess.id_fiscale_iva {
        check_id_iva(&mut c, "00305", "CessionarioCommittente/IdFiscaleIVA", id);
    }
    if let Some(cf) = &cess.codice_fiscale {
        if !fiscal::codice_fiscale_valido(cf) {
            c.err("00306", "CessionarioCommittente/CodiceFiscale", format!("codice fiscale '{}' non valido", cf));
        }
    }
    check_anagrafica(&mut c, "CessionarioCommittente/Anagrafica", &cess.anagrafica);
    check_indirizzo(&mut c, "CessionarioCommittente/Sede", &cess.sede);

    let ced_paese = ced.id_fiscale_iva.paese.as_str();
    let cess_paese = cess.id_fiscale_iva.as_ref().map(|i| i.paese.as_str()).unwrap_or("IT");
    if ced_paese != "IT" && cess_paese != "IT" {
        c.err("00476", "IdPaese", "cedente e cessionario non possono essere entrambi esteri");
    }
    let same_subject = cess.id_fiscale_iva.as_ref().map(|i| i == &ced.id_fiscale_iva).unwrap_or(false);

    if f.bodies.is_empty() {
        c.err("GA-BODY", "FatturaElettronicaBody", "nessun corpo fattura");
    }

    for (bi, b) in f.bodies.iter().enumerate() {
        let bp = format!("Body[{}]", bi + 1);
        let d = &b.dati_generali.documento;
        let td = d.tipo_documento.as_str();
        if !codes::is_valid(codes::TIPO_DOCUMENTO, td) {
            c.err("GA-TD", &format!("{}/TipoDocumento", bp), "tipo documento non valido");
        }
        if d.divisa.len() != 3 {
            c.err("GA-DIV", &format!("{}/Divisa", bp), "divisa ISO 4217 di 3 lettere");
        }
        match d.data {
            None => c.err("GA-DATA", &format!("{}/Data", bp), "data mancante"),
            Some(data) => {
                if data > chrono::Local::now().date_naive() {
                    c.warn("00403", &format!("{}/Data", bp), "data successiva a oggi: sarà scartata se successiva alla ricezione");
                }
                for fc in &b.dati_generali.fatture_collegate {
                    if let Some(dfc) = fc.data {
                        if data < dfc {
                            c.err("00418", &format!("{}/Data", bp), "data antecedente a quella della fattura collegata");
                        }
                    }
                }
            }
        }
        c.basic(&format!("{}/Numero", bp), &d.numero, 20);
        if !d.numero.chars().any(|ch| ch.is_ascii_digit()) {
            c.err("00425", &format!("{}/Numero", bp), "il numero deve contenere almeno una cifra");
        }

        // ruoli per tipo documento
        let must_differ = ["TD01", "TD02", "TD03", "TD06", "TD16", "TD17", "TD18", "TD19", "TD20", "TD24", "TD25", "TD28", "TD29"];
        let must_equal = ["TD21", "TD27"];
        if must_differ.contains(&td) && same_subject {
            c.err("00471", &format!("{}/TipoDocumento", bp), "cedente e cessionario non possono coincidere per questo tipo documento");
        }
        if must_equal.contains(&td) && !same_subject {
            c.err("00472", &format!("{}/TipoDocumento", bp), "cedente e cessionario devono coincidere per questo tipo documento");
        }
        if ["TD17", "TD18", "TD19"].contains(&td) && ced_paese == "IT" {
            c.err("00473", "CedentePrestatore/IdPaese", "per le integrazioni TD17-TD19 il cedente deve essere estero");
        }
        if td == "TD18" && !codes::PAESI_UE.contains(&ced_paese) {
            c.err("00473", "CedentePrestatore/IdPaese", "TD18: il cedente deve essere di un paese UE");
        }
        if td == "TD28" && ced_paese != "SM" {
            c.err("00473", "CedentePrestatore/IdPaese", "TD28: il cedente deve essere di San Marino");
        }
        if ["TD16", "TD17", "TD18", "TD19", "TD20", "TD22", "TD23", "TD26", "TD27", "TD28"].contains(&td) && cess.id_fiscale_iva.is_none() {
            c.err("00475", "CessionarioCommittente/IdFiscaleIVA", "per questo tipo documento è obbligatoria la partita IVA del cessionario");
        }

        // causali
        for (i, ca) in d.causale.iter().enumerate() {
            c.latin(&format!("{}/Causale[{}]", bp, i + 1), ca, 200);
        }
        // sconti di testata
        for s in &d.sconto_maggiorazione {
            if s.percentuale.is_none() && s.importo.is_none() {
                c.err("00437", &format!("{}/ScontoMaggiorazione", bp), "indicare percentuale o importo");
            }
        }

        // ritenute
        let any_line_ritenuta = b.linee.iter().any(|l| l.ritenuta);
        if any_line_ritenuta && d.dati_ritenuta.is_empty() {
            c.err("00411", &format!("{}/DatiRitenuta", bp), "righe con Ritenuta=SI ma DatiRitenuta assente");
        }
        for r in &d.dati_ritenuta {
            if !codes::is_valid(codes::TIPO_RITENUTA, &r.tipo_ritenuta) {
                c.err("GA-RIT", &format!("{}/DatiRitenuta/TipoRitenuta", bp), "tipo ritenuta non valido");
            }
            if !codes::is_valid(codes::CAUSALE_PAGAMENTO, &r.causale_pagamento) {
                c.err("GA-RIT", &format!("{}/DatiRitenuta/CausalePagamento", bp), "causale di pagamento non valida");
            }
        }

        // casse
        for (i, cp) in d.dati_cassa_previdenziale.iter().enumerate() {
            let p = format!("{}/DatiCassaPrevidenziale[{}]", bp, i + 1);
            if !codes::is_valid(codes::TIPO_CASSA, &cp.tipo_cassa) {
                c.err("GA-CASSA", &p, "tipo cassa non valido");
            }
            if cp.aliquota_iva.is_zero() && cp.natura.is_none() {
                c.err("00413", &format!("{}/Natura", p), "natura obbligatoria con aliquota zero");
            }
            if !cp.aliquota_iva.is_zero() && cp.natura.is_some() {
                c.err("00414", &format!("{}/Natura", p), "natura non ammessa con aliquota diversa da zero");
            }
            if cp.ritenuta && d.dati_ritenuta.is_empty() {
                c.err("00415", &format!("{}/Ritenuta", p), "Ritenuta=SI ma DatiRitenuta assente");
            }
        }

        if b.linee.is_empty() {
            c.err("GA-LIN", &format!("{}/DettaglioLinee", bp), "almeno una riga obbligatoria");
        }

        // righe
        let mut line_keys: BTreeSet<(Decimal, String)> = BTreeSet::new();
        let mut sum_by_rate: BTreeMap<Decimal, Decimal> = BTreeMap::new();
        for (i, l) in b.linee.iter().enumerate() {
            let p = format!("{}/DettaglioLinee[{}]", bp, i + 1);
            if l.numero_linea as usize != i + 1 {
                c.warn("GA-NUM", &format!("{}/NumeroLinea", p), "numerazione righe non progressiva");
            }
            c.latin(&format!("{}/Descrizione", p), &l.descrizione, 1000);
            if let Some(q) = l.quantita {
                if q.is_sign_negative() {
                    c.err("GA-QTA", &format!("{}/Quantita", p), "la quantità non può essere negativa");
                }
            }
            if l.aliquota_iva.is_zero() && l.natura.is_none() {
                c.err("00400", &format!("{}/Natura", p), "natura obbligatoria con aliquota zero");
            }
            if !l.aliquota_iva.is_zero() && l.natura.is_some() {
                c.err("00401", &format!("{}/Natura", p), "natura non ammessa con aliquota diversa da zero");
            }
            if l.aliquota_iva > Decimal::ONE_HUNDRED || l.aliquota_iva.is_sign_negative() {
                c.err("00424", &format!("{}/AliquotaIVA", p), "aliquota non espressa in percentuale");
            }
            if let Some(n) = &l.natura {
                if codes::NATURA_DEPRECATA.contains(&n.as_str()) {
                    c.err("00445", &format!("{}/Natura", p), "codici natura generici N2, N3, N6 non più ammessi");
                } else if !codes::is_valid(codes::NATURA, n) {
                    c.err("GA-NAT", &format!("{}/Natura", p), "codice natura non valido");
                }
            }
            for s in &l.sconto_maggiorazione {
                if s.percentuale.is_none() && s.importo.is_none() {
                    c.err("00438", &format!("{}/ScontoMaggiorazione", p), "indicare percentuale o importo");
                }
                if let Some(pc) = s.percentuale {
                    if pc > Decimal::ONE_HUNDRED {
                        c.err("GA-SC", &format!("{}/ScontoMaggiorazione", p), "percentuale oltre 100");
                    }
                }
            }
            // 00423: PrezzoTotale = (PrezzoUnitario +/- sconti/maggiorazioni) x Quantità
            let mut unit = l.prezzo_unitario;
            let mut sconto_imp = Decimal::ZERO;
            let mut magg_imp = Decimal::ZERO;
            for s in &l.sconto_maggiorazione {
                if let Some(imp) = s.importo {
                    if s.tipo == "SC" {
                        sconto_imp += imp;
                    } else {
                        magg_imp += imp;
                    }
                } else if let Some(pc) = s.percentuale {
                    let delta = pct_of(unit, pc);
                    if s.tipo == "SC" {
                        unit -= delta;
                    } else {
                        unit += delta;
                    }
                }
            }
            unit = unit - sconto_imp + magg_imp;
            let expected = unit * l.quantita.unwrap_or(Decimal::ONE);
            if (expected - l.prezzo_totale).abs() >= Decimal::new(1, 2) {
                c.err(
                    "00423",
                    &format!("{}/PrezzoTotale", p),
                    format!("PrezzoTotale {} non coerente con il calcolo atteso {}", l.prezzo_totale, round2(expected)),
                );
            }
            line_keys.insert((l.aliquota_iva, natura_key(&l.natura)));
            *sum_by_rate.entry(l.aliquota_iva).or_default() += l.prezzo_totale;
        }
        for cp in &d.dati_cassa_previdenziale {
            line_keys.insert((cp.aliquota_iva, natura_key(&cp.natura)));
            *sum_by_rate.entry(cp.aliquota_iva).or_default() += cp.importo_contributo_cassa;
        }

        // riepilogo
        if b.riepilogo.is_empty() {
            c.err("00419", &format!("{}/DatiRiepilogo", bp), "riepilogo IVA assente");
        }
        let mut riep_keys: BTreeSet<(Decimal, String)> = BTreeSet::new();
        let mut riep_by_rate: BTreeMap<Decimal, Decimal> = BTreeMap::new();
        for (i, r) in b.riepilogo.iter().enumerate() {
            let p = format!("{}/DatiRiepilogo[{}]", bp, i + 1);
            if r.aliquota_iva.is_zero() && r.natura.is_none() {
                c.err("00429", &format!("{}/Natura", p), "natura obbligatoria con aliquota zero");
            }
            if !r.aliquota_iva.is_zero() && r.natura.is_some() {
                c.err("00430", &format!("{}/Natura", p), "natura non ammessa con aliquota diversa da zero");
            }
            if let Some(n) = &r.natura {
                if codes::NATURA_DEPRECATA.contains(&n.as_str()) {
                    c.err("00445", &format!("{}/Natura", p), "codici natura generici N2, N3, N6 non più ammessi");
                }
                if n.starts_with("N6") && r.esigibilita_iva.as_deref() == Some("S") {
                    c.err("00420", &format!("{}/EsigibilitaIVA", p), "inversione contabile incompatibile con scissione dei pagamenti");
                }
                if r.riferimento_normativo.is_none() {
                    c.warn("GA-RIF", &format!("{}/RiferimentoNormativo", p), "consigliato indicare la norma di esenzione/non imponibilità");
                }
            }
            // 00421: Imposta = Imponibile x Aliquota / 100 (tolleranza 0,01)
            let expected = pct_of(r.imponibile_importo, r.aliquota_iva);
            if (expected - r.imposta).abs() >= Decimal::new(1, 2) {
                c.err("00421", &format!("{}/Imposta", p), format!("imposta {} attesa {}", r.imposta, round2(expected)));
            }
            riep_keys.insert((r.aliquota_iva, natura_key(&r.natura)));
            *riep_by_rate.entry(r.aliquota_iva).or_default() += r.imponibile_importo - r.arrotondamento.unwrap_or_default();
        }
        // 00443/00444/00419: corrispondenza fra aliquote/nature di righe e riepilogo
        for k in &line_keys {
            if !riep_keys.contains(k) {
                c.err(if k.1.is_empty() { "00443" } else { "00444" }, &format!("{}/DatiRiepilogo", bp), format!("manca il riepilogo per aliquota {} {}", k.0, k.1));
            }
        }
        for k in &riep_keys {
            if !line_keys.contains(k) {
                c.err(if k.1.is_empty() { "00443" } else { "00444" }, &format!("{}/DatiRiepilogo", bp), format!("riepilogo per aliquota {} {} senza righe corrispondenti", k.0, k.1));
            }
        }
        // 00422: Imponibile = somma PrezzoTotale + contributi cassa + arrotondamenti (tolleranza 1 euro)
        for (rate, sum_lines) in &sum_by_rate {
            let riep = riep_by_rate.get(rate).copied().unwrap_or_default();
            if (riep - sum_lines).abs() >= Decimal::ONE {
                c.err("00422", &format!("{}/DatiRiepilogo", bp), format!("imponibile per aliquota {} non coerente con le righe ({} vs {})", rate, riep, sum_lines));
            }
        }

        // pagamenti
        for (i, pg) in b.pagamenti.iter().enumerate() {
            let p = format!("{}/DatiPagamento[{}]", bp, i + 1);
            if !codes::is_valid(codes::CONDIZIONI_PAGAMENTO, &pg.condizioni) {
                c.err("GA-PAG", &p, "condizioni di pagamento non valide");
            }
            if pg.dettagli.is_empty() {
                c.err("GA-PAG", &p, "almeno un dettaglio pagamento");
            }
            for dp in &pg.dettagli {
                if !codes::is_valid(codes::MODALITA_PAGAMENTO, &dp.modalita_pagamento) {
                    c.err("GA-PAG", &format!("{}/ModalitaPagamento", p), "modalità di pagamento non valida");
                }
                if let Some(iban) = &dp.iban {
                    if !fiscal::iban_valido(iban) {
                        c.err("GA-IBAN", &format!("{}/IBAN", p), "IBAN non valido");
                    }
                }
            }
        }

        // coerenza del totale documento (solo avviso: non è un controllo SdI)
        if let Some(tot) = d.importo_totale_documento {
            let computed: Decimal = b.riepilogo.iter().map(|r| r.imponibile_importo + r.imposta).sum();
            if (computed - tot).abs() >= Decimal::new(1, 2) {
                c.warn("GA-TOT", &format!("{}/ImportoTotaleDocumento", bp), format!("totale {} diverso dalla somma del riepilogo {}", tot, computed));
            }
        }
        if pa && b.dati_generali.ordini_acquisto.iter().all(|o| o.codice_cig.is_none()) && b.dati_generali.contratti.is_empty() {
            c.warn("GA-CIG", &format!("{}/DatiOrdineAcquisto", bp), "per la PA di norma servono riferimento ordine e CIG/CUP");
        }
    }

    c.issues.sort_by(|a, b| a.severity.cmp(&b.severity));
    c.issues
}

pub fn has_errors(issues: &[Issue]) -> bool {
    issues.iter().any(|i| i.severity == Severity::Error)
}
