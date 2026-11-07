//! Firma e verifica con un certificato di prova autofirmato (tests/data/test-firma.pfx, password "prova").

fn pfx() -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/test-firma.pfx")).unwrap()
}

fn xml() -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../specs/esempi/IT01234567890_FPA01.xml");
    std::fs::read(p).unwrap_or_else(|_| "<?xml version=\"1.0\"?><prova>àèì</prova>".as_bytes().to_vec())
}

#[test]
fn sign_and_verify_cades_bes() {
    let data = xml();
    let p7m = openfatt_sign::sign_with_pfx_bytes(&data, &pfx(), "prova").unwrap();
    assert_eq!(p7m[0], 0x30);
    // il contenuto si estrae anche con il lettore indipendente di openfatt-fatturapa
    assert_eq!(openfatt_fatturapa::p7m::extract(&p7m).unwrap(), data);
    let v = openfatt_sign::verify(&p7m).unwrap();
    assert_eq!(v.content, data);
    assert!(v.cades, "manca signing-certificate-v2");
    assert!(v.sha256, "manca SHA-256");
    assert!(v.signer.subject.contains("Firma di Prova"), "{:?}", v.signer);
    assert!(v.signer.non_repudiation);
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/xsd-samples-firma");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("IT01234567890_FPA01.xml.p7m"), &p7m).unwrap();
}

#[test]
fn tampered_signature_is_rejected() {
    let data = xml();
    let mut p7m = openfatt_sign::sign_with_pfx_bytes(&data, &pfx(), "prova").unwrap();
    // altera un byte del contenuto firmato
    let pos = p7m.windows(5).position(|w| w == b"<?xml").unwrap() + 30;
    p7m[pos] ^= 0x01;
    assert!(openfatt_sign::verify(&p7m).is_err());
}

#[test]
fn wrong_password() {
    let e = openfatt_sign::sign_with_pfx_bytes(b"x", &pfx(), "sbagliata").unwrap_err();
    assert!(e.0.contains("pfx"), "{}", e);
}

#[test]
fn pfx_info_and_store_listing() {
    let i = openfatt_sign::pfx_info(&pfx(), "prova").unwrap();
    assert!(!i.expired);
    assert_eq!(i.thumbprint.len(), 40);
    // l'elenco dell'archivio personale deve funzionare anche se vuoto
    openfatt_sign::list_certificates().unwrap();
}
