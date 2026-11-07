//! Codifica DER minima per gli attributi firmati CAdES.

/// 1.2.840.113549.1.9.16.2.47 id-aa-signingCertificateV2
pub const OID_SIGNING_CERTIFICATE_V2: &[u8] = &[0x06, 0x0B, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x10, 0x02, 0x2F];
/// 2.16.840.1.101.3.4.2.1 sha256
pub const OID_SHA256: &[u8] = &[0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];

pub fn len(n: usize) -> Vec<u8> {
    if n < 0x80 {
        vec![n as u8]
    } else {
        let bytes: Vec<u8> = n.to_be_bytes().iter().copied().skip_while(|b| *b == 0).collect();
        let mut v = vec![0x80 | bytes.len() as u8];
        v.extend(bytes);
        v
    }
}

pub fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut v = vec![tag];
    v.extend(len(content.len()));
    v.extend_from_slice(content);
    v
}

/// INTEGER da byte big-endian (aggiunge lo zero iniziale se il bit alto è 1).
pub fn integer(be: &[u8]) -> Vec<u8> {
    let mut b: Vec<u8> = be.iter().copied().skip_while(|x| *x == 0).collect();
    if b.is_empty() {
        b.push(0);
    }
    if b[0] & 0x80 != 0 {
        b.insert(0, 0);
    }
    tlv(0x02, &b)
}

/// Valore dell'attributo SigningCertificateV2 (RFC 5035):
/// SEQUENCE { SEQUENCE OF ESSCertIDv2 { certHash, IssuerSerial } }
/// con hashAlgorithm omesso (predefinito SHA-256).
pub fn signing_certificate_v2(cert_sha256: &[u8], issuer_name_der: &[u8], serial_be: &[u8]) -> Vec<u8> {
    let general_names = tlv(0x30, &tlv(0xA4, issuer_name_der)); // directoryName [4] EXPLICIT Name
    let issuer_serial = tlv(0x30, &[general_names, integer(serial_be)].concat());
    let ess_cert_id = tlv(0x30, &[tlv(0x04, cert_sha256), issuer_serial].concat());
    tlv(0x30, &tlv(0x30, &ess_cert_id))
}

/// UTCTime (AAMMGGhhmmssZ) per l'attributo signingTime.
pub fn utc_time(t: chrono::DateTime<chrono::Utc>) -> Vec<u8> {
    tlv(0x17, t.format("%y%m%d%H%M%SZ").to_string().as_bytes())
}

pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encodings() {
        assert_eq!(len(5), vec![5]);
        assert_eq!(len(300), vec![0x82, 0x01, 0x2C]);
        assert_eq!(integer(&[0x80]), vec![0x02, 0x02, 0x00, 0x80]);
        assert_eq!(integer(&[0x00, 0x01]), vec![0x02, 0x01, 0x01]);
        let v = signing_certificate_v2(&[0u8; 32], &[0x30, 0x00], &[0x05]);
        // SEQ(SEQ(SEQ(OCTET32, SEQ(SEQ([4](NAME)), INT))))
        assert_eq!(v[0], 0x30);
        assert!(contains(&v, &[0x04, 0x20]));
        assert!(contains(&v, &[0xA4, 0x02, 0x30, 0x00]));
    }
}
