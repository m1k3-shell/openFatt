//! Implementazione Windows (crypt32): archivio certificati, PKCS#12, CryptSignMessage,
//! CryptVerifyMessageSignature.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{GetLastError, FILETIME, SYSTEMTIME};
use windows_sys::Win32::Security::Cryptography::*;
use windows_sys::Win32::System::Time::FileTimeToSystemTime;

use crate::{der, CertInfo, Result, SignError};

const ENCODING: u32 = X509_ASN_ENCODING | PKCS_7_ASN_ENCODING;
// flag di PFXImportCertStore
const PKCS12_ALWAYS_CNG_KSP_FLAG: u32 = 0x0000_0200;
const PKCS12_NO_PERSIST_KEY_FLAG: u32 = 0x0000_8000;

fn last_error(what: &str) -> SignError {
    let code = unsafe { GetLastError() };
    let hint = match code {
        0x8010_006E | 0x8010_000C => " (operazione annullata o smart card non inserita)",
        0x8009_0016 => " (chiave privata non trovata: inserire smart card/token)",
        0x8009_0008 => " (algoritmo non supportato dal dispositivo)",
        0x8007_0056 => " (password errata)",
        0x8009_0010 => " (accesso alla chiave negato o PIN errato)",
        _ => "",
    };
    SignError(format!("{} non riuscito: codice 0x{:08X}{}", what, code, hint))
}

struct Store(HCERTSTORE);
impl Drop for Store {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CertCloseStore(self.0, 0) };
        }
    }
}

struct Cert(*const CERT_CONTEXT);
impl Drop for Cert {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CertFreeCertificateContext(self.0) };
        }
    }
}

fn open_my_store() -> Result<Store> {
    let name: Vec<u16> = "MY\0".encode_utf16().collect();
    let h = unsafe { CertOpenStore(CERT_STORE_PROV_SYSTEM_W, 0, 0, CERT_SYSTEM_STORE_CURRENT_USER | CERT_STORE_READONLY_FLAG, name.as_ptr() as *const c_void) };
    if h.is_null() {
        return Err(last_error("Apertura archivio certificati"));
    }
    Ok(Store(h))
}

fn name_string(c: *const CERT_CONTEXT, issuer: bool) -> String {
    let flags = if issuer { CERT_NAME_ISSUER_FLAG } else { 0 };
    unsafe {
        let n = CertGetNameStringW(c, CERT_NAME_SIMPLE_DISPLAY_TYPE, flags, null(), null_mut(), 0);
        if n <= 1 {
            return String::new();
        }
        let mut buf = vec![0u16; n as usize];
        CertGetNameStringW(c, CERT_NAME_SIMPLE_DISPLAY_TYPE, flags, null(), buf.as_mut_ptr(), n);
        String::from_utf16_lossy(&buf[..(n as usize - 1)])
    }
}

fn filetime_date(ft: &FILETIME) -> (String, chrono::NaiveDate) {
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { FileTimeToSystemTime(ft, &mut st) };
    let d = chrono::NaiveDate::from_ymd_opt(st.wYear as i32, st.wMonth as u32, st.wDay as u32).unwrap_or_default();
    (d.format("%Y-%m-%d").to_string(), d)
}

fn has_private_key(c: *const CERT_CONTEXT) -> bool {
    // chiave persistita (CSP/KSP, smart card) oppure chiave in memoria (PKCS#12 non persistito)
    [CERT_KEY_PROV_INFO_PROP_ID, CERT_KEY_CONTEXT_PROP_ID, CERT_NCRYPT_KEY_HANDLE_PROP_ID].iter().any(|prop| {
        let mut size = 0u32;
        unsafe { CertGetCertificateContextProperty(c, *prop, null_mut(), &mut size) != 0 }
    })
}

fn thumbprint(c: *const CERT_CONTEXT) -> String {
    let mut buf = [0u8; 20];
    let mut size = 20u32;
    unsafe { CertGetCertificateContextProperty(c, CERT_SHA1_HASH_PROP_ID, buf.as_mut_ptr() as *mut c_void, &mut size) };
    buf[..size as usize].iter().map(|b| format!("{:02X}", b)).collect()
}

fn non_repudiation(c: *const CERT_CONTEXT) -> bool {
    let mut usage = [0u8; 2];
    let ok = unsafe { CertGetIntendedKeyUsage(ENCODING, (*c).pCertInfo, usage.as_mut_ptr(), 2) };
    ok != 0 && (usage[0] as u32 & CERT_NON_REPUDIATION_KEY_USAGE) != 0
}

fn info(c: *const CERT_CONTEXT) -> CertInfo {
    let ci = unsafe { &*(*c).pCertInfo };
    let (valid_to, to) = filetime_date(&ci.NotAfter);
    let (valid_from, _) = filetime_date(&ci.NotBefore);
    CertInfo {
        thumbprint: thumbprint(c),
        subject: name_string(c, false),
        issuer: name_string(c, true),
        valid_to,
        valid_from,
        non_repudiation: non_repudiation(c),
        expired: to < chrono::Local::now().date_naive(),
    }
}

pub fn list_certificates() -> Result<Vec<CertInfo>> {
    let store = open_my_store()?;
    let mut out = Vec::new();
    let mut prev: *const CERT_CONTEXT = null();
    loop {
        let c = unsafe { CertEnumCertificatesInStore(store.0, prev) };
        if c.is_null() {
            break;
        }
        if has_private_key(c) {
            out.push(info(c));
        }
        prev = c; // CertEnumCertificatesInStore libera il contesto precedente
    }
    // i certificati di firma qualificata (non ripudio) prima, poi quelli validi
    out.sort_by(|a, b| b.non_repudiation.cmp(&a.non_repudiation).then(a.expired.cmp(&b.expired)).then(a.subject.cmp(&b.subject)));
    Ok(out)
}

fn hex_to_bytes(h: &str) -> Vec<u8> {
    (0..h.len() / 2).filter_map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).ok()).collect()
}

fn find_by_thumbprint(store: &Store, tp: &str) -> Result<Cert> {
    let mut hash = hex_to_bytes(&tp.replace(' ', ""));
    let blob = CRYPT_INTEGER_BLOB { cbData: hash.len() as u32, pbData: hash.as_mut_ptr() };
    let c = unsafe { CertFindCertificateInStore(store.0, ENCODING, 0, CERT_FIND_SHA1_HASH, &blob as *const _ as *const c_void, null()) };
    if c.is_null() {
        return Err(SignError("Certificato non trovato: collegare la smart card/il token o sceglierlo di nuovo nelle impostazioni".into()));
    }
    Ok(Cert(c))
}

fn import_pfx(pfx: &[u8], password: &str) -> Result<Store> {
    let mut data = pfx.to_vec();
    let blob = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_mut_ptr() };
    let pw: Vec<u16> = password.encode_utf16().chain(std::iter::once(0)).collect();
    let mut h = unsafe { PFXImportCertStore(&blob, pw.as_ptr(), PKCS12_ALWAYS_CNG_KSP_FLAG | PKCS12_NO_PERSIST_KEY_FLAG) };
    if h.is_null() {
        // sistemi più vecchi: chiave persistita nel provider CNG
        h = unsafe { PFXImportCertStore(&blob, pw.as_ptr(), PKCS12_ALWAYS_CNG_KSP_FLAG) };
    }
    if h.is_null() {
        return Err(last_error("Apertura del file .pfx (password corretta?)"));
    }
    Ok(Store(h))
}

fn first_with_key(store: &Store) -> Result<Cert> {
    let mut prev: *const CERT_CONTEXT = null();
    loop {
        let c = unsafe { CertEnumCertificatesInStore(store.0, prev) };
        if c.is_null() {
            return Err(SignError("Il file .pfx non contiene un certificato con chiave privata".into()));
        }
        if has_private_key(c) {
            let dup = unsafe { CertDuplicateCertificateContext(c) };
            unsafe { CertFreeCertificateContext(c) };
            return Ok(Cert(dup));
        }
        prev = c;
    }
}

pub fn pfx_info(pfx: &[u8], password: &str) -> Result<CertInfo> {
    let store = import_pfx(pfx, password)?;
    let c = first_with_key(&store)?;
    Ok(info(c.0))
}

pub fn sign_with_store_cert(data: &[u8], thumbprint: &str) -> Result<Vec<u8>> {
    let store = open_my_store()?;
    let c = find_by_thumbprint(&store, thumbprint)?;
    sign_with_cert(data, c.0)
}

pub fn sign_with_pfx(data: &[u8], pfx: &[u8], password: &str) -> Result<Vec<u8>> {
    let store = import_pfx(pfx, password)?;
    let c = first_with_key(&store)?;
    sign_with_cert(data, c.0)
}

fn sign_with_cert(data: &[u8], cert: *const CERT_CONTEXT) -> Result<Vec<u8>> {
    let i = info(cert);
    if i.expired {
        return Err(SignError(format!("Il certificato di {} è scaduto il {}", i.subject, i.valid_to)));
    }
    let (cert_der, issuer, serial_be) = unsafe {
        let cc = &*cert;
        let der_bytes = std::slice::from_raw_parts(cc.pbCertEncoded, cc.cbCertEncoded as usize).to_vec();
        let ci = &*cc.pCertInfo;
        let issuer = std::slice::from_raw_parts(ci.Issuer.pbData, ci.Issuer.cbData as usize).to_vec();
        // il numero di serie in CERT_INFO è little-endian
        let mut serial = std::slice::from_raw_parts(ci.SerialNumber.pbData, ci.SerialNumber.cbData as usize).to_vec();
        serial.reverse();
        (der_bytes, issuer, serial)
    };

    // attributi firmati: signing-certificate-v2 e signingTime
    // (content-type e message-digest sono aggiunti da CryptSignMessage)
    let mut scv2 = der::signing_certificate_v2(&Sha256::digest(&cert_der), &issuer, &serial_be);
    let mut stime = der::utc_time(chrono::Utc::now());
    let mut scv2_blob = CRYPT_INTEGER_BLOB { cbData: scv2.len() as u32, pbData: scv2.as_mut_ptr() };
    let mut stime_blob = CRYPT_INTEGER_BLOB { cbData: stime.len() as u32, pbData: stime.as_mut_ptr() };
    let mut oid_scv2 = b"1.2.840.113549.1.9.16.2.47\0".to_vec();
    let mut oid_stime = b"1.2.840.113549.1.9.5\0".to_vec();
    let mut attrs = [
        CRYPT_ATTRIBUTE { pszObjId: oid_scv2.as_mut_ptr(), cValue: 1, rgValue: &mut scv2_blob },
        CRYPT_ATTRIBUTE { pszObjId: oid_stime.as_mut_ptr(), cValue: 1, rgValue: &mut stime_blob },
    ];
    let mut sha256_oid = b"2.16.840.1.101.3.4.2.1\0".to_vec();
    let mut certs = [cert as *mut CERT_CONTEXT];

    let mut para: CRYPT_SIGN_MESSAGE_PARA = unsafe { std::mem::zeroed() };
    para.cbSize = std::mem::size_of::<CRYPT_SIGN_MESSAGE_PARA>() as u32;
    para.dwMsgEncodingType = ENCODING;
    para.pSigningCert = cert;
    para.HashAlgorithm = CRYPT_ALGORITHM_IDENTIFIER { pszObjId: sha256_oid.as_mut_ptr(), Parameters: CRYPT_INTEGER_BLOB { cbData: 0, pbData: null_mut() } };
    para.cMsgCert = 1;
    para.rgpMsgCert = certs.as_mut_ptr();
    para.cAuthAttr = attrs.len() as u32;
    para.rgAuthAttr = attrs.as_mut_ptr();

    let ptrs = [data.as_ptr()];
    let lens = [data.len() as u32];
    let mut size = 0u32;
    let ok = unsafe { CryptSignMessage(&para, 0, 1, ptrs.as_ptr(), lens.as_ptr(), null_mut(), &mut size) };
    if ok == 0 {
        return Err(last_error("Preparazione della firma"));
    }
    let mut out = vec![0u8; size as usize];
    let ok = unsafe { CryptSignMessage(&para, 0, 1, ptrs.as_ptr(), lens.as_ptr(), out.as_mut_ptr(), &mut size) };
    if ok == 0 {
        return Err(last_error("Firma"));
    }
    out.truncate(size as usize);
    Ok(out)
}

pub fn verify(p7m: &[u8]) -> Result<(CertInfo, Vec<u8>)> {
    let mut para: CRYPT_VERIFY_MESSAGE_PARA = unsafe { std::mem::zeroed() };
    para.cbSize = std::mem::size_of::<CRYPT_VERIFY_MESSAGE_PARA>() as u32;
    para.dwMsgAndCertEncodingType = ENCODING;
    let mut size = 0u32;
    let mut signer: *mut CERT_CONTEXT = null_mut();
    let ok = unsafe { CryptVerifyMessageSignature(&para, 0, p7m.as_ptr(), p7m.len() as u32, null_mut(), &mut size, null_mut()) };
    if ok == 0 {
        return Err(last_error("Verifica della firma"));
    }
    let mut content = vec![0u8; size as usize];
    let ok = unsafe { CryptVerifyMessageSignature(&para, 0, p7m.as_ptr(), p7m.len() as u32, content.as_mut_ptr(), &mut size, &mut signer) };
    if ok == 0 {
        return Err(SignError("Firma non valida: il documento è stato alterato o la firma non corrisponde al certificato".into()));
    }
    content.truncate(size as usize);
    let signer = Cert(signer);
    Ok((info(signer.0), content))
}
