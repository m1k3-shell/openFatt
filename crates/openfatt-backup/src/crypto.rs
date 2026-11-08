//! Cifratura delle copie: AES-256-GCM a blocchi da 1 MB con chiave derivata dalla password
//! (PBKDF2-HMAC-SHA256). Ogni blocco ha il proprio contatore e l'ultimo è marcato, quindi
//! un file modificato, riordinato o troncato viene rifiutato.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};

const MAGIC: &[u8; 6] = b"OFBK1\n";
const ITERATIONS: u32 = 600_000;
const CHUNK: usize = 1 << 20;

fn cipher(password: &str, salt: &[u8], iterations: u32) -> Result<Aes256Gcm, String> {
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(password.as_bytes(), salt, iterations, &mut key);
    Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())
}

fn nonce(prefix: &[u8; 7], counter: u32, last: bool) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..7].copy_from_slice(prefix);
    n[7..11].copy_from_slice(&counter.to_be_bytes());
    n[11] = last as u8;
    n
}

/// Legge fino a riempire il blocco (o alla fine del file).
fn fill(r: &mut impl Read, buf: &mut [u8]) -> Result<usize, String> {
    let mut got = 0;
    while got < buf.len() {
        let n = r.read(&mut buf[got..]).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        got += n;
    }
    Ok(got)
}

pub fn encrypt_file(src: &Path, dst: &Path, password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("password di cifratura mancante".into());
    }
    let mut salt = [0u8; 16];
    let mut prefix = [0u8; 7];
    getrandom::getrandom(&mut salt).map_err(|e| e.to_string())?;
    getrandom::getrandom(&mut prefix).map_err(|e| e.to_string())?;
    let c = cipher(password, &salt, ITERATIONS)?;
    let mut r = BufReader::new(File::open(src).map_err(|e| e.to_string())?);
    let file = File::create(dst).map_err(|e| e.to_string())?;
    let mut w = BufWriter::new(file);
    let werr = |e: std::io::Error| e.to_string();
    w.write_all(MAGIC).map_err(werr)?;
    w.write_all(&salt).map_err(werr)?;
    w.write_all(&ITERATIONS.to_be_bytes()).map_err(werr)?;
    w.write_all(&prefix).map_err(werr)?;
    // si legge un blocco in anticipo per sapere quale è l'ultimo
    let mut cur = vec![0u8; CHUNK];
    let mut next = vec![0u8; CHUNK];
    let mut n = fill(&mut r, &mut cur)?;
    let mut counter = 0u32;
    loop {
        let m = if n == CHUNK { fill(&mut r, &mut next)? } else { 0 };
        let last = m == 0;
        let ct = c.encrypt(Nonce::from_slice(&nonce(&prefix, counter, last)), &cur[..n]).map_err(|_| "cifratura non riuscita".to_string())?;
        w.write_all(&(ct.len() as u32).to_be_bytes()).map_err(werr)?;
        w.write_all(&ct).map_err(werr)?;
        if last {
            break;
        }
        std::mem::swap(&mut cur, &mut next);
        n = m;
        counter = counter.checked_add(1).ok_or("file troppo grande")?;
    }
    let f = w.into_inner().map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())
}

pub fn decrypt_file(src: &Path, dst: &Path, password: &str) -> Result<(), String> {
    let bad = || "Password errata oppure copia danneggiata".to_string();
    let mut r = BufReader::new(File::open(src).map_err(|e| e.to_string())?);
    let mut head = [0u8; 6];
    r.read_exact(&mut head).map_err(|_| bad())?;
    if &head != MAGIC {
        return Err("Non è una copia cifrata di openFatt".into());
    }
    let mut salt = [0u8; 16];
    let mut iters = [0u8; 4];
    let mut prefix = [0u8; 7];
    r.read_exact(&mut salt).map_err(|_| bad())?;
    r.read_exact(&mut iters).map_err(|_| bad())?;
    r.read_exact(&mut prefix).map_err(|_| bad())?;
    let c = cipher(password, &salt, u32::from_be_bytes(iters))?;
    let mut w = BufWriter::new(File::create(dst).map_err(|e| e.to_string())?);
    let mut counter = 0u32;
    let mut len = [0u8; 4];
    r.read_exact(&mut len).map_err(|_| bad())?;
    loop {
        let size = u32::from_be_bytes(len) as usize;
        if size > CHUNK + 16 {
            return Err(bad());
        }
        let mut ct = vec![0u8; size];
        r.read_exact(&mut ct).map_err(|_| bad())?;
        // c'è un altro blocco? da questo dipende il contrassegno di ultimo blocco
        let more = fill(&mut r, &mut len)?;
        if more != 0 && more != 4 {
            return Err(bad());
        }
        let last = more == 0;
        let pt = c.decrypt(Nonce::from_slice(&nonce(&prefix, counter, last)), ct.as_ref()).map_err(|_| bad())?;
        w.write_all(&pt).map_err(|e| e.to_string())?;
        if last {
            break;
        }
        counter = counter.checked_add(1).ok_or_else(bad)?;
    }
    let f = w.into_inner().map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())
}
