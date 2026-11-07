//! Serializzazione XML del tracciato, nell'ordine esatto delle sequenze dello schema.

use chrono::{NaiveDate, NaiveDateTime};
use openfatt_core::money::{fmt_xml, fmt_xml_var};
use rust_decimal::Decimal;

use crate::model::*;

pub const NS_FATTURA: &str = "http://ivaservizi.agenziaentrate.gov.it/docs/xsd/fatture/v1.2";

pub struct XmlWriter {
    buf: String,
    stack: Vec<&'static str>,
    indent: bool,
}

pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            _ => o.push(c),
        }
    }
    o
}

impl XmlWriter {
    pub fn new(indent: bool) -> Self {
        XmlWriter { buf: String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"), stack: Vec::new(), indent }
    }

    fn pad(&mut self) {
        if self.indent {
            for _ in 0..self.stack.len() {
                self.buf.push_str("  ");
            }
        }
    }

    fn nl(&mut self) {
        if self.indent {
            self.buf.push('\n');
        }
    }

    pub fn open_attrs(&mut self, name: &'static str, attrs: &[(&str, &str)]) {
        self.pad();
        self.buf.push('<');
        self.buf.push_str(name);
        for (k, v) in attrs {
            self.buf.push_str(&format!(" {}=\"{}\"", k, escape(v)));
        }
        self.buf.push('>');
        self.nl();
        self.stack.push(name);
    }

    pub fn open(&mut self, name: &'static str) {
        self.open_attrs(name, &[]);
    }

    pub fn close(&mut self) {
        let name = self.stack.pop().expect("close senza open");
        self.pad();
        self.buf.push_str("</");
        self.buf.push_str(name);
        self.buf.push('>');
        self.nl();
    }

    pub fn el(&mut self, name: &str, value: &str) {
        self.pad();
        self.buf.push_str(&format!("<{}>{}</{}>", name, escape(value), name));
        self.nl();
    }

    pub fn opt(&mut self, name: &str, value: &Option<String>) {
        if let Some(v) = value {
            if !v.is_empty() {
                self.el(name, v);
            }
        }
    }

    pub fn dec2(&mut self, name: &str, v: Decimal) {
        self.el(name, &fmt_xml(v, 2));
    }

    pub fn opt_dec2(&mut self, name: &str, v: Option<Decimal>) {
        if let Some(v) = v {
            self.dec2(name, v);
        }
    }

    pub fn dec_var(&mut self, name: &str, v: Decimal, min: u32, max: u32) {
        self.el(name, &fmt_xml_var(v, min, max));
    }

    pub fn date(&mut self, name: &str, d: NaiveDate) {
        self.el(name, &d.format("%Y-%m-%d").to_string());
    }

    pub fn opt_date(&mut self, name: &str, d: Option<NaiveDate>) {
        if let Some(d) = d {
            self.date(name, d);
        }
    }

    pub fn opt_datetime(&mut self, name: &str, d: Option<NaiveDateTime>) {
        if let Some(d) = d {
            self.el(name, &d.format("%Y-%m-%dT%H:%M:%S").to_string());
        }
    }

    pub fn finish(mut self) -> String {
        while !self.stack.is_empty() {
            self.close();
        }
        self.buf
    }
}

fn id_fiscale(w: &mut XmlWriter, name: &'static str, id: &IdFiscale) {
    w.open(name);
    w.el("IdPaese", &id.paese);
    w.el("IdCodice", &id.codice);
    w.close();
}

fn anagrafica(w: &mut XmlWriter, a: &Anagrafica) {
    w.open("Anagrafica");
    if let Some(d) = &a.denominazione {
        w.el("Denominazione", d);
    } else {
        w.el("Nome", a.nome.as_deref().unwrap_or(""));
        w.el("Cognome", a.cognome.as_deref().unwrap_or(""));
    }
    w.opt("Titolo", &a.titolo);
    w.opt("CodEORI", &a.cod_eori);
    w.close();
}

fn indirizzo(w: &mut XmlWriter, name: &'static str, i: &Indirizzo) {
    w.open(name);
    w.el("Indirizzo", &i.indirizzo);
    w.opt("NumeroCivico", &i.numero_civico);
    w.el("CAP", &i.cap);
    w.el("Comune", &i.comune);
    w.opt("Provincia", &i.provincia);
    w.el("Nazione", &i.nazione);
    w.close();
}

fn soggetto(w: &mut XmlWriter, s: &Soggetto) {
    w.open("DatiAnagrafici");
    if let Some(id) = &s.id_fiscale_iva {
        id_fiscale(w, "IdFiscaleIVA", id);
    }
    w.opt("CodiceFiscale", &s.codice_fiscale);
    anagrafica(w, &s.anagrafica);
    w.close();
}

fn sconti(w: &mut XmlWriter, list: &[ScontoMaggiorazione]) {
    for s in list {
        w.open("ScontoMaggiorazione");
        w.el("Tipo", &s.tipo);
        if let Some(p) = s.percentuale {
            w.dec2("Percentuale", p);
        }
        if let Some(i) = s.importo {
            w.dec_var("Importo", i, 2, 8);
        }
        w.close();
    }
}

fn correlati(w: &mut XmlWriter, name: &'static str, list: &[DatiDocumentiCorrelati]) {
    for d in list {
        w.open(name);
        for l in &d.riferimento_numero_linea {
            w.el("RiferimentoNumeroLinea", &l.to_string());
        }
        w.el("IdDocumento", &d.id_documento);
        w.opt_date("Data", d.data);
        w.opt("NumItem", &d.num_item);
        w.opt("CodiceCommessaConvenzione", &d.codice_commessa_convenzione);
        w.opt("CodiceCUP", &d.codice_cup);
        w.opt("CodiceCIG", &d.codice_cig);
        w.close();
    }
}

fn header(w: &mut XmlWriter, h: &Header) {
    w.open("FatturaElettronicaHeader");

    let t = &h.dati_trasmissione;
    w.open("DatiTrasmissione");
    id_fiscale(w, "IdTrasmittente", &t.id_trasmittente);
    w.el("ProgressivoInvio", &t.progressivo_invio);
    w.el("FormatoTrasmissione", &t.formato_trasmissione);
    w.el("CodiceDestinatario", &t.codice_destinatario);
    if t.telefono.is_some() || t.email.is_some() {
        w.open("ContattiTrasmittente");
        w.opt("Telefono", &t.telefono);
        w.opt("Email", &t.email);
        w.close();
    }
    w.opt("PECDestinatario", &t.pec_destinatario);
    w.close();

    let c = &h.cedente;
    w.open("CedentePrestatore");
    w.open("DatiAnagrafici");
    id_fiscale(w, "IdFiscaleIVA", &c.id_fiscale_iva);
    w.opt("CodiceFiscale", &c.codice_fiscale);
    anagrafica(w, &c.anagrafica);
    w.opt("AlboProfessionale", &c.albo_professionale);
    w.opt("ProvinciaAlbo", &c.provincia_albo);
    w.opt("NumeroIscrizioneAlbo", &c.numero_iscrizione_albo);
    w.opt_date("DataIscrizioneAlbo", c.data_iscrizione_albo);
    w.el("RegimeFiscale", &c.regime_fiscale);
    w.close();
    indirizzo(w, "Sede", &c.sede);
    if let Some(s) = &c.stabile_organizzazione {
        indirizzo(w, "StabileOrganizzazione", s);
    }
    if let Some(r) = &c.iscrizione_rea {
        w.open("IscrizioneREA");
        w.el("Ufficio", &r.ufficio);
        w.el("NumeroREA", &r.numero_rea);
        w.opt_dec2("CapitaleSociale", r.capitale_sociale);
        w.opt("SocioUnico", &r.socio_unico);
        w.el("StatoLiquidazione", &r.stato_liquidazione);
        w.close();
    }
    if let Some(ct) = &c.contatti {
        if ct.telefono.is_some() || ct.fax.is_some() || ct.email.is_some() {
            w.open("Contatti");
            w.opt("Telefono", &ct.telefono);
            w.opt("Fax", &ct.fax);
            w.opt("Email", &ct.email);
            w.close();
        }
    }
    w.opt("RiferimentoAmministrazione", &c.riferimento_amministrazione);
    w.close();

    if let Some(r) = &h.rappresentante_fiscale {
        w.open("RappresentanteFiscale");
        soggetto(w, r);
        w.close();
    }

    let cc = &h.cessionario;
    w.open("CessionarioCommittente");
    w.open("DatiAnagrafici");
    if let Some(id) = &cc.id_fiscale_iva {
        id_fiscale(w, "IdFiscaleIVA", id);
    }
    w.opt("CodiceFiscale", &cc.codice_fiscale);
    anagrafica(w, &cc.anagrafica);
    w.close();
    indirizzo(w, "Sede", &cc.sede);
    if let Some(s) = &cc.stabile_organizzazione {
        indirizzo(w, "StabileOrganizzazione", s);
    }
    if let Some((id, a)) = &cc.rappresentante_fiscale {
        w.open("RappresentanteFiscale");
        id_fiscale(w, "IdFiscaleIVA", id);
        if let Some(d) = &a.denominazione {
            w.el("Denominazione", d);
        } else {
            w.el("Nome", a.nome.as_deref().unwrap_or(""));
            w.el("Cognome", a.cognome.as_deref().unwrap_or(""));
        }
        w.close();
    }
    w.close();

    if let Some(t) = &h.terzo_intermediario {
        w.open("TerzoIntermediarioOSoggettoEmittente");
        soggetto(w, t);
        w.close();
    }
    w.opt("SoggettoEmittente", &h.soggetto_emittente);
    w.close();
}

fn body(w: &mut XmlWriter, b: &Body) {
    w.open("FatturaElettronicaBody");
    let g = &b.dati_generali;
    w.open("DatiGenerali");

    let d = &g.documento;
    w.open("DatiGeneraliDocumento");
    w.el("TipoDocumento", &d.tipo_documento);
    w.el("Divisa", &d.divisa);
    if let Some(data) = d.data {
        w.date("Data", data);
    }
    w.el("Numero", &d.numero);
    for r in &d.dati_ritenuta {
        w.open("DatiRitenuta");
        w.el("TipoRitenuta", &r.tipo_ritenuta);
        w.dec2("ImportoRitenuta", r.importo_ritenuta);
        w.dec2("AliquotaRitenuta", r.aliquota_ritenuta);
        w.el("CausalePagamento", &r.causale_pagamento);
        w.close();
    }
    if let Some(bo) = &d.dati_bollo {
        w.open("DatiBollo");
        w.el("BolloVirtuale", "SI");
        w.opt_dec2("ImportoBollo", bo.importo_bollo);
        w.close();
    }
    for c in &d.dati_cassa_previdenziale {
        w.open("DatiCassaPrevidenziale");
        w.el("TipoCassa", &c.tipo_cassa);
        w.dec2("AlCassa", c.al_cassa);
        w.dec2("ImportoContributoCassa", c.importo_contributo_cassa);
        w.opt_dec2("ImponibileCassa", c.imponibile_cassa);
        w.dec2("AliquotaIVA", c.aliquota_iva);
        if c.ritenuta {
            w.el("Ritenuta", "SI");
        }
        w.opt("Natura", &c.natura);
        w.opt("RiferimentoAmministrazione", &c.riferimento_amministrazione);
        w.close();
    }
    sconti(w, &d.sconto_maggiorazione);
    w.opt_dec2("ImportoTotaleDocumento", d.importo_totale_documento);
    w.opt_dec2("Arrotondamento", d.arrotondamento);
    for c in &d.causale {
        w.el("Causale", c);
    }
    if d.art73 {
        w.el("Art73", "SI");
    }
    w.close();

    correlati(w, "DatiOrdineAcquisto", &g.ordini_acquisto);
    correlati(w, "DatiContratto", &g.contratti);
    correlati(w, "DatiConvenzione", &g.convenzioni);
    correlati(w, "DatiRicezione", &g.ricezioni);
    correlati(w, "DatiFattureCollegate", &g.fatture_collegate);
    for s in &g.sal {
        w.open("DatiSAL");
        w.el("RiferimentoFase", &s.to_string());
        w.close();
    }
    for ddt in &g.ddt {
        w.open("DatiDDT");
        w.el("NumeroDDT", &ddt.numero_ddt);
        w.date("DataDDT", ddt.data_ddt);
        for l in &ddt.riferimento_numero_linea {
            w.el("RiferimentoNumeroLinea", &l.to_string());
        }
        w.close();
    }
    if let Some(t) = &g.trasporto {
        w.open("DatiTrasporto");
        if let Some(v) = &t.vettore {
            w.open("DatiAnagraficiVettore");
            if let Some(id) = &v.id_fiscale_iva {
                id_fiscale(w, "IdFiscaleIVA", id);
            }
            w.opt("CodiceFiscale", &v.codice_fiscale);
            anagrafica(w, &v.anagrafica);
            w.close();
        }
        w.opt("MezzoTrasporto", &t.mezzo_trasporto);
        w.opt("CausaleTrasporto", &t.causale_trasporto);
        if let Some(n) = t.numero_colli {
            w.el("NumeroColli", &n.to_string());
        }
        w.opt("Descrizione", &t.descrizione);
        w.opt("UnitaMisuraPeso", &t.unita_misura_peso);
        if let Some(p) = t.peso_lordo {
            w.dec2("PesoLordo", p);
        }
        if let Some(p) = t.peso_netto {
            w.dec2("PesoNetto", p);
        }
        w.opt_datetime("DataOraRitiro", t.data_ora_ritiro);
        w.opt_date("DataInizioTrasporto", t.data_inizio_trasporto);
        w.opt("TipoResa", &t.tipo_resa);
        if let Some(i) = &t.indirizzo_resa {
            indirizzo(w, "IndirizzoResa", i);
        }
        w.opt_datetime("DataOraConsegna", t.data_ora_consegna);
        w.close();
    }
    if let Some((n, data)) = &g.fattura_principale {
        w.open("FatturaPrincipale");
        w.el("NumeroFatturaPrincipale", n);
        w.date("DataFatturaPrincipale", *data);
        w.close();
    }
    w.close(); // DatiGenerali

    w.open("DatiBeniServizi");
    for l in &b.linee {
        w.open("DettaglioLinee");
        w.el("NumeroLinea", &l.numero_linea.to_string());
        w.opt("TipoCessionePrestazione", &l.tipo_cessione_prestazione);
        for c in &l.codice_articolo {
            w.open("CodiceArticolo");
            w.el("CodiceTipo", &c.tipo);
            w.el("CodiceValore", &c.valore);
            w.close();
        }
        w.el("Descrizione", &l.descrizione);
        if let Some(q) = l.quantita {
            w.dec_var("Quantita", q, 2, 8);
        }
        w.opt("UnitaMisura", &l.unita_misura);
        w.opt_date("DataInizioPeriodo", l.data_inizio_periodo);
        w.opt_date("DataFinePeriodo", l.data_fine_periodo);
        w.dec_var("PrezzoUnitario", l.prezzo_unitario, 2, 8);
        sconti(w, &l.sconto_maggiorazione);
        w.dec_var("PrezzoTotale", l.prezzo_totale, 2, 8);
        w.dec2("AliquotaIVA", l.aliquota_iva);
        if l.ritenuta {
            w.el("Ritenuta", "SI");
        }
        w.opt("Natura", &l.natura);
        w.opt("RiferimentoAmministrazione", &l.riferimento_amministrazione);
        for a in &l.altri_dati_gestionali {
            w.open("AltriDatiGestionali");
            w.el("TipoDato", &a.tipo_dato);
            w.opt("RiferimentoTesto", &a.riferimento_testo);
            if let Some(n) = a.riferimento_numero {
                w.dec_var("RiferimentoNumero", n, 2, 8);
            }
            w.opt_date("RiferimentoData", a.riferimento_data);
            w.close();
        }
        w.close();
    }
    for r in &b.riepilogo {
        w.open("DatiRiepilogo");
        w.dec2("AliquotaIVA", r.aliquota_iva);
        w.opt("Natura", &r.natura);
        w.opt_dec2("SpeseAccessorie", r.spese_accessorie);
        if let Some(a) = r.arrotondamento {
            w.dec_var("Arrotondamento", a, 2, 8);
        }
        w.dec2("ImponibileImporto", r.imponibile_importo);
        w.dec2("Imposta", r.imposta);
        w.opt("EsigibilitaIVA", &r.esigibilita_iva);
        w.opt("RiferimentoNormativo", &r.riferimento_normativo);
        w.close();
    }
    w.close(); // DatiBeniServizi

    if let Some((data, percorso)) = &b.dati_veicoli {
        w.open("DatiVeicoli");
        w.date("Data", *data);
        w.el("TotalePercorso", percorso);
        w.close();
    }

    for p in &b.pagamenti {
        w.open("DatiPagamento");
        w.el("CondizioniPagamento", &p.condizioni);
        for d in &p.dettagli {
            w.open("DettaglioPagamento");
            w.opt("Beneficiario", &d.beneficiario);
            w.el("ModalitaPagamento", &d.modalita_pagamento);
            w.opt_date("DataRiferimentoTerminiPagamento", d.data_riferimento_termini);
            if let Some(g) = d.giorni_termini {
                w.el("GiorniTerminiPagamento", &g.to_string());
            }
            w.opt_date("DataScadenzaPagamento", d.data_scadenza);
            w.dec2("ImportoPagamento", d.importo);
            w.opt("IstitutoFinanziario", &d.istituto_finanziario);
            w.opt("IBAN", &d.iban);
            w.opt("ABI", &d.abi);
            w.opt("CAB", &d.cab);
            w.opt("BIC", &d.bic);
            w.opt("CodicePagamento", &d.codice_pagamento);
            w.close();
        }
        w.close();
    }

    for a in &b.allegati {
        use base64::Engine;
        w.open("Allegati");
        w.el("NomeAttachment", &a.nome);
        w.opt("AlgoritmoCompressione", &a.algoritmo_compressione);
        w.opt("FormatoAttachment", &a.formato);
        w.opt("DescrizioneAttachment", &a.descrizione);
        w.el("Attachment", &base64::engine::general_purpose::STANDARD.encode(&a.dati));
        w.close();
    }
    w.close(); // Body
}

/// Produce l'XML completo della fattura.
pub fn to_xml(f: &FatturaElettronica) -> String {
    let mut w = XmlWriter::new(true);
    w.open_attrs(
        "p:FatturaElettronica",
        &[
            ("versione", f.versione.as_str()),
            ("xmlns:ds", "http://www.w3.org/2000/09/xmldsig#"),
            ("xmlns:p", NS_FATTURA),
            ("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance"),
        ],
    );
    header(&mut w, &f.header);
    for b in &f.bodies {
        body(&mut w, b);
    }
    w.finish()
}
