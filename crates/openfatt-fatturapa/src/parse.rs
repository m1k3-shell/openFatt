//! Lettura di fatture elettroniche (ricevute dai fornitori o esportate da altri programmi).
//! La lettura è tollerante: si basa sui nomi locali degli elementi e ignora i namespace.

use std::str::FromStr;

use chrono::{NaiveDate, NaiveDateTime};
use roxmltree::Node;
use rust_decimal::Decimal;

use crate::model::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ParseError {}

pub(crate) fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

pub(crate) fn children<'a, 'i>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children().filter(move |c| c.is_element() && c.tag_name().name() == name)
}

pub(crate) fn text(n: Node, name: &str) -> Option<String> {
    child(n, name).and_then(|c| c.text()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn text_or(n: Node, name: &str) -> String {
    text(n, name).unwrap_or_default()
}

fn dec(n: Node, name: &str) -> Option<Decimal> {
    text(n, name).and_then(|s| Decimal::from_str(&s).ok())
}

fn dec_or(n: Node, name: &str) -> Decimal {
    dec(n, name).unwrap_or_default()
}

fn date(n: Node, name: &str) -> Option<NaiveDate> {
    text(n, name).and_then(|s| NaiveDate::parse_from_str(s.get(..10).unwrap_or(&s), "%Y-%m-%d").ok())
}

fn datetime(n: Node, name: &str) -> Option<NaiveDateTime> {
    text(n, name).and_then(|s| {
        let s = s.get(..19).unwrap_or(&s).to_string();
        NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S").ok()
    })
}

fn si(n: Node, name: &str) -> bool {
    text(n, name).map(|s| s.eq_ignore_ascii_case("SI")).unwrap_or(false)
}

fn id_fiscale(n: Option<Node>) -> Option<IdFiscale> {
    let n = n?;
    Some(IdFiscale { paese: text_or(n, "IdPaese"), codice: text_or(n, "IdCodice") })
}

fn anagrafica(n: Option<Node>) -> Anagrafica {
    match n {
        None => Anagrafica::default(),
        Some(n) => Anagrafica {
            denominazione: text(n, "Denominazione"),
            nome: text(n, "Nome"),
            cognome: text(n, "Cognome"),
            titolo: text(n, "Titolo"),
            cod_eori: text(n, "CodEORI"),
        },
    }
}

fn indirizzo(n: Option<Node>) -> Indirizzo {
    match n {
        None => Indirizzo::default(),
        Some(n) => Indirizzo {
            indirizzo: text_or(n, "Indirizzo"),
            numero_civico: text(n, "NumeroCivico"),
            cap: text_or(n, "CAP"),
            comune: text_or(n, "Comune"),
            provincia: text(n, "Provincia"),
            nazione: text_or(n, "Nazione"),
        },
    }
}

fn soggetto(n: Option<Node>) -> Option<Soggetto> {
    let n = n?;
    let da = child(n, "DatiAnagrafici").unwrap_or(n);
    Some(Soggetto {
        id_fiscale_iva: id_fiscale(child(da, "IdFiscaleIVA")),
        codice_fiscale: text(da, "CodiceFiscale"),
        anagrafica: anagrafica(child(da, "Anagrafica")),
    })
}

fn sconti(n: Node) -> Vec<ScontoMaggiorazione> {
    children(n, "ScontoMaggiorazione")
        .map(|s| ScontoMaggiorazione { tipo: text_or(s, "Tipo"), percentuale: dec(s, "Percentuale"), importo: dec(s, "Importo") })
        .collect()
}

fn correlati(n: Node, name: &'static str) -> Vec<DatiDocumentiCorrelati> {
    children(n, name)
        .map(|d| DatiDocumentiCorrelati {
            riferimento_numero_linea: children(d, "RiferimentoNumeroLinea").filter_map(|x| x.text()?.trim().parse().ok()).collect(),
            id_documento: text_or(d, "IdDocumento"),
            data: date(d, "Data"),
            num_item: text(d, "NumItem"),
            codice_commessa_convenzione: text(d, "CodiceCommessaConvenzione"),
            codice_cup: text(d, "CodiceCUP"),
            codice_cig: text(d, "CodiceCIG"),
        })
        .collect()
}

fn header(h: Node) -> Header {
    let dt = child(h, "DatiTrasmissione");
    let dati_trasmissione = match dt {
        None => DatiTrasmissione::default(),
        Some(dt) => DatiTrasmissione {
            id_trasmittente: id_fiscale(child(dt, "IdTrasmittente")).unwrap_or_default(),
            progressivo_invio: text_or(dt, "ProgressivoInvio"),
            formato_trasmissione: text_or(dt, "FormatoTrasmissione"),
            codice_destinatario: text_or(dt, "CodiceDestinatario"),
            telefono: child(dt, "ContattiTrasmittente").and_then(|c| text(c, "Telefono")),
            email: child(dt, "ContattiTrasmittente").and_then(|c| text(c, "Email")),
            pec_destinatario: text(dt, "PECDestinatario"),
        },
    };

    let cedente = match child(h, "CedentePrestatore") {
        None => CedentePrestatore::default(),
        Some(cp) => {
            let da = child(cp, "DatiAnagrafici");
            let rea = child(cp, "IscrizioneREA").map(|r| IscrizioneRea {
                ufficio: text_or(r, "Ufficio"),
                numero_rea: text_or(r, "NumeroREA"),
                capitale_sociale: dec(r, "CapitaleSociale"),
                socio_unico: text(r, "SocioUnico"),
                stato_liquidazione: text_or(r, "StatoLiquidazione"),
            });
            let contatti = child(cp, "Contatti").map(|c| Contatti { telefono: text(c, "Telefono"), fax: text(c, "Fax"), email: text(c, "Email") });
            CedentePrestatore {
                id_fiscale_iva: da.and_then(|d| id_fiscale(child(d, "IdFiscaleIVA"))).unwrap_or_default(),
                codice_fiscale: da.and_then(|d| text(d, "CodiceFiscale")),
                anagrafica: anagrafica(da.and_then(|d| child(d, "Anagrafica"))),
                albo_professionale: da.and_then(|d| text(d, "AlboProfessionale")),
                provincia_albo: da.and_then(|d| text(d, "ProvinciaAlbo")),
                numero_iscrizione_albo: da.and_then(|d| text(d, "NumeroIscrizioneAlbo")),
                data_iscrizione_albo: da.and_then(|d| date(d, "DataIscrizioneAlbo")),
                regime_fiscale: da.map(|d| text_or(d, "RegimeFiscale")).unwrap_or_default(),
                sede: indirizzo(child(cp, "Sede")),
                stabile_organizzazione: child(cp, "StabileOrganizzazione").map(|s| indirizzo(Some(s))),
                iscrizione_rea: rea,
                contatti,
                riferimento_amministrazione: text(cp, "RiferimentoAmministrazione"),
            }
        }
    };

    let cessionario = match child(h, "CessionarioCommittente") {
        None => CessionarioCommittente::default(),
        Some(cc) => {
            let da = child(cc, "DatiAnagrafici");
            CessionarioCommittente {
                id_fiscale_iva: da.and_then(|d| id_fiscale(child(d, "IdFiscaleIVA"))),
                codice_fiscale: da.and_then(|d| text(d, "CodiceFiscale")),
                anagrafica: anagrafica(da.and_then(|d| child(d, "Anagrafica"))),
                sede: indirizzo(child(cc, "Sede")),
                stabile_organizzazione: child(cc, "StabileOrganizzazione").map(|s| indirizzo(Some(s))),
                rappresentante_fiscale: child(cc, "RappresentanteFiscale").map(|r| {
                    (
                        id_fiscale(child(r, "IdFiscaleIVA")).unwrap_or_default(),
                        Anagrafica { denominazione: text(r, "Denominazione"), nome: text(r, "Nome"), cognome: text(r, "Cognome"), titolo: None, cod_eori: None },
                    )
                }),
            }
        }
    };

    Header {
        dati_trasmissione,
        cedente,
        rappresentante_fiscale: soggetto(child(h, "RappresentanteFiscale")),
        cessionario,
        terzo_intermediario: soggetto(child(h, "TerzoIntermediarioOSoggettoEmittente")),
        soggetto_emittente: text(h, "SoggettoEmittente"),
    }
}

fn body(b: Node) -> Body {
    let dg = child(b, "DatiGenerali");
    let dgd = dg.and_then(|g| child(g, "DatiGeneraliDocumento"));
    let documento = match dgd {
        None => DatiGeneraliDocumento::default(),
        Some(d) => DatiGeneraliDocumento {
            tipo_documento: text_or(d, "TipoDocumento"),
            divisa: text_or(d, "Divisa"),
            data: date(d, "Data"),
            numero: text_or(d, "Numero"),
            dati_ritenuta: children(d, "DatiRitenuta")
                .map(|r| DatiRitenuta {
                    tipo_ritenuta: text_or(r, "TipoRitenuta"),
                    importo_ritenuta: dec_or(r, "ImportoRitenuta"),
                    aliquota_ritenuta: dec_or(r, "AliquotaRitenuta"),
                    causale_pagamento: text_or(r, "CausalePagamento"),
                })
                .collect(),
            dati_bollo: child(d, "DatiBollo").map(|bo| DatiBollo { bollo_virtuale: si(bo, "BolloVirtuale"), importo_bollo: dec(bo, "ImportoBollo") }),
            dati_cassa_previdenziale: children(d, "DatiCassaPrevidenziale")
                .map(|c| DatiCassaPrevidenziale {
                    tipo_cassa: text_or(c, "TipoCassa"),
                    al_cassa: dec_or(c, "AlCassa"),
                    importo_contributo_cassa: dec_or(c, "ImportoContributoCassa"),
                    imponibile_cassa: dec(c, "ImponibileCassa"),
                    aliquota_iva: dec_or(c, "AliquotaIVA"),
                    ritenuta: si(c, "Ritenuta"),
                    natura: text(c, "Natura"),
                    riferimento_amministrazione: text(c, "RiferimentoAmministrazione"),
                })
                .collect(),
            sconto_maggiorazione: sconti(d),
            importo_totale_documento: dec(d, "ImportoTotaleDocumento"),
            arrotondamento: dec(d, "Arrotondamento"),
            causale: children(d, "Causale").filter_map(|c| c.text().map(|t| t.trim().to_string())).collect(),
            art73: si(d, "Art73"),
        },
    };
    let dati_generali = match dg {
        None => DatiGenerali { documento, ..Default::default() },
        Some(g) => DatiGenerali {
            documento,
            ordini_acquisto: correlati(g, "DatiOrdineAcquisto"),
            contratti: correlati(g, "DatiContratto"),
            convenzioni: correlati(g, "DatiConvenzione"),
            ricezioni: correlati(g, "DatiRicezione"),
            fatture_collegate: correlati(g, "DatiFattureCollegate"),
            sal: children(g, "DatiSAL").filter_map(|s| text(s, "RiferimentoFase")?.parse().ok()).collect(),
            ddt: children(g, "DatiDDT")
                .filter_map(|d| {
                    Some(DatiDdt {
                        numero_ddt: text_or(d, "NumeroDDT"),
                        data_ddt: date(d, "DataDDT")?,
                        riferimento_numero_linea: children(d, "RiferimentoNumeroLinea").filter_map(|x| x.text()?.trim().parse().ok()).collect(),
                    })
                })
                .collect(),
            trasporto: child(g, "DatiTrasporto").map(|t| DatiTrasporto {
                vettore: child(t, "DatiAnagraficiVettore").map(|v| Soggetto {
                    id_fiscale_iva: id_fiscale(child(v, "IdFiscaleIVA")),
                    codice_fiscale: text(v, "CodiceFiscale"),
                    anagrafica: anagrafica(child(v, "Anagrafica")),
                }),
                mezzo_trasporto: text(t, "MezzoTrasporto"),
                causale_trasporto: text(t, "CausaleTrasporto"),
                numero_colli: text(t, "NumeroColli").and_then(|s| s.parse().ok()),
                descrizione: text(t, "Descrizione"),
                unita_misura_peso: text(t, "UnitaMisuraPeso"),
                peso_lordo: dec(t, "PesoLordo"),
                peso_netto: dec(t, "PesoNetto"),
                data_ora_ritiro: datetime(t, "DataOraRitiro"),
                data_inizio_trasporto: date(t, "DataInizioTrasporto"),
                tipo_resa: text(t, "TipoResa"),
                indirizzo_resa: child(t, "IndirizzoResa").map(|i| indirizzo(Some(i))),
                data_ora_consegna: datetime(t, "DataOraConsegna"),
            }),
            fattura_principale: child(g, "FatturaPrincipale")
                .and_then(|fp| Some((text_or(fp, "NumeroFatturaPrincipale"), date(fp, "DataFatturaPrincipale")?))),
        },
    };

    let dbs = child(b, "DatiBeniServizi");
    let linee = dbs
        .map(|d| {
            children(d, "DettaglioLinee")
                .map(|l| DettaglioLinee {
                    numero_linea: text(l, "NumeroLinea").and_then(|s| s.parse().ok()).unwrap_or(0),
                    tipo_cessione_prestazione: text(l, "TipoCessionePrestazione"),
                    codice_articolo: children(l, "CodiceArticolo")
                        .map(|c| CodiceArticolo { tipo: text_or(c, "CodiceTipo"), valore: text_or(c, "CodiceValore") })
                        .collect(),
                    descrizione: text_or(l, "Descrizione"),
                    quantita: dec(l, "Quantita"),
                    unita_misura: text(l, "UnitaMisura"),
                    data_inizio_periodo: date(l, "DataInizioPeriodo"),
                    data_fine_periodo: date(l, "DataFinePeriodo"),
                    prezzo_unitario: dec_or(l, "PrezzoUnitario"),
                    sconto_maggiorazione: sconti(l),
                    prezzo_totale: dec_or(l, "PrezzoTotale"),
                    aliquota_iva: dec_or(l, "AliquotaIVA"),
                    ritenuta: si(l, "Ritenuta"),
                    natura: text(l, "Natura"),
                    riferimento_amministrazione: text(l, "RiferimentoAmministrazione"),
                    altri_dati_gestionali: children(l, "AltriDatiGestionali")
                        .map(|a| AltriDatiGestionali {
                            tipo_dato: text_or(a, "TipoDato"),
                            riferimento_testo: text(a, "RiferimentoTesto"),
                            riferimento_numero: dec(a, "RiferimentoNumero"),
                            riferimento_data: date(a, "RiferimentoData"),
                        })
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default();
    let riepilogo = dbs
        .map(|d| {
            children(d, "DatiRiepilogo")
                .map(|r| DatiRiepilogo {
                    aliquota_iva: dec_or(r, "AliquotaIVA"),
                    natura: text(r, "Natura"),
                    spese_accessorie: dec(r, "SpeseAccessorie"),
                    arrotondamento: dec(r, "Arrotondamento"),
                    imponibile_importo: dec_or(r, "ImponibileImporto"),
                    imposta: dec_or(r, "Imposta"),
                    esigibilita_iva: text(r, "EsigibilitaIVA"),
                    riferimento_normativo: text(r, "RiferimentoNormativo"),
                })
                .collect()
        })
        .unwrap_or_default();

    let pagamenti = children(b, "DatiPagamento")
        .map(|p| DatiPagamento {
            condizioni: text_or(p, "CondizioniPagamento"),
            dettagli: children(p, "DettaglioPagamento")
                .map(|d| DettaglioPagamento {
                    beneficiario: text(d, "Beneficiario"),
                    modalita_pagamento: text_or(d, "ModalitaPagamento"),
                    data_riferimento_termini: date(d, "DataRiferimentoTerminiPagamento"),
                    giorni_termini: text(d, "GiorniTerminiPagamento").and_then(|s| s.parse().ok()),
                    data_scadenza: date(d, "DataScadenzaPagamento"),
                    importo: dec_or(d, "ImportoPagamento"),
                    istituto_finanziario: text(d, "IstitutoFinanziario"),
                    iban: text(d, "IBAN"),
                    abi: text(d, "ABI"),
                    cab: text(d, "CAB"),
                    bic: text(d, "BIC"),
                    codice_pagamento: text(d, "CodicePagamento"),
                })
                .collect(),
        })
        .collect();

    let allegati = children(b, "Allegati")
        .map(|a| {
            use base64::Engine;
            let raw: String = text_or(a, "Attachment").chars().filter(|c| !c.is_whitespace()).collect();
            Allegato {
                nome: text_or(a, "NomeAttachment"),
                algoritmo_compressione: text(a, "AlgoritmoCompressione"),
                formato: text(a, "FormatoAttachment"),
                descrizione: text(a, "DescrizioneAttachment"),
                dati: base64::engine::general_purpose::STANDARD.decode(raw.as_bytes()).unwrap_or_default(),
            }
        })
        .collect();

    let dati_veicoli = child(b, "DatiVeicoli").and_then(|v| Some((date(v, "Data")?, text_or(v, "TotalePercorso"))));

    Body { dati_generali, linee, riepilogo, dati_veicoli, pagamenti, allegati }
}

/// Converte i byte in testo, gestendo BOM e codifiche Latin-1/Windows-1252 dichiarate.
pub fn decode_xml_bytes(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xFE, 0xFF]) {
        let le = b.starts_with(&[0xFF, 0xFE]);
        let units: Vec<u16> = b[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        return String::from_utf16_lossy(&units);
    }
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        // ISO-8859-1 / Windows-1252: ogni byte corrisponde al code point
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

/// Rimuove la dichiarazione di encoding (roxmltree accetta solo testo già decodificato).
fn strip_decl(s: &str) -> &str {
    let t = s.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with("<?xml") {
        if let Some(end) = t.find("?>") {
            return &t[end + 2..];
        }
    }
    t
}

/// Legge una fattura elettronica XML (non firmata; per i .p7m usare prima `p7m::extract`).
pub fn parse(xml: &str) -> Result<FatturaElettronica, ParseError> {
    let opts = roxmltree::ParsingOptions { allow_dtd: false, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(strip_decl(xml), opts).map_err(|e| ParseError(format!("XML non valido: {}", e)))?;
    let root = doc.root_element();
    if root.tag_name().name() != "FatturaElettronica" {
        return Err(ParseError(format!("elemento radice '{}' non è FatturaElettronica", root.tag_name().name())));
    }
    let versione = root.attribute("versione").unwrap_or("").to_string();
    let h = child(root, "FatturaElettronicaHeader").ok_or_else(|| ParseError("FatturaElettronicaHeader mancante".into()))?;
    let bodies: Vec<Body> = children(root, "FatturaElettronicaBody").map(body).collect();
    if bodies.is_empty() {
        return Err(ParseError("FatturaElettronicaBody mancante".into()));
    }
    Ok(FatturaElettronica { versione, header: header(h), bodies })
}

pub fn parse_bytes(bytes: &[u8]) -> Result<FatturaElettronica, ParseError> {
    parse(&decode_xml_bytes(bytes))
}
