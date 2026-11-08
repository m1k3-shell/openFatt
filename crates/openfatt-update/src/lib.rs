//! Firma e verifica degli aggiornamenti.
//!
//! Ogni versione pubblicata ha un manifesto firmato con Ed25519: indica versione, nome, dimensione e
//! impronta SHA-256 dell'installatore. Il programma contiene solo la chiave pubblica e installa
//! soltanto file che corrispondono a un manifesto firmato: l'accesso alla pagina delle versioni
//! da solo non basta per distribuire un programma diverso.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Dimensione massima accettata per un installatore.
pub const MAX_SIZE: u64 = 500 * 1024 * 1024;
/// Dimensione massima accettata per un manifesto.
pub const MAX_MANIFEST: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
    pub signature: String,
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// Testo firmato: il prefisso impedisce di riusare la firma per altri scopi.
fn signed_text(version: &str, file: &str, size: u64, sha256: &str) -> String {
    format!("openfatt-aggiornamento-1\n{}\n{}\n{}\n{}\n", version, file, size, sha256)
}

/// Nome semplice, senza cartelle né caratteri che il sistema potrebbe interpretare.
fn safe_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 100 && !name.starts_with('.') && name.ends_with(".exe") && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && !name.contains("..")
}

/// Nuova chiave privata (formato PKCS#8).
pub fn generate_key() -> Result<Vec<u8>, String> {
    let doc = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| "Generazione della chiave non riuscita".to_string())?;
    Ok(doc.as_ref().to_vec())
}

pub fn public_key_hex(private_key: &[u8]) -> Result<String, String> {
    let k = Ed25519KeyPair::from_pkcs8(private_key).map_err(|_| "Chiave privata non valida".to_string())?;
    Ok(to_hex(k.public_key().as_ref()))
}

fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let (mut h, mut buf, mut total) = (Sha256::new(), vec![0u8; 1 << 16], 0u64);
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok((total, to_hex(&h.finalize())));
        }
        h.update(&buf[..n]);
        total += n as u64;
    }
}

/// Firma un installatore e prepara il suo manifesto.
pub fn sign_file(private_key: &[u8], path: &Path, version: &str) -> Result<Manifest, String> {
    let key = Ed25519KeyPair::from_pkcs8(private_key).map_err(|_| "Chiave privata non valida".to_string())?;
    let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let (size, sha256) = hash_file(path)?;
    let signature = to_hex(key.sign(signed_text(version, &file, size, &sha256).as_bytes()).as_ref());
    let m = Manifest { version: version.to_string(), file, size, sha256, signature };
    verify(&m, &to_hex(key.public_key().as_ref()))?;
    Ok(m)
}

/// Controlla la firma e la correttezza dei campi del manifesto.
pub fn verify(m: &Manifest, public_key: &str) -> Result<(), String> {
    if !safe_name(&m.file) {
        return Err("Nome del file di aggiornamento non valido".into());
    }
    if m.size == 0 || m.size > MAX_SIZE {
        return Err("Dimensione del file di aggiornamento non valida".into());
    }
    if m.sha256.len() != 64 || from_hex(&m.sha256).is_none() {
        return Err("Impronta del file di aggiornamento non valida".into());
    }
    let key = from_hex(public_key).ok_or("Chiave pubblica non valida")?;
    let sig = from_hex(&m.signature).ok_or("Firma non valida")?;
    UnparsedPublicKey::new(&ED25519, &key).verify(signed_text(&m.version, &m.file, m.size, &m.sha256).as_bytes(), &sig).map_err(|_| "La firma dell'aggiornamento non è valida: il file non viene installato".to_string())
}

/// Salva il file ricevuto solo se corrisponde al manifesto. In caso contrario non resta nulla su disco.
pub fn save_verified(mut source: impl Read, m: &Manifest, dir: &Path, progress: &AtomicU64) -> Result<PathBuf, String> {
    if !safe_name(&m.file) {
        return Err("Nome del file di aggiornamento non valido".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let part = dir.join(format!("{}.part", m.file));
    let result = (|| -> Result<(), String> {
        let mut out = File::create(&part).map_err(|e| format!("{}: {}", part.display(), e))?;
        let (mut h, mut buf, mut total) = (Sha256::new(), vec![0u8; 1 << 16], 0u64);
        loop {
            let n = source.read(&mut buf).map_err(|e| format!("Scaricamento interrotto: {}", e))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > m.size {
                return Err("Il file ricevuto è più grande del previsto".into());
            }
            h.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| format!("Scrittura non riuscita: {}", e))?;
            progress.store(total, Ordering::Relaxed);
        }
        if total != m.size {
            return Err("Il file ricevuto è incompleto".into());
        }
        if to_hex(&h.finalize()) != m.sha256 {
            return Err("Il file ricevuto non corrisponde all'impronta firmata".into());
        }
        out.sync_all().map_err(|e| e.to_string())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    let dest = dir.join(&m.file);
    let _ = std::fs::remove_file(&dest);
    std::fs::rename(&part, &dest).map_err(|e| format!("{}: {}", dest.display(), e))?;
    Ok(dest)
}

/// Ricontrolla il file già scaricato, subito prima di eseguirlo.
pub fn check_file(path: &Path, m: &Manifest) -> Result<(), String> {
    let (size, sha256) = hash_file(path)?;
    if size == m.size && sha256 == m.sha256 {
        Ok(())
    } else {
        Err("Il file di aggiornamento è stato modificato dopo lo scaricamento".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str, content: &[u8]) -> (PathBuf, Vec<u8>, String, Manifest) {
        let dir = std::env::temp_dir().join(format!("openfatt-update-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key = generate_key().unwrap();
        let public = public_key_hex(&key).unwrap();
        let file = dir.join("openFatt-Setup-1.2.3.exe");
        std::fs::write(&file, content).unwrap();
        let m = sign_file(&key, &file, "1.2.3").unwrap();
        (dir, key, public, m)
    }

    #[test]
    fn manifesto_firmato_accettato() {
        let (_, _, public, m) = setup("ok", b"programma");
        assert_eq!(m.size, 9);
        assert!(verify(&m, &public).is_ok());
    }

    #[test]
    fn ogni_campo_modificato_invalida_la_firma() {
        let (_, _, public, m) = setup("campi", b"programma");
        for bad in [
            Manifest { version: "9.9.9".into(), ..m.clone() },
            Manifest { file: "altro.exe".into(), ..m.clone() },
            Manifest { size: 10, ..m.clone() },
            Manifest { sha256: "0".repeat(64), ..m.clone() },
            Manifest { signature: "0".repeat(128), ..m.clone() },
            Manifest { signature: "zz".into(), ..m.clone() },
        ] {
            assert!(verify(&bad, &public).is_err(), "{:?}", bad);
        }
    }

    #[test]
    fn chiave_diversa_rifiutata() {
        let (_, _, _, m) = setup("chiave", b"programma");
        let other = public_key_hex(&generate_key().unwrap()).unwrap();
        assert!(verify(&m, &other).is_err());
    }

    #[test]
    fn nomi_di_file_pericolosi_rifiutati() {
        let (_, key, public, m) = setup("nomi", b"programma");
        let _ = key;
        for name in ["..\\..\\a.exe", "a/b.exe", "a b.exe", "a.bat", "", ".exe", "a..exe", "a&calc.exe"] {
            let bad = Manifest { file: name.into(), ..m.clone() };
            assert!(verify(&bad, &public).is_err(), "{}", name);
            let dir = std::env::temp_dir();
            assert!(save_verified(&b"programma"[..], &bad, &dir, &AtomicU64::new(0)).is_err(), "{}", name);
        }
    }

    #[test]
    fn salva_solo_il_file_giusto() {
        let (dir, _, _, m) = setup("salva", b"programma");
        let down = dir.join("giu");
        let p = AtomicU64::new(0);
        let path = save_verified(&b"programma"[..], &m, &down, &p).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"programma");
        assert_eq!(p.load(Ordering::Relaxed), 9);
        assert!(check_file(&path, &m).is_ok());
        std::fs::write(&path, b"programmX").unwrap();
        assert!(check_file(&path, &m).is_err());
    }

    #[test]
    fn file_alterati_non_restano_su_disco() {
        let (dir, _, _, m) = setup("alterati", b"programma");
        let down = dir.join("giu");
        let p = AtomicU64::new(0);
        for bad in [&b"programmX"[..], &b"progr"[..], &b"programma in piu"[..], &b""[..]] {
            assert!(save_verified(bad, &m, &down, &p).is_err());
            assert_eq!(std::fs::read_dir(&down).unwrap().count(), 0, "resta un file per {:?}", bad);
        }
    }

    #[test]
    fn scaricamento_interrotto() {
        struct Rotto(u8);
        impl Read for Rotto {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                if self.0 == 0 {
                    return Err(std::io::Error::other("rete"));
                }
                self.0 -= 1;
                b[0] = b'p';
                Ok(1)
            }
        }
        let (dir, _, _, m) = setup("rotto", b"programma");
        let down = dir.join("giu");
        assert!(save_verified(Rotto(3), &m, &down, &AtomicU64::new(0)).is_err());
        assert_eq!(std::fs::read_dir(&down).unwrap().count(), 0);
    }
}
