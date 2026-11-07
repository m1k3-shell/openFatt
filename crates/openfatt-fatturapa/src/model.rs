//! Rappresentazione del tracciato FatturaPA 1.2.x (fatture ordinarie FPR12/FPA12).
//! I nomi dei campi seguono quelli dello schema ufficiale.

use chrono::{NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FatturaElettronica {
    /// FPR12 (privati) o FPA12 (pubblica amministrazione)
    pub versione: String,
    pub header: Header,
    pub bodies: Vec<Body>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IdFiscale {
    pub paese: String,
    pub codice: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Anagrafica {
    pub denominazione: Option<String>,
    pub nome: Option<String>,
    pub cognome: Option<String>,
    pub titolo: Option<String>,
    pub cod_eori: Option<String>,
}

impl Anagrafica {
    pub fn display(&self) -> String {
        if let Some(d) = &self.denominazione {
            return d.clone();
        }
        format!("{} {}", self.nome.clone().unwrap_or_default(), self.cognome.clone().unwrap_or_default())
            .trim()
            .to_string()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Indirizzo {
    pub indirizzo: String,
    pub numero_civico: Option<String>,
    pub cap: String,
    pub comune: String,
    pub provincia: Option<String>,
    pub nazione: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatiTrasmissione {
    pub id_trasmittente: IdFiscale,
    pub progressivo_invio: String,
    pub formato_trasmissione: String,
    pub codice_destinatario: String,
    pub telefono: Option<String>,
    pub email: Option<String>,
    pub pec_destinatario: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IscrizioneRea {
    pub ufficio: String,
    pub numero_rea: String,
    pub capitale_sociale: Option<Decimal>,
    pub socio_unico: Option<String>,
    pub stato_liquidazione: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Contatti {
    pub telefono: Option<String>,
    pub fax: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CedentePrestatore {
    pub id_fiscale_iva: IdFiscale,
    pub codice_fiscale: Option<String>,
    pub anagrafica: Anagrafica,
    pub albo_professionale: Option<String>,
    pub provincia_albo: Option<String>,
    pub numero_iscrizione_albo: Option<String>,
    pub data_iscrizione_albo: Option<NaiveDate>,
    pub regime_fiscale: String,
    pub sede: Indirizzo,
    pub stabile_organizzazione: Option<Indirizzo>,
    pub iscrizione_rea: Option<IscrizioneRea>,
    pub contatti: Option<Contatti>,
    pub riferimento_amministrazione: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Soggetto {
    pub id_fiscale_iva: Option<IdFiscale>,
    pub codice_fiscale: Option<String>,
    pub anagrafica: Anagrafica,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CessionarioCommittente {
    pub id_fiscale_iva: Option<IdFiscale>,
    pub codice_fiscale: Option<String>,
    pub anagrafica: Anagrafica,
    pub sede: Indirizzo,
    pub stabile_organizzazione: Option<Indirizzo>,
    /// Rappresentante fiscale del cessionario: id IVA e anagrafica
    pub rappresentante_fiscale: Option<(IdFiscale, Anagrafica)>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub dati_trasmissione: DatiTrasmissione,
    pub cedente: CedentePrestatore,
    pub rappresentante_fiscale: Option<Soggetto>,
    pub cessionario: CessionarioCommittente,
    pub terzo_intermediario: Option<Soggetto>,
    /// CC = cessionario, TZ = terzo
    pub soggetto_emittente: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatiRitenuta {
    pub tipo_ritenuta: String,
    pub importo_ritenuta: Decimal,
    pub aliquota_ritenuta: Decimal,
    pub causale_pagamento: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatiBollo {
    pub bollo_virtuale: bool,
    pub importo_bollo: Option<Decimal>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatiCassaPrevidenziale {
    pub tipo_cassa: String,
    pub al_cassa: Decimal,
    pub importo_contributo_cassa: Decimal,
    pub imponibile_cassa: Option<Decimal>,
    pub aliquota_iva: Decimal,
    pub ritenuta: bool,
    pub natura: Option<String>,
    pub riferimento_amministrazione: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScontoMaggiorazione {
    /// SC o MG
    pub tipo: String,
    pub percentuale: Option<Decimal>,
    pub importo: Option<Decimal>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatiDocumentiCorrelati {
    pub riferimento_numero_linea: Vec<u32>,
    pub id_documento: String,
    pub data: Option<NaiveDate>,
    pub num_item: Option<String>,
    pub codice_commessa_convenzione: Option<String>,
    pub codice_cup: Option<String>,
    pub codice_cig: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatiDdt {
    pub numero_ddt: String,
    pub data_ddt: NaiveDate,
    pub riferimento_numero_linea: Vec<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatiTrasporto {
    pub vettore: Option<Soggetto>,
    pub mezzo_trasporto: Option<String>,
    pub causale_trasporto: Option<String>,
    pub numero_colli: Option<u32>,
    pub descrizione: Option<String>,
    pub unita_misura_peso: Option<String>,
    pub peso_lordo: Option<Decimal>,
    pub peso_netto: Option<Decimal>,
    pub data_ora_ritiro: Option<NaiveDateTime>,
    pub data_inizio_trasporto: Option<NaiveDate>,
    pub tipo_resa: Option<String>,
    pub indirizzo_resa: Option<Indirizzo>,
    pub data_ora_consegna: Option<NaiveDateTime>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatiGeneraliDocumento {
    pub tipo_documento: String,
    pub divisa: String,
    pub data: Option<NaiveDate>,
    pub numero: String,
    pub dati_ritenuta: Vec<DatiRitenuta>,
    pub dati_bollo: Option<DatiBollo>,
    pub dati_cassa_previdenziale: Vec<DatiCassaPrevidenziale>,
    pub sconto_maggiorazione: Vec<ScontoMaggiorazione>,
    pub importo_totale_documento: Option<Decimal>,
    pub arrotondamento: Option<Decimal>,
    pub causale: Vec<String>,
    pub art73: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatiGenerali {
    pub documento: DatiGeneraliDocumento,
    pub ordini_acquisto: Vec<DatiDocumentiCorrelati>,
    pub contratti: Vec<DatiDocumentiCorrelati>,
    pub convenzioni: Vec<DatiDocumentiCorrelati>,
    pub ricezioni: Vec<DatiDocumentiCorrelati>,
    pub fatture_collegate: Vec<DatiDocumentiCorrelati>,
    pub sal: Vec<u32>,
    pub ddt: Vec<DatiDdt>,
    pub trasporto: Option<DatiTrasporto>,
    pub fattura_principale: Option<(String, NaiveDate)>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CodiceArticolo {
    pub tipo: String,
    pub valore: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AltriDatiGestionali {
    pub tipo_dato: String,
    pub riferimento_testo: Option<String>,
    pub riferimento_numero: Option<Decimal>,
    pub riferimento_data: Option<NaiveDate>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DettaglioLinee {
    pub numero_linea: u32,
    pub tipo_cessione_prestazione: Option<String>,
    pub codice_articolo: Vec<CodiceArticolo>,
    pub descrizione: String,
    pub quantita: Option<Decimal>,
    pub unita_misura: Option<String>,
    pub data_inizio_periodo: Option<NaiveDate>,
    pub data_fine_periodo: Option<NaiveDate>,
    pub prezzo_unitario: Decimal,
    pub sconto_maggiorazione: Vec<ScontoMaggiorazione>,
    pub prezzo_totale: Decimal,
    pub aliquota_iva: Decimal,
    pub ritenuta: bool,
    pub natura: Option<String>,
    pub riferimento_amministrazione: Option<String>,
    pub altri_dati_gestionali: Vec<AltriDatiGestionali>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatiRiepilogo {
    pub aliquota_iva: Decimal,
    pub natura: Option<String>,
    pub spese_accessorie: Option<Decimal>,
    pub arrotondamento: Option<Decimal>,
    pub imponibile_importo: Decimal,
    pub imposta: Decimal,
    pub esigibilita_iva: Option<String>,
    pub riferimento_normativo: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DettaglioPagamento {
    pub beneficiario: Option<String>,
    pub modalita_pagamento: String,
    pub data_riferimento_termini: Option<NaiveDate>,
    pub giorni_termini: Option<u32>,
    pub data_scadenza: Option<NaiveDate>,
    pub importo: Decimal,
    pub istituto_finanziario: Option<String>,
    pub iban: Option<String>,
    pub abi: Option<String>,
    pub cab: Option<String>,
    pub bic: Option<String>,
    pub codice_pagamento: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatiPagamento {
    pub condizioni: String,
    pub dettagli: Vec<DettaglioPagamento>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Allegato {
    pub nome: String,
    pub algoritmo_compressione: Option<String>,
    pub formato: Option<String>,
    pub descrizione: Option<String>,
    #[serde(skip)]
    pub dati: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub dati_generali: DatiGenerali,
    pub linee: Vec<DettaglioLinee>,
    pub riepilogo: Vec<DatiRiepilogo>,
    pub dati_veicoli: Option<(NaiveDate, String)>,
    pub pagamenti: Vec<DatiPagamento>,
    pub allegati: Vec<Allegato>,
}
