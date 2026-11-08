//! Modello di dominio del gestionale.

use chrono::{NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

pub type Id = i64;

// ---------------------------------------------------------------------------
// Azienda
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Company {
    /// Ragione sociale (vuota per persona fisica, che usa nome e cognome).
    pub name: String,
    pub first_name: String,
    pub last_name: String,
    pub title: String,
    pub vat_country: String,
    pub vat_number: String,
    pub tax_code: String,
    pub regime_fiscale: String,
    pub address: String,
    pub civic: String,
    pub zip: String,
    pub city: String,
    pub province: String,
    pub country: String,
    pub phone: String,
    pub email: String,
    pub pec: String,
    pub website: String,
    pub rea_office: String,
    pub rea_number: String,
    pub share_capital: Option<Decimal>,
    /// "SU" socio unico, "SM" più soci, vuoto se non applicabile.
    pub sole_shareholder: String,
    /// true = in liquidazione
    pub in_liquidation: bool,
    pub albo: String,
    pub albo_province: String,
    pub albo_number: String,
    pub albo_date: Option<NaiveDate>,
    pub bank_name: String,
    pub iban: String,
    pub bic: String,
    /// Identificativo del trasmittente se diverso dall'azienda (es. commercialista).
    pub transmitter_country: String,
    pub transmitter_code: String,
    pub logo_path: String,
    pub invoice_footer: String,
}

impl Default for Company {
    fn default() -> Self {
        Company {
            name: String::new(),
            first_name: String::new(),
            last_name: String::new(),
            title: String::new(),
            vat_country: "IT".into(),
            vat_number: String::new(),
            tax_code: String::new(),
            regime_fiscale: "RF01".into(),
            address: String::new(),
            civic: String::new(),
            zip: String::new(),
            city: String::new(),
            province: String::new(),
            country: "IT".into(),
            phone: String::new(),
            email: String::new(),
            pec: String::new(),
            website: String::new(),
            rea_office: String::new(),
            rea_number: String::new(),
            share_capital: None,
            sole_shareholder: String::new(),
            in_liquidation: false,
            albo: String::new(),
            albo_province: String::new(),
            albo_number: String::new(),
            albo_date: None,
            bank_name: String::new(),
            iban: String::new(),
            bic: String::new(),
            transmitter_country: String::new(),
            transmitter_code: String::new(),
            logo_path: String::new(),
            invoice_footer: String::new(),
        }
    }
}

impl Company {
    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else {
            format!("{} {}", self.first_name.trim(), self.last_name.trim()).trim().to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// Soggetti (clienti e fornitori)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum PartyRole {
    #[default]
    Customer,
    Supplier,
    Both,
}

impl PartyRole {
    pub fn code(self) -> &'static str {
        match self {
            PartyRole::Customer => "C",
            PartyRole::Supplier => "F",
            PartyRole::Both => "CF",
        }
    }
    pub fn from_code(s: &str) -> Self {
        match s {
            "F" => PartyRole::Supplier,
            "CF" => PartyRole::Both,
            _ => PartyRole::Customer,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            PartyRole::Customer => "Cliente",
            PartyRole::Supplier => "Fornitore",
            PartyRole::Both => "Cliente e fornitore",
        }
    }
    pub fn is_customer(self) -> bool {
        matches!(self, PartyRole::Customer | PartyRole::Both)
    }
    pub fn is_supplier(self) -> bool {
        matches!(self, PartyRole::Supplier | PartyRole::Both)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Party {
    pub id: Id,
    pub code: String,
    pub role: PartyRole,
    /// Ragione sociale; se vuota si usano nome e cognome (persona fisica).
    pub name: String,
    pub first_name: String,
    pub last_name: String,
    pub vat_country: String,
    pub vat_number: String,
    pub tax_code: String,
    pub address: String,
    pub civic: String,
    pub zip: String,
    pub city: String,
    pub province: String,
    pub country: String,
    pub phone: String,
    pub mobile: String,
    pub email: String,
    pub pec: String,
    pub sdi_code: String,
    pub is_pa: bool,
    pub eori: String,
    pub default_payment_id: Option<Id>,
    pub default_vat_id: Option<Id>,
    pub default_discount: String,
    /// Listino prezzi assegnato
    pub price_list_id: Option<Id>,
    pub iban: String,
    /// Banca d'appoggio (descrizione, per le Ri.Ba.).
    pub bank_name: String,
    /// ABI e CAB della banca d'appoggio, se non si conosce l'IBAN.
    pub bank_abi: String,
    pub bank_cab: String,
    /// Mandato per gli addebiti diretti SEPA (SDD): riferimento e data di firma.
    pub sdd_mandate_id: String,
    pub sdd_mandate_date: Option<NaiveDate>,
    /// Schema SDD: "CORE" (consumatori e imprese) o "B2B" (solo imprese).
    pub sdd_scheme: String,
    /// Il primo addebito del mandato è già stato presentato (i successivi sono ricorrenti).
    pub sdd_first_done: bool,
    pub notes: String,
    pub active: bool,
    /// Agente che segue il cliente (provvigioni)
    pub agent_id: Option<Id>,
    /// Referenti (amministrazione, acquisti, ...)
    pub contacts: Vec<Contact>,
    /// Dichiarazioni d'intento ricevute dall'esportatore abituale
    pub intents: Vec<IntentDeclaration>,
    /// Carta fedeltà
    pub loyalty_card: String,
    pub version: i64,
}

/// Referente di un cliente o fornitore.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Contact {
    pub name: String,
    pub role: String,
    pub phone: String,
    pub email: String,
    /// Riceve i documenti via email
    pub gets_documents: bool,
}

/// Dichiarazione d'intento (art. 8 c.1 lett. c DPR 633/72) ricevuta da un esportatore abituale.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct IntentDeclaration {
    /// Protocollo di ricezione telematica assegnato dall'Agenzia (17 cifre + 6)
    pub protocol: String,
    /// Data della ricevuta
    pub date: Option<NaiveDate>,
    pub year: i32,
    /// Importo massimo indicato; zero = singola operazione / nessun limite indicato
    pub limit: Decimal,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub note: String,
}

impl Default for Party {
    fn default() -> Self {
        Party {
            id: 0,
            code: String::new(),
            role: PartyRole::Customer,
            name: String::new(),
            first_name: String::new(),
            last_name: String::new(),
            vat_country: "IT".into(),
            vat_number: String::new(),
            tax_code: String::new(),
            address: String::new(),
            civic: String::new(),
            zip: String::new(),
            city: String::new(),
            province: String::new(),
            country: "IT".into(),
            phone: String::new(),
            mobile: String::new(),
            email: String::new(),
            pec: String::new(),
            sdi_code: String::new(),
            is_pa: false,
            eori: String::new(),
            default_payment_id: None,
            default_vat_id: None,
            default_discount: String::new(),
            price_list_id: None,
            iban: String::new(),
            bank_name: String::new(),
            bank_abi: String::new(),
            bank_cab: String::new(),
            sdd_mandate_id: String::new(),
            sdd_mandate_date: None,
            sdd_scheme: String::new(),
            sdd_first_done: false,
            notes: String::new(),
            active: true,
            agent_id: None,
            contacts: Vec::new(),
            intents: Vec::new(),
            loyalty_card: String::new(),
            version: 0,
        }
    }
}

impl Party {
    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else {
            format!("{} {}", self.first_name.trim(), self.last_name.trim()).trim().to_string()
        }
    }
    pub fn full_address(&self) -> String {
        let mut s = self.address.trim().to_string();
        if !self.civic.trim().is_empty() {
            s.push_str(", ");
            s.push_str(self.civic.trim());
        }
        s
    }
    pub fn city_line(&self) -> String {
        let mut s = format!("{} {}", self.zip.trim(), self.city.trim()).trim().to_string();
        if !self.province.trim().is_empty() {
            s.push_str(&format!(" ({})", self.province.trim()));
        }
        if !self.country.is_empty() && self.country != "IT" {
            s.push_str(&format!(" - {}", self.country));
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Tabelle: aliquote IVA e pagamenti
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct VatCode {
    pub id: Id,
    pub code: String,
    pub description: String,
    pub rate: Decimal,
    /// Natura dell'operazione (obbligatoria se l'aliquota è zero).
    pub natura: Option<String>,
    /// Riferimento normativo da riportare in fattura (es. "Art. 10 DPR 633/72").
    pub legal_ref: String,
    pub active: bool,
    /// Scissione dei pagamenti (art. 17-ter DPR 633/72): l'IVA la versa la Pubblica Amministrazione.
    pub split_payment: bool,
}

impl Product {
    /// Codice e prezzo dell'articolo presso un fornitore (principale o aggiuntivo).
    pub fn supplier_terms(&self, supplier: Id) -> Option<(String, Decimal)> {
        if self.supplier_id == Some(supplier) {
            return Some((self.supplier_code.clone(), self.supplier_price));
        }
        self.other_suppliers.iter().find(|s| s.supplier_id == supplier).map(|s| (s.code.clone(), s.price))
    }
}

/// Fornitore aggiuntivo di un articolo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ProductSupplier {
    pub supplier_id: Id,
    pub code: String,
    pub price: Decimal,
    pub notes: String,
}

/// Riferimento all'aliquota memorizzato nella riga (storicizzato).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(default)]
pub struct VatRef {
    pub vat_id: Option<Id>,
    pub rate: Decimal,
    pub natura: Option<String>,
    pub legal_ref: String,
    /// Aliquota con scissione dei pagamenti ("22sp")
    pub split: bool,
}

impl VatRef {
    pub fn from_code(v: &VatCode) -> Self {
        VatRef { vat_id: Some(v.id), rate: v.rate, natura: v.natura.clone(), legal_ref: v.legal_ref.clone(), split: v.split_payment }
    }
    pub fn label(&self) -> String {
        match &self.natura {
            Some(n) if self.rate.is_zero() => n.clone(),
            _ if self.split => format!("{}sp", crate::money::fmt_it_trim(self.rate, 0, 2)),
            _ => format!("{}%", crate::money::fmt_it_trim(self.rate, 0, 2)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Installment {
    /// Giorni dalla data documento.
    pub days: u32,
    /// Scadenza a fine mese.
    pub end_of_month: bool,
    /// Giorni aggiuntivi dopo il fine mese (es. "fine mese + 10").
    pub extra_days: u32,
    /// Percentuale del netto a pagare (le rate devono sommare a 100).
    pub percent: Decimal,
}

impl Default for Installment {
    fn default() -> Self {
        Installment { days: 0, end_of_month: false, extra_days: 0, percent: Decimal::ONE_HUNDRED }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PaymentMethod {
    pub id: Id,
    pub name: String,
    /// Modalità FatturaPA (MP01...MP23).
    pub mp_code: String,
    pub installments: Vec<Installment>,
    /// Mostra l'IBAN aziendale in fattura.
    pub show_iban: bool,
    pub active: bool,
}

// ---------------------------------------------------------------------------
// Articoli
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Product {
    pub id: Id,
    pub code: String,
    pub description: String,
    pub unit: String,
    pub category: String,
    /// Prezzo di vendita al netto dell'IVA.
    pub price: Decimal,
    /// Prezzi dei listini aggiuntivi (listino 2, 3...).
    pub extra_prices: Vec<Decimal>,
    pub vat_id: Option<Id>,
    pub cost: Decimal,
    pub barcode: String,
    pub is_service: bool,
    pub manage_stock: bool,
    pub min_stock: Decimal,
    /// Fornitore principale, con il suo codice, prezzo e note
    pub supplier_id: Option<Id>,
    pub supplier_code: String,
    pub supplier_price: Decimal,
    pub supplier_notes: String,
    /// Fornitori aggiuntivi, ciascuno col proprio codice e prezzo
    pub other_suppliers: Vec<ProductSupplier>,
    /// Caratteristiche
    pub producer: String,
    pub url: String,
    /// Dimensioni: unità, netto e imballo (larghezza, altezza, profondità)
    pub dim_unit: String,
    pub dim_net: [Decimal; 3],
    pub dim_pack: [Decimal; 3],
    /// Peso: unità, netto e lordo
    pub weight_unit: String,
    pub weight_net: Decimal,
    pub weight_gross: Decimal,
    /// Giorni per l'ordine al fornitore e multipli d'ordine
    pub order_days: u32,
    pub order_multiple: Decimal,
    pub notes: String,
    pub active: bool,
    /// Varianti (es. taglia e colore); vuoto = articolo senza varianti.
    pub variants: VariantSpec,
    /// Tracciabilità: nessuna, lotti con scadenza, numeri di serie.
    pub tracking: Tracking,
    /// Articolo semplice, gruppo (kit) o assemblato con distinta base
    pub kind: ProductKind,
    /// Componenti del gruppo o della distinta base
    pub components: Vec<Component>,
    /// Pezzi per confezione (zero = venduto a pezzi)
    pub pack_size: Decimal,
    /// Unità della confezione (es. "CF", "CT")
    pub pack_unit: String,
    /// Contributo ambientale per unità (RAEE, pile, pneumatici...)
    pub eco_fee: Decimal,
    pub eco_label: String,
    /// Provvigione specifica per gli agenti (sostituisce quella dell'agente)
    pub commission: Option<Decimal>,
    /// Pubblicato sul negozio online
    pub online: bool,
    pub version: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ProductKind {
    #[default]
    Simple,
    /// Gruppo di articoli: inserito in un documento si espande nei componenti
    Kit,
    /// Prodotto finito con distinta base: si carica con la produzione scaricando i componenti
    Assembled,
}

impl ProductKind {
    pub fn label(self) -> &'static str {
        match self {
            ProductKind::Simple => "Articolo semplice",
            ProductKind::Kit => "Gruppo di articoli (kit)",
            ProductKind::Assembled => "Prodotto con distinta base",
        }
    }
}

/// Riga di distinta base o di gruppo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Component {
    pub product_id: Id,
    pub qty: Decimal,
    pub note: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Tracking {
    #[default]
    None,
    /// Lotti con eventuale data di scadenza
    Lot,
    /// Un numero di serie per ogni unità
    Serial,
}

impl Tracking {
    pub fn label(self) -> &'static str {
        match self {
            Tracking::None => "Nessuna",
            Tracking::Lot => "Lotti e scadenze",
            Tracking::Serial => "Numeri di serie",
        }
    }
}

/// Codice a barre specifico di una variante.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct VariantBarcode {
    pub variant: String,
    pub barcode: String,
}

/// Varianti su due assi (es. Taglia: S, M, L; Colore: Rosso, Blu).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct VariantSpec {
    pub axis1: String,
    pub values1: Vec<String>,
    pub axis2: String,
    pub values2: Vec<String>,
    pub barcodes: Vec<VariantBarcode>,
}

impl VariantSpec {
    pub fn is_empty(&self) -> bool {
        self.values1.is_empty() && self.values2.is_empty()
    }
    /// Tutte le combinazioni, nel formato "M/Rosso" (o solo "M" con un asse).
    pub fn combinations(&self) -> Vec<String> {
        let a: Vec<&String> = self.values1.iter().filter(|v| !v.trim().is_empty()).collect();
        let b: Vec<&String> = self.values2.iter().filter(|v| !v.trim().is_empty()).collect();
        match (a.is_empty(), b.is_empty()) {
            (true, true) => vec![],
            (false, true) => a.iter().map(|x| x.trim().to_string()).collect(),
            (true, false) => b.iter().map(|x| x.trim().to_string()).collect(),
            (false, false) => a.iter().flat_map(|x| b.iter().map(move |y| format!("{}/{}", x.trim(), y.trim()))).collect(),
        }
    }
    pub fn contains(&self, v: &str) -> bool {
        self.combinations().iter().any(|c| c == v)
    }
    pub fn barcode_for(&self, variant: &str) -> Option<&str> {
        self.barcodes.iter().find(|b| b.variant == variant && !b.barcode.is_empty()).map(|b| b.barcode.as_str())
    }
}

/// Cifra di controllo EAN-13 per le prime 12 cifre.
pub fn ean13_check_digit(first12: &str) -> Option<u32> {
    if first12.len() != 12 || !first12.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let sum: u32 = first12.bytes().enumerate().map(|(i, b)| (b - b'0') as u32 * if i % 2 == 0 { 1 } else { 3 }).sum();
    Some((10 - sum % 10) % 10)
}

/// Verifica un codice EAN-13 completo.
pub fn ean13_valid(code: &str) -> bool {
    code.len() == 13 && ean13_check_digit(&code[..12]).map(|c| code.as_bytes()[12] == b'0' + c as u8).unwrap_or(false)
}

impl Default for Product {
    fn default() -> Self {
        Product {
            id: 0,
            code: String::new(),
            description: String::new(),
            unit: "pz".into(),
            category: String::new(),
            price: Decimal::ZERO,
            extra_prices: Vec::new(),
            vat_id: None,
            cost: Decimal::ZERO,
            barcode: String::new(),
            is_service: false,
            manage_stock: true,
            min_stock: Decimal::ZERO,
            supplier_id: None,
            supplier_code: String::new(),
            supplier_price: Decimal::ZERO,
            supplier_notes: String::new(),
            other_suppliers: Vec::new(),
            producer: String::new(),
            url: String::new(),
            dim_unit: String::new(),
            dim_net: [Decimal::ZERO; 3],
            dim_pack: [Decimal::ZERO; 3],
            weight_unit: String::new(),
            weight_net: Decimal::ZERO,
            weight_gross: Decimal::ZERO,
            order_days: 0,
            order_multiple: Decimal::ZERO,
            notes: String::new(),
            active: true,
            variants: VariantSpec::default(),
            tracking: Tracking::None,
            kind: ProductKind::Simple,
            components: Vec::new(),
            pack_size: Decimal::ZERO,
            pack_unit: String::new(),
            eco_fee: Decimal::ZERO,
            eco_label: String::new(),
            commission: None,
            online: false,
            version: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Documenti
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum DocKind {
    /// Preventivo
    Quote,
    /// Ordine cliente
    CustomerOrder,
    /// Documento di trasporto
    DeliveryNote,
    /// Fattura immediata
    #[default]
    Invoice,
    /// Fattura accompagnatoria
    AccompanyingInvoice,
    /// Nota di credito
    CreditNote,
    /// Nota di debito
    DebitNote,
    /// Fattura d'acconto
    DownPaymentInvoice,
    /// Parcella
    Fee,
    /// Fattura proforma
    Proforma,
    /// Autofattura / integrazione (TD16-TD29)
    SelfInvoice,
    /// Preventivo ricevuto da un fornitore
    SupplierQuote,
    /// Ordine a fornitore
    SupplierOrder,
    /// Arrivo merce (DDT del fornitore)
    GoodsReceipt,
    /// Fattura di acquisto
    PurchaseInvoice,
    /// Nota di credito del fornitore
    PurchaseCreditNote,
    /// Vendita al banco (documento commerciale emesso dal registratore telematico)
    RetailSale,
    /// Trasferimento di merce tra due magazzini dell'azienda
    StockTransfer,
    /// Produzione: carica i prodotti finiti e scarica i componenti della distinta base
    Production,
}

impl DocKind {
    pub const ALL: [DocKind; 19] = [
        DocKind::Quote,
        DocKind::CustomerOrder,
        DocKind::DeliveryNote,
        DocKind::Invoice,
        DocKind::AccompanyingInvoice,
        DocKind::CreditNote,
        DocKind::DebitNote,
        DocKind::DownPaymentInvoice,
        DocKind::Fee,
        DocKind::Proforma,
        DocKind::SelfInvoice,
        DocKind::SupplierQuote,
        DocKind::SupplierOrder,
        DocKind::GoodsReceipt,
        DocKind::PurchaseInvoice,
        DocKind::PurchaseCreditNote,
        DocKind::RetailSale,
        DocKind::StockTransfer,
        DocKind::Production,
    ];

    pub fn code(self) -> &'static str {
        match self {
            DocKind::Quote => "PRV",
            DocKind::CustomerOrder => "ORC",
            DocKind::DeliveryNote => "DDT",
            DocKind::Invoice => "FAT",
            DocKind::AccompanyingInvoice => "FAC",
            DocKind::CreditNote => "NDC",
            DocKind::DebitNote => "NDD",
            DocKind::DownPaymentInvoice => "FAA",
            DocKind::Fee => "PAR",
            DocKind::Proforma => "PRO",
            DocKind::SelfInvoice => "AUT",
            DocKind::SupplierQuote => "PRF",
            DocKind::SupplierOrder => "ORF",
            DocKind::GoodsReceipt => "ARR",
            DocKind::PurchaseInvoice => "FAQ",
            DocKind::PurchaseCreditNote => "NCF",
            DocKind::RetailSale => "BAN",
            DocKind::StockTransfer => "TRS",
            DocKind::Production => "PRD",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        DocKind::ALL.iter().copied().find(|k| k.code() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            DocKind::Quote => "Preventivo",
            DocKind::CustomerOrder => "Ordine cliente",
            DocKind::DeliveryNote => "DDT",
            DocKind::Invoice => "Fattura",
            DocKind::AccompanyingInvoice => "Fattura accompagnatoria",
            DocKind::CreditNote => "Nota di credito",
            DocKind::DebitNote => "Nota di debito",
            DocKind::DownPaymentInvoice => "Fattura d'acconto",
            DocKind::Fee => "Parcella",
            DocKind::Proforma => "Fattura proforma",
            DocKind::SelfInvoice => "Autofattura / integrazione",
            DocKind::SupplierQuote => "Preventivo fornitore",
            DocKind::SupplierOrder => "Ordine fornitore",
            DocKind::GoodsReceipt => "Arrivo merce",
            DocKind::PurchaseInvoice => "Fattura di acquisto",
            DocKind::PurchaseCreditNote => "Nota di credito fornitore",
            DocKind::RetailSale => "Vendita al banco",
            DocKind::StockTransfer => "Trasferimento tra magazzini",
            DocKind::Production => "Produzione",
        }
    }

    pub fn plural(self) -> &'static str {
        match self {
            DocKind::Quote => "Preventivi",
            DocKind::CustomerOrder => "Ordini clienti",
            DocKind::DeliveryNote => "DDT",
            DocKind::Invoice => "Fatture",
            DocKind::AccompanyingInvoice => "Fatture accompagnatorie",
            DocKind::CreditNote => "Note di credito",
            DocKind::DebitNote => "Note di debito",
            DocKind::DownPaymentInvoice => "Fatture d'acconto",
            DocKind::Fee => "Parcelle",
            DocKind::Proforma => "Proforma",
            DocKind::SelfInvoice => "Autofatture",
            DocKind::SupplierQuote => "Preventivi fornitori",
            DocKind::SupplierOrder => "Ordini fornitori",
            DocKind::GoodsReceipt => "Arrivi merce",
            DocKind::PurchaseInvoice => "Fatture di acquisto",
            DocKind::PurchaseCreditNote => "Note di credito fornitori",
            DocKind::RetailSale => "Vendite al banco",
            DocKind::StockTransfer => "Trasferimenti tra magazzini",
            DocKind::Production => "Produzioni",
        }
    }

    pub fn is_purchase(self) -> bool {
        matches!(
            self,
            DocKind::SupplierQuote | DocKind::SupplierOrder | DocKind::GoodsReceipt | DocKind::PurchaseInvoice | DocKind::PurchaseCreditNote
        )
    }

    pub fn is_sale(self) -> bool {
        !self.is_purchase() && !self.is_internal()
    }

    /// Documenti interni di magazzino (nessun cliente o fornitore).
    pub fn is_internal(self) -> bool {
        matches!(self, DocKind::StockTransfer | DocKind::Production)
    }

    /// Effetto sul magazzino: -1 scarico, +1 carico, 0 nessuno.
    pub fn stock_effect(self) -> i32 {
        match self {
            DocKind::DeliveryNote | DocKind::Invoice | DocKind::AccompanyingInvoice | DocKind::RetailSale => -1,
            DocKind::CreditNote => 1,
            DocKind::GoodsReceipt | DocKind::PurchaseInvoice => 1,
            DocKind::PurchaseCreditNote => -1,
            DocKind::Production => 1,
            _ => 0,
        }
    }

    /// Tipo documento FatturaPA predefinito, se il documento va trasmesso allo SdI.
    pub fn default_fe_type(self) -> Option<&'static str> {
        match self {
            DocKind::Invoice | DocKind::AccompanyingInvoice => Some("TD01"),
            DocKind::DownPaymentInvoice => Some("TD02"),
            DocKind::CreditNote => Some("TD04"),
            DocKind::DebitNote => Some("TD05"),
            DocKind::Fee => Some("TD06"),
            DocKind::SelfInvoice => Some("TD17"),
            _ => None,
        }
    }

    /// Ordini: hanno uno stato (da confermare, confermato, concluso) e una data prevista.
    pub fn has_order_state(self) -> bool {
        matches!(self, DocKind::CustomerOrder | DocKind::SupplierOrder)
    }

    /// Documento fiscale che va numerato in modo progressivo e trasmesso.
    pub fn is_fiscal(self) -> bool {
        self.default_fe_type().is_some()
    }

    /// Ha i dati di trasporto (DDT e fattura accompagnatoria).
    pub fn has_transport(self) -> bool {
        matches!(self, DocKind::DeliveryNote | DocKind::AccompanyingInvoice | DocKind::GoodsReceipt)
    }

    /// Genera scadenze di pagamento.
    pub fn has_due_dates(self) -> bool {
        matches!(
            self,
            DocKind::Invoice
                | DocKind::AccompanyingInvoice
                | DocKind::CreditNote
                | DocKind::DebitNote
                | DocKind::DownPaymentInvoice
                | DocKind::Fee
                | DocKind::PurchaseInvoice
                | DocKind::PurchaseCreditNote
        )
    }

    /// Le note di credito invertono il segno degli incassi.
    pub fn is_credit(self) -> bool {
        matches!(self, DocKind::CreditNote | DocKind::PurchaseCreditNote)
    }

    /// Registro di numerazione: documenti che condividono la stessa sequenza.
    /// Fatture, accompagnatorie, note e parcelle condividono il registro IVA vendite.
    pub fn numbering_register(self) -> &'static str {
        match self {
            DocKind::Invoice
            | DocKind::AccompanyingInvoice
            | DocKind::CreditNote
            | DocKind::DebitNote
            | DocKind::DownPaymentInvoice
            | DocKind::Fee => "VEN",
            other => other.code(),
        }
    }

    /// Documenti che si possono generare da questo (trasformazione / evasione).
    pub fn transform_targets(self) -> &'static [DocKind] {
        match self {
            DocKind::Quote => &[DocKind::CustomerOrder, DocKind::DeliveryNote, DocKind::Invoice, DocKind::AccompanyingInvoice, DocKind::Proforma],
            DocKind::CustomerOrder => &[DocKind::DeliveryNote, DocKind::Invoice, DocKind::AccompanyingInvoice, DocKind::Proforma],
            DocKind::DeliveryNote => &[DocKind::Invoice],
            DocKind::Proforma => &[DocKind::Invoice],
            DocKind::Invoice | DocKind::AccompanyingInvoice => &[DocKind::CreditNote],
            DocKind::SupplierQuote => &[DocKind::SupplierOrder, DocKind::GoodsReceipt, DocKind::PurchaseInvoice],
            DocKind::SupplierOrder => &[DocKind::GoodsReceipt, DocKind::PurchaseInvoice],
            DocKind::GoodsReceipt => &[DocKind::PurchaseInvoice],
            DocKind::PurchaseInvoice => &[DocKind::SelfInvoice, DocKind::PurchaseCreditNote],
            _ => &[],
        }
    }
}

/// Stato di un ordine. "In parte concluso" e "Concluso" si ricavano anche da soli dalle
/// righe evase; si possono però impostare a mano (ordine chiuso senza altri documenti).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum OrderStatus {
    #[default]
    ToConfirm,
    Confirmed,
    Partial,
    Concluded,
}

impl OrderStatus {
    pub const ALL: [OrderStatus; 4] = [OrderStatus::ToConfirm, OrderStatus::Confirmed, OrderStatus::Partial, OrderStatus::Concluded];

    pub fn label(self) -> &'static str {
        match self {
            OrderStatus::ToConfirm => "Da confermare",
            OrderStatus::Confirmed => "Confermato",
            OrderStatus::Partial => "Parz. concluso",
            OrderStatus::Concluded => "Concluso",
        }
    }

    pub fn from_name(s: &str) -> Self {
        match s {
            "Confirmed" => OrderStatus::Confirmed,
            "Partial" => OrderStatus::Partial,
            "Concluded" => OrderStatus::Concluded,
            _ => OrderStatus::ToConfirm,
        }
    }

    /// Stato effettivo, tenendo conto delle righe già evase.
    pub fn effective(self, any_fulfilled: bool, all_fulfilled: bool) -> Self {
        if all_fulfilled {
            OrderStatus::Concluded
        } else if any_fulfilled && self != OrderStatus::Concluded {
            OrderStatus::Partial
        } else {
            self
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum DocStatus {
    #[default]
    Draft,
    Issued,
    Cancelled,
}

impl DocStatus {
    pub fn code(self) -> &'static str {
        match self {
            DocStatus::Draft => "BOZZA",
            DocStatus::Issued => "EMESSO",
            DocStatus::Cancelled => "ANNULLATO",
        }
    }
    pub fn from_code(s: &str) -> Self {
        match s {
            "EMESSO" => DocStatus::Issued,
            "ANNULLATO" => DocStatus::Cancelled,
            _ => DocStatus::Draft,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            DocStatus::Draft => "Bozza",
            DocStatus::Issued => "Emesso",
            DocStatus::Cancelled => "Annullato",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum LineKind {
    #[default]
    Item,
    /// Riga solo descrittiva (nessun importo).
    Text,
    /// Subtotale delle righe precedenti (dall'ultimo subtotale)
    Subtotal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DocLine {
    pub id: Id,
    pub kind: LineKind,
    pub product_id: Option<Id>,
    pub code: String,
    pub description: String,
    pub qty: Decimal,
    pub unit: String,
    /// Prezzo unitario: netto IVA, oppure IVA inclusa se il documento ha `prices_include_vat`.
    pub unit_price: Decimal,
    /// Sconti in cascata: "10+5+2,5" (percentuali).
    pub discounts: String,
    pub vat: VatRef,
    /// Soggetta a ritenuta d'acconto.
    pub withholding: bool,
    /// Soggetta a contributo cassa previdenziale.
    pub pension_fund: bool,
    /// Tipo cessione/prestazione FatturaPA (SC, PR, AB, AC) se rilevante.
    pub supply_type: String,
    pub period_start: Option<NaiveDate>,
    pub period_end: Option<NaiveDate>,
    /// Riga d'origine (trasformazione/evasione).
    pub source_line_id: Option<Id>,
    pub source_doc_id: Option<Id>,
    /// Costo unitario (per margini e valorizzazione magazzino).
    pub unit_cost: Decimal,
    /// Lotto (in uscita vuoto = assegnazione automatica per scadenza)
    pub lot: String,
    /// Scadenza del lotto (nei carichi)
    pub lot_expiry: Option<NaiveDate>,
    /// Variante (es. "M/Rosso")
    pub variant: String,
    /// Numeri di serie, uno per unità
    pub serials: Vec<String>,
    /// Provenienza del prezzo (listino, condizione particolare, manuale); non stampata.
    pub price_note: String,
    /// Numero di confezioni (la quantità è confezioni x pezzi per confezione)
    pub packs: Option<Decimal>,
    /// Riga calcolata: importo pari a questa percentuale delle righe precedenti
    pub calc_percent: Option<Decimal>,
    /// Contributo ambientale per unità, addebitato in una riga dedicata
    pub eco_fee: Decimal,
    pub eco_label: String,
    /// Provvigione dell'agente su questa riga (sostituisce quella del documento)
    pub commission: Option<Decimal>,
}

impl DocLine {
    /// Variante, lotto e numeri di serie in forma leggibile (per stampe e fattura elettronica).
    pub fn tracking_note(&self) -> String {
        let mut parts = Vec::new();
        if !self.variant.trim().is_empty() {
            parts.push(format!("Var. {}", self.variant.trim()));
        }
        if !self.lot.trim().is_empty() {
            let mut s = format!("Lotto {}", self.lot.trim());
            if let Some(e) = self.lot_expiry {
                s.push_str(&format!(" scad. {}", e.format("%d/%m/%Y")));
            }
            parts.push(s);
        }
        if !self.serials.is_empty() {
            parts.push(format!("S/N {}", self.serials.join(", ")));
        }
        parts.join(" - ")
    }

    pub fn description_with_tracking(&self) -> String {
        let note = self.tracking_note();
        if note.is_empty() {
            self.description.clone()
        } else {
            format!("{} ({})", self.description.trim(), note)
        }
    }
}

impl Default for DocLine {
    fn default() -> Self {
        DocLine {
            id: 0,
            kind: LineKind::Item,
            product_id: None,
            code: String::new(),
            description: String::new(),
            qty: Decimal::ONE,
            unit: String::new(),
            unit_price: Decimal::ZERO,
            discounts: String::new(),
            vat: VatRef::default(),
            withholding: false,
            pension_fund: false,
            supply_type: String::new(),
            period_start: None,
            period_end: None,
            source_line_id: None,
            source_doc_id: None,
            unit_cost: Decimal::ZERO,
            lot: String::new(),
            lot_expiry: None,
            variant: String::new(),
            serials: Vec::new(),
            price_note: String::new(),
            packs: None,
            calc_percent: None,
            eco_fee: Decimal::ZERO,
            eco_label: String::new(),
            commission: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Withholding {
    /// RT01...RT06
    pub kind: String,
    /// Aliquota della ritenuta (es. 20).
    pub rate: Decimal,
    /// Percentuale dell'imponibile assoggettata (es. 100, oppure 50 per agenti).
    pub base_percent: Decimal,
    /// Causale di pagamento CU (A, M, ...).
    pub payment_reason: String,
}

impl Default for Withholding {
    fn default() -> Self {
        Withholding {
            kind: "RT01".into(),
            rate: Decimal::from(20),
            base_percent: Decimal::ONE_HUNDRED,
            payment_reason: "A".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PensionFund {
    /// TC01...TC22
    pub kind: String,
    /// Aliquota del contributo (es. 4).
    pub rate: Decimal,
    /// Aliquota IVA applicata al contributo.
    pub vat: VatRef,
    /// Il contributo è soggetto a ritenuta (es. rivalsa INPS).
    pub subject_to_withholding: bool,
}

impl Default for PensionFund {
    fn default() -> Self {
        PensionFund {
            kind: "TC22".into(),
            rate: Decimal::from(4),
            vat: VatRef { vat_id: None, rate: Decimal::from(22), natura: None, legal_ref: String::new(), split: false },
            subject_to_withholding: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum StampMode {
    /// Applicata automaticamente se gli importi senza IVA superano 77,47 euro.
    #[default]
    Auto,
    Yes,
    No,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Stamp {
    pub mode: StampMode,
    pub amount: Decimal,
    /// Addebita il bollo al cliente con una riga dedicata.
    pub charge_to_customer: bool,
}

impl Default for Stamp {
    fn default() -> Self {
        Stamp { mode: StampMode::Auto, amount: Decimal::new(200, 2), charge_to_customer: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Transport {
    pub reason: String,
    pub carrier: String,
    pub carrier_vat: String,
    pub by: String,
    pub packages: Option<u32>,
    pub appearance: String,
    pub gross_weight: Option<Decimal>,
    pub net_weight: Option<Decimal>,
    pub port: String,
    pub start: Option<NaiveDateTime>,
    pub destination: String,
}

/// Riferimento a documento correlato (ordine d'acquisto, contratto, fattura collegata).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct RelatedDoc {
    /// "ORDINE", "CONTRATTO", "CONVENZIONE", "RICEZIONE", "FATTURA"
    pub kind: String,
    pub number: String,
    pub date: Option<NaiveDate>,
    pub cig: String,
    pub cup: String,
    pub commessa: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DueDate {
    pub id: Id,
    pub date: NaiveDate,
    pub amount: Decimal,
    pub mp_code: String,
    pub paid: Decimal,
    /// Abbuono: differenza chiusa senza incasso.
    pub writeoff: Decimal,
    pub paid_date: Option<NaiveDate>,
}

impl Default for DueDate {
    fn default() -> Self {
        DueDate {
            id: 0,
            date: NaiveDate::from_ymd_opt(2000, 1, 1).unwrap(),
            amount: Decimal::ZERO,
            mp_code: "MP05".into(),
            paid: Decimal::ZERO,
            writeoff: Decimal::ZERO,
            paid_date: None,
        }
    }
}

impl DueDate {
    pub fn residual(&self) -> Decimal {
        self.amount - self.paid - self.writeoff
    }
    pub fn is_paid(&self) -> bool {
        self.residual() <= Decimal::ZERO
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum RetailPayMethod {
    #[default]
    Cash,
    /// Carta, bancomat, app: pagamento elettronico
    Electronic,
    /// Buoni pasto / ticket
    Ticket,
    /// Corrispettivo non riscosso
    NotCollected,
}

impl RetailPayMethod {
    pub fn label(self) -> &'static str {
        match self {
            RetailPayMethod::Cash => "Contanti",
            RetailPayMethod::Electronic => "Pagamento elettronico",
            RetailPayMethod::Ticket => "Buoni / ticket",
            RetailPayMethod::NotCollected => "Non riscosso",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct RetailPayment {
    pub method: RetailPayMethod,
    pub amount: Decimal,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum RtStatus {
    /// Vendita registrata, documento commerciale non ancora emesso dal registratore
    #[default]
    Pending,
    /// Documento commerciale emesso dal registratore telematico
    Printed,
    /// Emesso manualmente sul registratore, estremi inseriti dall'operatore
    Manual,
    /// Simulazione (nessun valore fiscale)
    Simulated,
    /// Il registratore ha restituito un errore
    Failed,
}

impl RtStatus {
    pub fn label(self) -> &'static str {
        match self {
            RtStatus::Pending => "Da emettere sul registratore",
            RtStatus::Printed => "Documento commerciale emesso",
            RtStatus::Manual => "Emesso manualmente",
            RtStatus::Simulated => "Simulazione (non fiscale)",
            RtStatus::Failed => "Errore del registratore",
        }
    }
}

/// Estremi di un documento commerciale (per i resi e gli annulli).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct RetailRef {
    pub rt_serial: String,
    pub z_number: String,
    pub receipt_number: String,
    pub date: Option<NaiveDate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct RetailInfo {
    pub payments: Vec<RetailPayment>,
    /// Contanti ricevuti dal cliente
    pub cash_given: Decimal,
    pub change: Decimal,
    /// Codice fiscale/P.IVA del cliente (documento commerciale "parlante")
    pub customer_tax_code: String,
    pub lottery_code: String,
    pub rt_status: RtStatus,
    pub rt_serial: String,
    pub z_number: String,
    pub receipt_number: String,
    pub rt_message: String,
    /// Vendita di reso: estremi del documento commerciale originale
    pub refund_of: Option<RetailRef>,
}

impl RetailInfo {
    pub fn paid(&self) -> Decimal {
        self.payments.iter().map(|p| p.amount).sum()
    }
    pub fn reference(&self) -> String {
        if self.receipt_number.is_empty() {
            String::new()
        } else {
            format!("{}-{}", self.z_number, self.receipt_number)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Document {
    pub id: Id,
    pub kind: DocKind,
    pub status: DocStatus,
    pub series: String,
    pub year: i32,
    /// Numero progressivo, assegnato all'emissione.
    pub number: Option<i64>,
    pub date: NaiveDate,
    /// Numero del documento del fornitore (documenti di acquisto).
    pub external_number: String,
    pub party_id: Option<Id>,
    /// Copia dei dati del soggetto al momento dell'emissione.
    pub party: Party,
    pub destination: String,
    pub payment_id: Option<Id>,
    pub payment: Option<PaymentMethod>,
    pub prices_include_vat: bool,
    pub lines: Vec<DocLine>,
    /// Sconto percentuale sul documento (in cascata, es. "5" o "3+2").
    pub discount: String,
    pub pension_fund: Option<PensionFund>,
    pub withholdings: Vec<Withholding>,
    pub stamp: Stamp,
    /// I = immediata, D = differita, S = scissione dei pagamenti
    pub vat_liability: String,
    /// Tipo FatturaPA scelto manualmente (TD24, TD17...).
    pub fe_type: String,
    pub causale: String,
    pub notes: String,
    pub internal_notes: String,
    pub related: Vec<RelatedDoc>,
    pub transport: Option<Transport>,
    /// Dati della vendita al banco
    pub retail: Option<RetailInfo>,
    /// Magazzino movimentato (vuoto = magazzino predefinito); origine nei trasferimenti
    pub warehouse: String,
    /// Magazzino di destinazione (solo trasferimenti)
    pub to_warehouse: String,
    pub due_dates: Vec<DueDate>,
    /// Valuta del documento (vuoto = euro) e cambio: unità di valuta per 1 euro
    pub currency: String,
    pub exchange_rate: Decimal,
    /// Agente e provvigione percentuale del documento
    pub agent_id: Option<Id>,
    pub commission_rate: Option<Decimal>,
    /// Dichiarazione d'intento a cui si riferisce la fattura (protocollo)
    pub intent_protocol: String,
    /// Ordine del negozio online da cui nasce il documento
    pub shop_order: String,
    /// Documento importato da un altro programma, che ne ha già gestito la fattura elettronica
    pub legacy_fe: bool,
    /// Listino scelto nel documento: vuoto = quello del cliente, 0 = prezzo base
    pub price_list_id: Option<Id>,
    /// Ordini: stato e data prevista di conclusione
    pub order_status: OrderStatus,
    pub expected_date: Option<NaiveDate>,
    pub created_at: Option<NaiveDateTime>,
    pub updated_at: Option<NaiveDateTime>,
    pub version: i64,
}

impl Default for Document {
    fn default() -> Self {
        let today = chrono::Local::now().date_naive();
        Document {
            id: 0,
            kind: DocKind::Invoice,
            status: DocStatus::Draft,
            series: String::new(),
            year: chrono::Datelike::year(&today),
            number: None,
            date: today,
            external_number: String::new(),
            party_id: None,
            party: Party::default(),
            destination: String::new(),
            payment_id: None,
            payment: None,
            prices_include_vat: false,
            lines: Vec::new(),
            discount: String::new(),
            pension_fund: None,
            withholdings: Vec::new(),
            stamp: Stamp::default(),
            vat_liability: "I".into(),
            fe_type: String::new(),
            causale: String::new(),
            notes: String::new(),
            internal_notes: String::new(),
            related: Vec::new(),
            transport: None,
            retail: None,
            warehouse: String::new(),
            to_warehouse: String::new(),
            due_dates: Vec::new(),
            currency: String::new(),
            exchange_rate: Decimal::ONE,
            agent_id: None,
            commission_rate: None,
            intent_protocol: String::new(),
            shop_order: String::new(),
            legacy_fe: false,
            price_list_id: None,
            order_status: OrderStatus::ToConfirm,
            expected_date: None,
            created_at: None,
            updated_at: None,
            version: 0,
        }
    }
}

impl Document {
    pub fn new(kind: DocKind) -> Self {
        let mut d = Document { kind, ..Default::default() };
        if kind.has_transport() {
            d.transport = Some(Transport::default());
        }
        if kind == DocKind::CustomerOrder {
            d.order_status = OrderStatus::Confirmed;
        }
        d
    }

    /// Numero visualizzato: "12/A" o "12".
    pub fn number_label(&self) -> String {
        match self.number {
            Some(n) if self.series.trim().is_empty() => n.to_string(),
            Some(n) => format!("{}/{}", n, self.series.trim()),
            None => "(bozza)".into(),
        }
    }

    /// Tipo documento FatturaPA effettivo.
    pub fn fe_type_effective(&self) -> Option<String> {
        if !self.fe_type.trim().is_empty() {
            return Some(self.fe_type.trim().to_string());
        }
        self.kind.default_fe_type().map(|s| s.to_string())
    }

    pub fn title(&self) -> String {
        format!("{} n. {} del {}", self.kind.label(), self.number_label(), self.date.format("%d/%m/%Y"))
    }

    /// Codice della valuta del documento (EUR se non indicata).
    pub fn currency_code(&self) -> &str {
        let c = self.currency.trim();
        if c.is_empty() {
            "EUR"
        } else {
            c
        }
    }

    pub fn is_foreign_currency(&self) -> bool {
        self.currency_code() != "EUR"
    }

    /// Converte in euro un importo espresso nella valuta del documento.
    pub fn to_eur(&self, amount: Decimal) -> Decimal {
        if !self.is_foreign_currency() || self.exchange_rate.is_zero() {
            amount
        } else {
            crate::money::round2(amount / self.exchange_rate)
        }
    }
}

// ---------------------------------------------------------------------------
// Pagamenti, prima nota, magazzino
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CashEntry {
    pub id: Id,
    pub date: NaiveDate,
    pub description: String,
    /// Conto/risorsa: "Cassa", "Banca ...", ecc.
    pub account: String,
    /// Positivo = entrata, negativo = uscita.
    pub amount: Decimal,
    pub document_id: Option<Id>,
    pub due_date_id: Option<Id>,
    pub party_id: Option<Id>,
    pub category: String,
}

impl Default for CashEntry {
    fn default() -> Self {
        CashEntry {
            id: 0,
            date: chrono::Local::now().date_naive(),
            description: String::new(),
            account: "Cassa".into(),
            amount: Decimal::ZERO,
            document_id: None,
            due_date_id: None,
            party_id: None,
            category: String::new(),
        }
    }
}

/// Magazzino o deposito dell'azienda.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Warehouse {
    pub id: Id,
    pub name: String,
    pub address: String,
    pub notes: String,
    pub is_default: bool,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StockMove {
    pub id: Id,
    pub date: NaiveDate,
    pub product_id: Id,
    /// Positivo = carico, negativo = scarico.
    pub qty: Decimal,
    pub warehouse: String,
    pub unit_value: Decimal,
    pub reason: String,
    pub document_id: Option<Id>,
    pub line_id: Option<Id>,
    pub lot: String,
    pub variant: String,
    pub expiry: Option<NaiveDate>,
    pub serial: String,
}

impl Default for StockMove {
    fn default() -> Self {
        StockMove {
            id: 0,
            date: chrono::Local::now().date_naive(),
            product_id: 0,
            qty: Decimal::ZERO,
            warehouse: "Principale".into(),
            unit_value: Decimal::ZERO,
            reason: String::new(),
            document_id: None,
            line_id: None,
            lot: String::new(),
            variant: String::new(),
            expiry: None,
            serial: String::new(),
        }
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;
    #[test]
    fn combinations_and_ean() {
        let v = VariantSpec { axis1: "Taglia".into(), values1: vec!["S".into(), "M".into()], axis2: "Colore".into(), values2: vec!["Rosso".into(), " ".into(), "Blu".into()], barcodes: vec![] };
        assert_eq!(v.combinations(), vec!["S/Rosso", "S/Blu", "M/Rosso", "M/Blu"]);
        assert!(v.contains("M/Blu"));
        let one = VariantSpec { values1: vec!["40".into(), "41".into()], ..Default::default() };
        assert_eq!(one.combinations(), vec!["40", "41"]);
        assert_eq!(ean13_check_digit("400638133393"), Some(1));
        assert!(ean13_valid("4006381333931"));
        assert!(!ean13_valid("4006381333932"));
    }
}
