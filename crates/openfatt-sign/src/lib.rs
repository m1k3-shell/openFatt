//! Firma digitale CAdES-BES (busta PKCS#7/CMS, file .p7m) delle fatture elettroniche.
//!
//! Lo SdI accetta per le fatture firmate il formato CAdES-BES con algoritmo SHA-256 e
//! l'attributo firmato `signing-certificate-v2` (ESS, RFC 5035). La firma è obbligatoria
//! per le fatture verso la Pubblica Amministrazione (FPA12).
//!
//! Su Windows si usano le API crittografiche di sistema (crypt32):
//! - certificati nell'archivio personale dell'utente: comprende le smart card e i token USB
//!   dei certificatori, i cui driver li registrano nell'archivio quando il dispositivo è
//!   collegato; il PIN viene chiesto dal driver;
//! - certificati in file PKCS#12 (.pfx/.p12).
//!
//! La chiave privata non lascia mai il dispositivo: il programma passa a Windows solo il
//! documento da firmare.

#[cfg(windows)]
mod win;

pub mod der;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignError(pub String);

impl std::fmt::Display for SignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SignError {}

pub type Result<T> = std::result::Result<T, SignError>;

/// Certificato disponibile per la firma.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertInfo {
    /// Impronta SHA-1 in esadecimale (identifica il certificato nell'archivio).
    pub thumbprint: String,
    pub subject: String,
    pub issuer: String,
    /// Scadenza AAAA-MM-GG.
    pub valid_to: String,
    pub valid_from: String,
    /// Il certificato ha l'uso "non ripudio" tipico della firma qualificata.
    pub non_repudiation: bool,
    pub expired: bool,
}

impl CertInfo {
    pub fn label(&self) -> String {
        format!("{} (emesso da {}, scade il {})", self.subject, self.issuer, self.valid_to)
    }
}

/// Origine del certificato con cui firmare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SignSource {
    #[default]
    None,
    /// Certificato nell'archivio personale di Windows (anche smart card/token).
    Store { thumbprint: String, label: String },
    /// File PKCS#12; la password viene chiesta a ogni firma e non è memorizzata.
    Pfx { path: String },
}

/// Esito della verifica di una busta firmata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub signer: CertInfo,
    pub content: Vec<u8>,
    /// Presenza dell'attributo signing-certificate-v2 (requisito CAdES-BES).
    pub cades: bool,
    pub sha256: bool,
}

/// Certificati con chiave privata presenti nell'archivio personale dell'utente.
pub fn list_certificates() -> Result<Vec<CertInfo>> {
    #[cfg(windows)]
    {
        win::list_certificates()
    }
    #[cfg(not(windows))]
    {
        Err(SignError("Firma integrata disponibile solo su Windows: usare un programma di firma esterno".into()))
    }
}

/// Firma `data` producendo una busta CAdES-BES con contenuto incluso (.p7m, DER).
pub fn sign(data: &[u8], source: &SignSource, pfx_password: &str) -> Result<Vec<u8>> {
    #[cfg(windows)]
    {
        match source {
            SignSource::None => Err(SignError("Nessun certificato di firma configurato (Opzioni > Firma digitale)".into())),
            SignSource::Store { thumbprint, .. } => win::sign_with_store_cert(data, thumbprint),
            SignSource::Pfx { path } => {
                let pfx = std::fs::read(path).map_err(|e| SignError(format!("Impossibile leggere {}: {}", path, e)))?;
                win::sign_with_pfx(data, &pfx, pfx_password)
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (data, source, pfx_password);
        Err(SignError("Firma integrata disponibile solo su Windows".into()))
    }
}

/// Firma con un file PKCS#12 già letto in memoria.
pub fn sign_with_pfx_bytes(data: &[u8], pfx: &[u8], password: &str) -> Result<Vec<u8>> {
    #[cfg(windows)]
    {
        win::sign_with_pfx(data, pfx, password)
    }
    #[cfg(not(windows))]
    {
        let _ = (data, pfx, password);
        Err(SignError("Firma integrata disponibile solo su Windows".into()))
    }
}

/// Informazioni sul certificato contenuto in un file .pfx (per mostrarlo all'utente).
pub fn pfx_info(pfx: &[u8], password: &str) -> Result<CertInfo> {
    #[cfg(windows)]
    {
        win::pfx_info(pfx, password)
    }
    #[cfg(not(windows))]
    {
        let _ = (pfx, password);
        Err(SignError("non disponibile".into()))
    }
}

/// Verifica crittografica della firma (integrità e corrispondenza con il certificato del
/// firmatario). Non verifica la catena di fiducia né la revoca del certificato.
pub fn verify(p7m: &[u8]) -> Result<Verified> {
    let der_bytes = if p7m.first() == Some(&0x30) {
        p7m.to_vec()
    } else {
        use base64::Engine;
        let s: String = String::from_utf8_lossy(p7m).chars().filter(|c| !c.is_whitespace()).collect();
        base64::engine::general_purpose::STANDARD.decode(s.as_bytes()).map_err(|_| SignError("formato .p7m non riconosciuto".into()))?
    };
    let cades = der::contains(&der_bytes, der::OID_SIGNING_CERTIFICATE_V2);
    let sha256 = der::contains(&der_bytes, der::OID_SHA256);
    #[cfg(windows)]
    {
        let (signer, content) = win::verify(&der_bytes)?;
        Ok(Verified { signer, content, cades, sha256 })
    }
    #[cfg(not(windows))]
    {
        let _ = (cades, sha256);
        Err(SignError("Verifica disponibile solo su Windows".into()))
    }
}
