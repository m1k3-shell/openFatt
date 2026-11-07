//! Tabelle di codici ufficiali FatturaPA (schema 1.2.3, specifiche tecniche v1.4).
//! Fonte: Agenzia delle Entrate, www.fatturapa.gov.it. Le descrizioni sono abbreviate.

pub type CodeTable = &'static [(&'static str, &'static str)];

pub fn describe(table: CodeTable, code: &str) -> &'static str {
    table.iter().find(|(c, _)| *c == code).map(|(_, d)| *d).unwrap_or("")
}

pub fn is_valid(table: CodeTable, code: &str) -> bool {
    table.iter().any(|(c, _)| *c == code)
}

pub const TIPO_DOCUMENTO: CodeTable = &[
    ("TD01", "Fattura"),
    ("TD02", "Acconto/anticipo su fattura"),
    ("TD03", "Acconto/anticipo su parcella"),
    ("TD04", "Nota di credito"),
    ("TD05", "Nota di debito"),
    ("TD06", "Parcella"),
    ("TD16", "Integrazione fattura reverse charge interno"),
    ("TD17", "Integrazione/autofattura acquisto servizi dall'estero"),
    ("TD18", "Integrazione acquisto beni intracomunitari"),
    ("TD19", "Integrazione/autofattura acquisto beni ex art.17 c.2"),
    ("TD20", "Autofattura per regolarizzazione e integrazione"),
    ("TD21", "Autofattura per splafonamento"),
    ("TD22", "Estrazione beni da Deposito IVA"),
    ("TD23", "Estrazione beni da Deposito IVA con versamento IVA"),
    ("TD24", "Fattura differita art.21 c.4 lett. a"),
    ("TD25", "Fattura differita art.21 c.4 terzo periodo lett. b"),
    ("TD26", "Cessione beni ammortizzabili e passaggi interni"),
    ("TD27", "Fattura per autoconsumo o cessioni gratuite senza rivalsa"),
    ("TD28", "Acquisti da San Marino con IVA (fattura cartacea)"),
    ("TD29", "Comunicazione per omessa o irregolare fatturazione"),
];

/// Codici natura ammessi. I generici N2, N3, N6 sono scartati dal 2021 (errore 00445).
pub const NATURA: CodeTable = &[
    ("N1", "Escluse ex art. 15"),
    ("N2.1", "Non soggette artt. da 7 a 7-septies"),
    ("N2.2", "Non soggette - altri casi"),
    ("N3.1", "Non imponibili - esportazioni"),
    ("N3.2", "Non imponibili - cessioni intracomunitarie"),
    ("N3.3", "Non imponibili - cessioni verso San Marino"),
    ("N3.4", "Non imponibili - operazioni assimilate alle esportazioni"),
    ("N3.5", "Non imponibili - a seguito di dichiarazioni d'intento"),
    ("N3.6", "Non imponibili - altre operazioni"),
    ("N4", "Esenti"),
    ("N5", "Regime del margine / IVA non esposta"),
    ("N6.1", "Inversione contabile - rottami e materiali di recupero"),
    ("N6.2", "Inversione contabile - oro e argento"),
    ("N6.3", "Inversione contabile - subappalto edile"),
    ("N6.4", "Inversione contabile - cessione di fabbricati"),
    ("N6.5", "Inversione contabile - telefoni cellulari"),
    ("N6.6", "Inversione contabile - prodotti elettronici"),
    ("N6.7", "Inversione contabile - comparto edile e connessi"),
    ("N6.8", "Inversione contabile - settore energetico"),
    ("N6.9", "Inversione contabile - altri casi"),
    ("N7", "IVA assolta in altro stato UE"),
];

/// Codici natura generici presenti nello schema ma non più accettati dallo SdI.
pub const NATURA_DEPRECATA: &[&str] = &["N2", "N3", "N6"];

pub const REGIME_FISCALE: CodeTable = &[
    ("RF01", "Ordinario"),
    ("RF02", "Contribuenti minimi"),
    ("RF04", "Agricoltura e pesca"),
    ("RF05", "Vendita sali e tabacchi"),
    ("RF06", "Commercio fiammiferi"),
    ("RF07", "Editoria"),
    ("RF08", "Servizi di telefonia pubblica"),
    ("RF09", "Rivendita documenti di trasporto e sosta"),
    ("RF10", "Intrattenimenti e giochi"),
    ("RF11", "Agenzie di viaggi e turismo"),
    ("RF12", "Agriturismo"),
    ("RF13", "Vendite a domicilio"),
    ("RF14", "Beni usati, arte, antiquariato"),
    ("RF15", "Aste di arte e antiquariato"),
    ("RF16", "IVA per cassa P.A."),
    ("RF17", "IVA per cassa"),
    ("RF18", "Altro"),
    ("RF19", "Forfettario"),
    ("RF20", "Franchigia IVA transfrontaliera (Dir. UE 2020/285)"),
];

pub const MODALITA_PAGAMENTO: CodeTable = &[
    ("MP01", "Contanti"),
    ("MP02", "Assegno"),
    ("MP03", "Assegno circolare"),
    ("MP04", "Contanti presso Tesoreria"),
    ("MP05", "Bonifico"),
    ("MP06", "Vaglia cambiario"),
    ("MP07", "Bollettino bancario"),
    ("MP08", "Carta di pagamento"),
    ("MP09", "RID"),
    ("MP10", "RID utenze"),
    ("MP11", "RID veloce"),
    ("MP12", "RIBA"),
    ("MP13", "MAV"),
    ("MP14", "Quietanza erario"),
    ("MP15", "Giroconto su conti di contabilità speciale"),
    ("MP16", "Domiciliazione bancaria"),
    ("MP17", "Domiciliazione postale"),
    ("MP18", "Bollettino di c/c postale"),
    ("MP19", "SEPA Direct Debit"),
    ("MP20", "SEPA Direct Debit CORE"),
    ("MP21", "SEPA Direct Debit B2B"),
    ("MP22", "Trattenuta su somme già riscosse"),
    ("MP23", "PagoPA"),
];

pub const CONDIZIONI_PAGAMENTO: CodeTable = &[
    ("TP01", "Pagamento a rate"),
    ("TP02", "Pagamento completo"),
    ("TP03", "Anticipo"),
];

pub const TIPO_CASSA: CodeTable = &[
    ("TC01", "Cassa avvocati e procuratori legali"),
    ("TC02", "Cassa dottori commercialisti"),
    ("TC03", "Cassa geometri"),
    ("TC04", "Cassa ingegneri e architetti (Inarcassa)"),
    ("TC05", "Cassa del notariato"),
    ("TC06", "Cassa ragionieri e periti commerciali"),
    ("TC07", "ENASARCO"),
    ("TC08", "ENPACL"),
    ("TC09", "ENPAM"),
    ("TC10", "ENPAF"),
    ("TC11", "ENPAV"),
    ("TC12", "ENPAIA"),
    ("TC13", "Fondo imprese di spedizione e agenzie marittime"),
    ("TC14", "INPGI"),
    ("TC15", "ONAOSI"),
    ("TC16", "CASAGIT"),
    ("TC17", "EPPI"),
    ("TC18", "EPAP"),
    ("TC19", "ENPAB"),
    ("TC20", "ENPAPI"),
    ("TC21", "ENPAP"),
    ("TC22", "INPS"),
];

pub const TIPO_RITENUTA: CodeTable = &[
    ("RT01", "Ritenuta persone fisiche"),
    ("RT02", "Ritenuta persone giuridiche"),
    ("RT03", "Contributo INPS"),
    ("RT04", "Contributo ENASARCO"),
    ("RT05", "Contributo ENPAM"),
    ("RT06", "Altro contributo previdenziale"),
];

/// Causali di pagamento del modello CU (usate in DatiRitenuta).
pub const CAUSALE_PAGAMENTO: CodeTable = &[
    ("A", "Prestazioni di lavoro autonomo abituale"),
    ("B", "Utilizzazione opere dell'ingegno da parte dell'autore"),
    ("C", "Utili da contratti di associazione in partecipazione"),
    ("D", "Utili spettanti ai soci promotori"),
    ("E", "Levata di protesti cambiari"),
    ("G", "Indennità per cessazione attività sportiva"),
    ("H", "Indennità per cessazione rapporti di agenzia"),
    ("I", "Indennità per cessazione funzioni notarili"),
    ("L", "Utilizzazione opere dell'ingegno (non autore)"),
    ("L1", "Redditi da utilizzazione opere acquisite a titolo gratuito"),
    ("M", "Lavoro autonomo occasionale"),
    ("M1", "Redditi per obblighi di fare/non fare"),
    ("M2", "Prestazioni non abituali con iscrizione gestione separata"),
    ("N", "Indennità di trasferta e rimborsi sportivi dilettantistici"),
    ("O", "Prestazioni non abituali senza iscrizione gestione separata"),
    ("O1", "Obblighi di fare/non fare senza iscrizione gestione separata"),
    ("P", "Compensi per uso di attrezzature e diritti (non residenti)"),
    ("Q", "Provvigioni agente monomandatario"),
    ("R", "Provvigioni agente plurimandatario"),
    ("S", "Provvigioni commissionario"),
    ("T", "Provvigioni mediatore"),
    ("U", "Provvigioni procacciatore d'affari"),
    ("V", "Provvigioni vendita a domicilio"),
    ("V1", "Redditi da attività commerciali non abituali"),
    ("W", "Corrispettivi appalti (art. 25-ter)"),
    ("X", "Canoni a società o stabili organizzazioni"),
    ("Y", "Canoni corrisposti a società (dal 2004)"),
    ("ZO", "Titolo diverso dai precedenti"),
];

pub const ESIGIBILITA_IVA: CodeTable = &[
    ("I", "Immediata"),
    ("D", "Differita"),
    ("S", "Scissione dei pagamenti"),
];

pub const TIPO_CESSIONE_PRESTAZIONE: CodeTable = &[
    ("SC", "Sconto"),
    ("PR", "Premio"),
    ("AB", "Abbuono"),
    ("AC", "Spesa accessoria"),
];

/// Codici destinatario speciali.
pub const CODICE_DESTINATARIO_PEC_O_CASSETTO: &str = "0000000";
pub const CODICE_DESTINATARIO_ESTERO: &str = "XXXXXXX";

pub const PROVINCE: &[&str] = &[
    "AG", "AL", "AN", "AO", "AP", "AQ", "AR", "AT", "AV", "BA", "BG", "BI", "BL", "BN", "BO", "BR",
    "BS", "BT", "BZ", "CA", "CB", "CE", "CH", "CL", "CN", "CO", "CR", "CS", "CT", "CZ", "EN", "FC",
    "FE", "FG", "FI", "FM", "FR", "GE", "GO", "GR", "IM", "IS", "KR", "LC", "LE", "LI", "LO", "LT",
    "LU", "MB", "MC", "ME", "MI", "MN", "MO", "MS", "MT", "NA", "NO", "NU", "OR", "PA", "PC", "PD",
    "PE", "PG", "PI", "PN", "PO", "PR", "PT", "PU", "PV", "PZ", "RA", "RC", "RE", "RG", "RI", "RM",
    "RN", "RO", "SA", "SI", "SO", "SP", "SR", "SS", "SU", "SV", "TA", "TE", "TN", "TO", "TP", "TR",
    "TS", "TV", "UD", "VA", "VB", "VC", "VE", "VI", "VR", "VT", "VV", "EE",
];

/// Paesi UE (codici ISO usati per la partita IVA comunitaria, Grecia = EL).
pub const PAESI_UE: &[&str] = &[
    "AT", "BE", "BG", "CY", "CZ", "DE", "DK", "EE", "EL", "ES", "FI", "FR", "HR", "HU", "IE", "IT",
    "LT", "LU", "LV", "MT", "NL", "PL", "PT", "RO", "SE", "SI", "SK",
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lookups() {
        assert_eq!(describe(TIPO_DOCUMENTO, "TD04"), "Nota di credito");
        assert!(is_valid(NATURA, "N2.2"));
        assert!(!is_valid(NATURA, "N2"));
        assert!(is_valid(REGIME_FISCALE, "RF20"));
        assert_eq!(MODALITA_PAGAMENTO.len(), 23);
    }
}
