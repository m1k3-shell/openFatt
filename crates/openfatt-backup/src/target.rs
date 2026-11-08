//! Destinazioni: cartella locale (disco esterno, cartella sincronizzata) e WebDAV.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;

use crate::{sha256_file, Dest, DestKind};

pub(crate) trait Target {
    fn ensure_dir(&self) -> Result<(), String>;
    fn list(&self) -> Result<Vec<String>, String>;
    /// Scrive sotto nome provvisorio, verifica l'impronta, poi dà il nome definitivo.
    fn put(&self, local: &Path, name: &str, sha: &str) -> Result<(), String>;
    fn put_small(&self, name: &str, data: &[u8]) -> Result<(), String>;
    fn get_to(&self, name: &str, local: &Path) -> Result<(), String>;
    fn get_small(&self, name: &str) -> Result<Vec<u8>, String>;
    fn delete(&self, name: &str) -> Result<(), String>;
    /// Toglie i file provvisori rimasti da copie interrotte.
    fn cleanup_partial(&self);
    /// Elimina la cartella (solo per la prova di una destinazione nuova).
    fn remove_self(&self) -> Result<(), String>;
}

pub(crate) fn open_target(dest: &Dest, secret: Option<&str>, folder: &str) -> Result<Box<dyn Target>, String> {
    match dest.kind {
        DestKind::Folder | DestKind::CloudFolder => Ok(Box::new(FolderTarget { dir: Path::new(&dest.path).join(folder) })),
        DestKind::WebDav => Ok(Box::new(WebDavTarget::new(dest, secret, folder)?)),
    }
}

struct FolderTarget {
    dir: PathBuf,
}

impl Target for FolderTarget {
    fn ensure_dir(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {}", self.dir.display(), e))
    }

    fn list(&self) -> Result<Vec<String>, String> {
        let rd = std::fs::read_dir(&self.dir).map_err(|e| format!("{}: {}", self.dir.display(), e))?;
        Ok(rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect())
    }

    fn put(&self, local: &Path, name: &str, sha: &str) -> Result<(), String> {
        let part = self.dir.join(format!("{}.part", name));
        let done = (|| -> Result<(), String> {
            let mut r = BufReader::with_capacity(1 << 20, File::open(local).map_err(|e| e.to_string())?);
            let mut w = BufWriter::with_capacity(1 << 20, File::create(&part).map_err(|e| format!("{}: {}", part.display(), e))?);
            std::io::copy(&mut r, &mut w).map_err(|e| format!("Scrittura non riuscita: {}", e))?;
            // i dati devono essere davvero sul disco prima di fidarsi della copia
            let f = w.into_inner().map_err(|e| format!("Scrittura non riuscita: {}", e))?;
            f.sync_all().map_err(|e| format!("Scrittura non riuscita: {}", e))?;
            drop(f);
            if sha256_file(&part)? != sha {
                return Err("La copia riletta dal disco è diversa dall'originale".into());
            }
            std::fs::rename(&part, self.dir.join(name)).map_err(|e| e.to_string())
        })();
        if done.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        done
    }

    fn put_small(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let part = self.dir.join(format!("{}.part", name));
        let mut f = File::create(&part).map_err(|e| e.to_string())?;
        f.write_all(data).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
        drop(f);
        std::fs::rename(&part, self.dir.join(name)).map_err(|e| e.to_string())
    }

    fn get_to(&self, name: &str, local: &Path) -> Result<(), String> {
        std::fs::copy(self.dir.join(name), local).map(|_| ()).map_err(|e| format!("{}: {}", name, e))
    }

    fn get_small(&self, name: &str) -> Result<Vec<u8>, String> {
        std::fs::read(self.dir.join(name)).map_err(|e| e.to_string())
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        std::fs::remove_file(self.dir.join(name)).map_err(|e| e.to_string())
    }

    fn cleanup_partial(&self) {
        for n in self.list().unwrap_or_default() {
            if n.ends_with(".part") {
                let _ = std::fs::remove_file(self.dir.join(n));
            }
        }
    }

    fn remove_self(&self) -> Result<(), String> {
        std::fs::remove_dir(&self.dir).map_err(|e| e.to_string())
    }
}

struct WebDavTarget {
    agent: ureq::Agent,
    root: String,
    dir: String,
    auth: String,
    backoff_ms: u64,
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

impl WebDavTarget {
    fn new(dest: &Dest, secret: Option<&str>, folder: &str) -> Result<Self, String> {
        let url = dest.url.trim().trim_end_matches('/').to_string();
        let local = url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost");
        if !url.starts_with("https://") && !local {
            return Err("Per il cloud serve un indirizzo https:// (i dati non viaggiano in chiaro)".into());
        }
        let auth = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", dest.user, secret.unwrap_or(""))));
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(20)).timeout_read(Duration::from_secs(120)).timeout_write(Duration::from_secs(300)).redirects(0).build();
        Ok(WebDavTarget { agent, root: url.clone(), dir: format!("{}/{}", url, encode(folder)), auth, backoff_ms: if local { 5 } else { 1500 } })
    }

    fn url(&self, name: &str) -> String {
        format!("{}/{}", self.dir, encode(name))
    }

    fn explain(e: ureq::Error) -> String {
        match e {
            ureq::Error::Status(401, _) | ureq::Error::Status(403, _) => "Accesso al cloud rifiutato: controllare utente e password".to_string(),
            ureq::Error::Status(507, _) => "Spazio esaurito nel cloud".to_string(),
            ureq::Error::Status(c, r) => format!("Il cloud ha risposto {} {}", c, r.status_text()),
            ureq::Error::Transport(t) => format!("Cloud non raggiungibile: {}", t),
        }
    }

    /// Riprova gli errori temporanei (rete, server occupato).
    fn retry<T>(&self, mut f: impl FnMut() -> Result<T, ureq::Error>) -> Result<T, String> {
        let mut wait = self.backoff_ms;
        for attempt in 0..3 {
            match f() {
                Ok(v) => return Ok(v),
                Err(ureq::Error::Status(c, _)) if (500..600).contains(&c) && c != 507 && attempt < 2 => {}
                Err(ureq::Error::Transport(_)) if attempt < 2 => {}
                Err(e) => return Err(Self::explain(e)),
            }
            std::thread::sleep(Duration::from_millis(wait));
            wait *= 3;
        }
        Err("Cloud non raggiungibile".into())
    }

    fn put_file(&self, local: &Path, url: &str) -> Result<(), String> {
        let size = std::fs::metadata(local).map_err(|e| e.to_string())?.len();
        self.retry(|| {
            let f = File::open(local).map_err(|e| ureq::Error::Transport(ureq::Error::from(e).into_transport().unwrap()))?;
            self.agent.put(url).set("Authorization", &self.auth).set("Content-Type", "application/octet-stream").set("Content-Length", &size.to_string()).send(BufReader::with_capacity(1 << 20, f))
        })
        .map(|_| ())
    }

    fn download(&self, url: &str, local: &Path) -> Result<(), String> {
        let resp = self.retry(|| self.agent.get(url).set("Authorization", &self.auth).call())?;
        let mut r = resp.into_reader();
        let mut w = BufWriter::new(File::create(local).map_err(|e| e.to_string())?);
        std::io::copy(&mut r, &mut w).map_err(|e| format!("Scaricamento interrotto: {}", e))?;
        w.flush().map_err(|e| e.to_string())
    }
}

impl Target for WebDavTarget {
    fn ensure_dir(&self) -> Result<(), String> {
        // la cartella radice deve già esistere o essere creabile; 405 = c'è già
        for u in [&self.root, &self.dir] {
            match self.agent.request("MKCOL", u).set("Authorization", &self.auth).call() {
                Ok(_) | Err(ureq::Error::Status(405, _)) | Err(ureq::Error::Status(301, _)) => {}
                Err(ureq::Error::Status(409, _)) if u == &self.root => {}
                Err(e) => return Err(Self::explain(e)),
            }
        }
        Ok(())
    }

    fn list(&self) -> Result<Vec<String>, String> {
        let resp = self.retry(|| self.agent.request("PROPFIND", &format!("{}/", self.dir)).set("Authorization", &self.auth).set("Depth", "1").set("Content-Type", "application/xml").send_string("<?xml version=\"1.0\"?><propfind xmlns=\"DAV:\"><prop><resourcetype/></prop></propfind>"))?;
        let body = resp.into_string().map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        let lower = body.to_lowercase();
        let mut pos = 0;
        while let Some(i) = lower[pos..].find("href>") {
            let start = pos + i + 5;
            let Some(j) = body[start..].find("</") else { break };
            let href = body[start..start + j].trim();
            if !href.ends_with('/') && !href.is_empty() {
                if let Some(n) = href.rsplit('/').next() {
                    out.push(decode(n));
                }
            }
            pos = start + j + 2;
            // salta il tag di chiusura per non contarlo come apertura
            if let Some(k) = lower[pos..].find('>') {
                pos += k + 1;
            }
        }
        Ok(out)
    }

    fn put(&self, local: &Path, name: &str, sha: &str) -> Result<(), String> {
        let part = self.url(&format!("{}.part", name));
        self.put_file(local, &part)?;
        let check = std::env::temp_dir().join(format!("openfatt-verifica-{}.bin", crate::new_id()));
        let verified = self.download(&part, &check).and_then(|_| sha256_file(&check));
        let _ = std::fs::remove_file(&check);
        match verified {
            Ok(h) if h == sha => {}
            Ok(_) => {
                let _ = self.delete(&format!("{}.part", name));
                return Err("Il file riscaricato dal cloud è diverso dall'originale".into());
            }
            Err(e) => {
                let _ = self.delete(&format!("{}.part", name));
                return Err(e);
            }
        }
        let dest = self.url(name);
        self.retry(|| self.agent.request("MOVE", &part).set("Authorization", &self.auth).set("Destination", &dest).set("Overwrite", "T").call()).map(|_| ())
    }

    fn put_small(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let u = self.url(name);
        self.retry(|| self.agent.put(&u).set("Authorization", &self.auth).send_bytes(data)).map(|_| ())
    }

    fn get_to(&self, name: &str, local: &Path) -> Result<(), String> {
        self.download(&self.url(name), local)
    }

    fn get_small(&self, name: &str) -> Result<Vec<u8>, String> {
        let resp = self.retry(|| self.agent.get(&self.url(name)).set("Authorization", &self.auth).call())?;
        let mut v = Vec::new();
        resp.into_reader().take(4 << 20).read_to_end(&mut v).map_err(|e| e.to_string())?;
        Ok(v)
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        match self.agent.delete(&self.url(name)).set("Authorization", &self.auth).call() {
            Ok(_) | Err(ureq::Error::Status(404, _)) => Ok(()),
            Err(e) => Err(Self::explain(e)),
        }
    }

    fn cleanup_partial(&self) {
        for n in self.list().unwrap_or_default() {
            if n.ends_with(".part") {
                let _ = self.delete(&n);
            }
        }
    }

    fn remove_self(&self) -> Result<(), String> {
        let _ = self.agent.delete(&format!("{}/", self.dir)).set("Authorization", &self.auth).call();
        Ok(())
    }
}
