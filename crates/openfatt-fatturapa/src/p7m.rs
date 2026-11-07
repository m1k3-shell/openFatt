//! Estrazione del contenuto da buste CAdES/PKCS#7 (.p7m).
//!
//! Supporta codifica DER e BER (lunghezze indefinite, OCTET STRING costruiti a blocchi),
//! file codificati in base64 e buste annidate (firme multiple successive).
//! La verifica crittografica della firma non viene eseguita: lo SdI verifica la firma
//! dei file che inoltra; il contenuto estratto è comunque validato come XML.

use base64::Engine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P7mError(pub String);

impl std::fmt::Display for P7mError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for P7mError {}

#[derive(Debug)]
struct Tlv {
    tag: u8,
    constructed: bool,
    /// contenuto primitivo
    value: Vec<u8>,
    children: Vec<Tlv>,
}

const OID_SIGNED_DATA: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02];

fn read_len(data: &[u8], pos: &mut usize) -> Result<Option<usize>, P7mError> {
    let b = *data.get(*pos).ok_or_else(|| P7mError("lunghezza troncata".into()))?;
    *pos += 1;
    if b == 0x80 {
        return Ok(None); // indefinita
    }
    if b & 0x80 == 0 {
        return Ok(Some(b as usize));
    }
    let n = (b & 0x7F) as usize;
    if n > 8 {
        return Err(P7mError("lunghezza non supportata".into()));
    }
    let mut len = 0usize;
    for _ in 0..n {
        let x = *data.get(*pos).ok_or_else(|| P7mError("lunghezza troncata".into()))?;
        *pos += 1;
        len = len.checked_mul(256).and_then(|l| l.checked_add(x as usize)).ok_or_else(|| P7mError("lunghezza eccessiva".into()))?;
    }
    Ok(Some(len))
}

fn parse_tlv(data: &[u8], pos: &mut usize, depth: usize) -> Result<Tlv, P7mError> {
    if depth > 64 {
        return Err(P7mError("annidamento eccessivo".into()));
    }
    let tag = *data.get(*pos).ok_or_else(|| P7mError("dati troncati".into()))?;
    *pos += 1;
    if tag & 0x1F == 0x1F {
        // tag in forma estesa: saltiamo i byte successivi
        while let Some(b) = data.get(*pos) {
            *pos += 1;
            if b & 0x80 == 0 {
                break;
            }
        }
    }
    let constructed = tag & 0x20 != 0;
    let len = read_len(data, pos)?;
    if constructed {
        let mut children = Vec::new();
        match len {
            Some(l) => {
                let end = pos.checked_add(l).filter(|e| *e <= data.len()).ok_or_else(|| P7mError("lunghezza oltre i dati".into()))?;
                while *pos < end {
                    children.push(parse_tlv(data, pos, depth + 1)?);
                }
                *pos = end;
            }
            None => loop {
                if data.get(*pos) == Some(&0) && data.get(*pos + 1) == Some(&0) {
                    *pos += 2;
                    break;
                }
                if *pos >= data.len() {
                    return Err(P7mError("fine contenuto indefinito mancante".into()));
                }
                children.push(parse_tlv(data, pos, depth + 1)?);
            },
        }
        Ok(Tlv { tag, constructed, value: Vec::new(), children })
    } else {
        let l = len.ok_or_else(|| P7mError("lunghezza indefinita su primitivo".into()))?;
        let end = pos.checked_add(l).filter(|e| *e <= data.len()).ok_or_else(|| P7mError("lunghezza oltre i dati".into()))?;
        let value = data[*pos..end].to_vec();
        *pos = end;
        Ok(Tlv { tag, constructed, value, children: Vec::new() })
    }
}

fn octets(t: &Tlv) -> Vec<u8> {
    if t.constructed {
        t.children.iter().flat_map(octets).collect()
    } else {
        t.value.clone()
    }
}

fn maybe_base64(data: &[u8]) -> Option<Vec<u8>> {
    let s: String = std::str::from_utf8(data).ok()?.chars().filter(|c| !c.is_whitespace()).collect();
    let s = s
        .trim_start_matches("-----BEGINPKCS7-----")
        .trim_end_matches("-----ENDPKCS7-----")
        .to_string();
    base64::engine::general_purpose::STANDARD.decode(s.as_bytes()).ok()
}

fn extract_once(data: &[u8]) -> Result<Vec<u8>, P7mError> {
    let mut pos = 0;
    let root = parse_tlv(data, &mut pos, 0)?;
    if root.tag != 0x30 || root.children.len() < 2 {
        return Err(P7mError("struttura PKCS#7 non riconosciuta".into()));
    }
    if root.children[0].tag != 0x06 || root.children[0].value != OID_SIGNED_DATA {
        return Err(P7mError("il file non contiene SignedData".into()));
    }
    let explicit = &root.children[1];
    let signed = explicit.children.first().ok_or_else(|| P7mError("SignedData vuoto".into()))?;
    // SignedData: version, digestAlgorithms, encapContentInfo, ...
    let encap = signed
        .children
        .iter()
        .find(|c| c.tag == 0x30 && c.children.first().map(|o| o.tag == 0x06).unwrap_or(false))
        .ok_or_else(|| P7mError("encapContentInfo mancante".into()))?;
    let content = encap
        .children
        .iter()
        .find(|c| c.tag == 0xA0)
        .and_then(|c| c.children.first())
        .ok_or_else(|| P7mError("firma detached: il contenuto non è incluso nel file".into()))?;
    Ok(octets(content))
}

/// Estrae il documento firmato. Gestisce base64 e firme annidate.
pub fn extract(data: &[u8]) -> Result<Vec<u8>, P7mError> {
    let mut current = data.to_vec();
    if current.first() != Some(&0x30) {
        current = maybe_base64(&current).ok_or_else(|| P7mError("formato .p7m non riconosciuto".into()))?;
    }
    for _ in 0..5 {
        let inner = extract_once(&current)?;
        let trimmed: &[u8] = {
            let mut s = inner.as_slice();
            while let Some((&b, rest)) = s.split_first() {
                if b.is_ascii_whitespace() {
                    s = rest;
                } else {
                    break;
                }
            }
            s
        };
        if trimmed.first() == Some(&0x30) && extract_once(trimmed).is_ok() {
            current = trimmed.to_vec();
            continue;
        }
        return Ok(inner);
    }
    Err(P7mError("troppe buste annidate".into()))
}

/// Vero se i dati sembrano una busta PKCS#7.
pub fn looks_signed(data: &[u8]) -> bool {
    extract(data).is_ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn der_len(n: usize) -> Vec<u8> {
        if n < 128 {
            vec![n as u8]
        } else if n < 256 {
            vec![0x81, n as u8]
        } else {
            vec![0x82, (n >> 8) as u8, n as u8]
        }
    }
    fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut v = vec![tag];
        v.extend(der_len(content.len()));
        v.extend_from_slice(content);
        v
    }

    /// Costruisce una busta SignedData minimale (senza firma reale) per i test.
    pub fn fake_p7m(content: &[u8], ber_chunks: bool) -> Vec<u8> {
        let oid_data = tlv(0x06, &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x01]);
        let econtent = if ber_chunks {
            // OCTET STRING costruito a lunghezza indefinita, in due blocchi
            let mid = content.len() / 2;
            let mut v = vec![0x24, 0x80];
            v.extend(tlv(0x04, &content[..mid]));
            v.extend(tlv(0x04, &content[mid..]));
            v.extend([0, 0]);
            v
        } else {
            tlv(0x04, content)
        };
        let encap = tlv(0x30, &[oid_data, tlv(0xA0, &econtent)].concat());
        let version = tlv(0x02, &[1]);
        let digest = tlv(0x31, &[]);
        let signer_infos = tlv(0x31, &[]);
        let signed = tlv(0x30, &[version, digest, encap, signer_infos].concat());
        let oid_sd = tlv(0x06, OID_SIGNED_DATA);
        tlv(0x30, &[oid_sd, tlv(0xA0, &signed)].concat())
    }

    #[test]
    fn der_and_ber() {
        let xml = b"<?xml version=\"1.0\"?><a>ciao</a>".repeat(20);
        assert_eq!(extract(&fake_p7m(&xml, false)).unwrap(), xml);
        assert_eq!(extract(&fake_p7m(&xml, true)).unwrap(), xml);
        let b64 = base64::engine::general_purpose::STANDARD.encode(fake_p7m(&xml, false));
        assert_eq!(extract(b64.as_bytes()).unwrap(), xml);
        // firma annidata
        let nested = fake_p7m(&fake_p7m(&xml, false), false);
        assert_eq!(extract(&nested).unwrap(), xml);
        assert!(extract(b"<xml/>").is_err());
    }
}
