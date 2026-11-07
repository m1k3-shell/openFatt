//! Interoperabilità con altri programmi tramite formati pubblici:
//! - Easyfatt-XML (specifica pubblicata da Danea per lo scambio di documenti);
//! - CSV di anagrafiche e articoli (separatore ; o , con intestazioni in italiano);
//! - listini Metel, ordini NSO della Pubblica Amministrazione (UBL), ordini dei negozi online.
//!
//! Le importazioni producono un'anteprima: nulla viene scritto finché l'utente non conferma.

pub mod csv;
pub mod easyfatt_xml;
pub mod inventory;
pub mod metel;
pub mod nso;
pub mod shop;
pub mod table;

pub use easyfatt_xml::{export_documents, parse_documents, ImportedDocument};
