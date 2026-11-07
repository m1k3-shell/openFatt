//! openFatt - nucleo di dominio.
//!
//! Modello dati, tabelle di codici FatturaPA, controlli fiscali e motore di calcolo.
//! Nessuna dipendenza da interfaccia o database: le stesse funzioni servono a UI,
//! importazioni, stampe ed esportazioni XML.

pub mod calc;
pub mod codes;
pub mod fiscal;
pub mod mailtext;
pub mod model;
pub mod money;
pub mod pricing;
pub mod vat;

pub use rust_decimal::Decimal;
pub use rust_decimal_macros::dec;
