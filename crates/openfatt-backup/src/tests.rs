//! Prove sui guasti: disco staccato, copia interrotta, file alterati, cloud che sbaglia.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use chrono::{Duration, NaiveDate};

use super::*;

fn t0() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 11, 8).unwrap().and_hms_opt(10, 0, 0).unwrap()
}

struct Env {
    dir: PathBuf,
    source: PathBuf,
}

impl Env {
    fn new(name: &str) -> Env {
        let dir = std::env::temp_dir().join(format!("openfatt-bk-{}-{}", name, new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("azienda.gadb");
        let s = openfatt_store::Store::open(&source).unwrap();
        s.set_setting("prova", &"valore").unwrap();
        Env { dir, source }
    }
    fn folder(&self, name: &str) -> Dest {
        let d = Dest { id: new_id(), name: name.into(), kind: DestKind::Folder, path: self.dir.join(name).to_string_lossy().to_string(), enabled: true, ..Default::default() };
        prepare_and_test(&d, None, &self.dir.join("spool")).unwrap();
        d
    }
    fn job(&self, dests: Vec<JobDest>, now: NaiveDateTime) -> JobResult {
        let input = JobInput { source: self.source.clone(), stem: "azienda".into(), uid: "abcdef1234".into(), spool: self.dir.join("spool"), retention: Retention::default(), dests, now, app_version: "t".into() };
        run_job(input, &Arc::new(Mutex::new(String::new())))
    }
    fn touch(&self, v: &str) {
        openfatt_store::Store::open(&self.source).unwrap().set_setting("prova", &v).unwrap();
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn jd(d: &Dest) -> JobDest {
    JobDest { dest: d.clone(), password: None, secret: None, state: DestState::default() }
}

#[test]
fn copia_su_disco_verificata_e_ripristinabile() {
    let e = Env::new("base");
    let d = e.folder("disco");
    let r = e.job(vec![jd(&d)], t0());
    assert!(r.fatal.is_none());
    let o = &r.outcomes[0];
    assert_eq!(o.status, Status::Saved, "{}", o.message);
    assert!(o.state.last_ok.is_some());
    let list = list_backups(&d, None, "azienda", "abcdef1234").unwrap();
    assert_eq!(list.len(), 1);
    // file provvisori spariti, copia con il nome finale
    let dir = Path::new(&d.path).join(archive_folder("azienda", "abcdef1234"));
    assert!(dir.join(&list[0].file).exists());
    assert!(!dir.join(format!("{}.part", list[0].file)).exists());
    let out = e.dir.join("ripristinato.gadb");
    restore(&d, None, None, "azienda", "abcdef1234", &list[0], &out, &e.dir.join("spool")).unwrap();
    let s = openfatt_store::Store::open(&out).unwrap();
    assert_eq!(s.get_setting::<String>("prova").unwrap(), "valore");
    // mai sopra un file esistente
    assert!(restore(&d, None, None, "azienda", "abcdef1234", &list[0], &out, &e.dir.join("spool")).is_err());
}

#[test]
fn senza_modifiche_non_si_duplica() {
    let e = Env::new("skip");
    let d = e.folder("disco");
    let first = e.job(vec![jd(&d)], t0());
    let again = JobDest { state: first.outcomes[0].state.clone(), ..jd(&d) };
    let r = e.job(vec![again], t0() + Duration::hours(2));
    assert_eq!(r.outcomes[0].status, Status::Unchanged);
    assert_eq!(list_backups(&d, None, "azienda", "abcdef1234").unwrap().len(), 1);
}

#[test]
fn disco_staccato_non_blocca_le_altre_destinazioni() {
    let e = Env::new("stacc");
    let buono = e.folder("buono");
    let staccato = e.folder("staccato");
    std::fs::remove_dir_all(&staccato.path).unwrap();
    let r = e.job(vec![jd(&staccato), jd(&buono)], t0());
    assert_eq!(r.outcomes[0].status, Status::Unreachable);
    assert!(!r.outcomes[0].state.last_error.is_empty());
    assert_eq!(r.outcomes[1].status, Status::Saved);
    assert!(!reachable(&staccato));
}

#[test]
fn su_un_disco_diverso_non_si_scrive() {
    let e = Env::new("diverso");
    let d = e.folder("disco");
    // al posto del disco giusto c'è un'altra chiavetta con la stessa lettera e cartella
    std::fs::remove_file(Path::new(&d.path).join(MARKER)).unwrap();
    let r = e.job(vec![jd(&d)], t0());
    assert_eq!(r.outcomes[0].status, Status::Unreachable);
    assert!(std::fs::read_dir(&d.path).unwrap().next().is_none() || !Path::new(&d.path).join(archive_folder("azienda", "abcdef1234")).join("x").exists());
    let n = std::fs::read_dir(Path::new(&d.path).join(archive_folder("azienda", "abcdef1234"))).map(|r| r.count()).unwrap_or(0);
    assert_eq!(n, 0);
}

#[test]
fn archivio_danneggiato_non_tocca_le_copie_buone() {
    let e = Env::new("danno");
    let d = e.folder("disco");
    e.job(vec![jd(&d)], t0());
    let good = list_backups(&d, None, "azienda", "abcdef1234").unwrap();
    // l'archivio diventa spazzatura
    for s in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", e.source.display(), s));
    }
    std::fs::write(&e.source, vec![0x55u8; 8192]).unwrap();
    let r = e.job(vec![jd(&d)], t0() + Duration::days(1));
    assert!(r.fatal.is_some(), "la copia di un archivio rotto va rifiutata");
    assert_eq!(list_backups(&d, None, "azienda", "abcdef1234").unwrap(), good);
}

#[test]
fn copia_interrotta_viene_ignorata_e_ripulita() {
    let e = Env::new("part");
    let d = e.folder("disco");
    e.job(vec![jd(&d)], t0());
    let dir = Path::new(&d.path).join(archive_folder("azienda", "abcdef1234"));
    let part = dir.join("azienda 2026-11-09 10-00-00.gadb.part");
    std::fs::write(&part, b"mezza copia").unwrap();
    assert_eq!(list_backups(&d, None, "azienda", "abcdef1234").unwrap().len(), 1);
    e.touch("nuovo");
    let r = e.job(vec![jd(&d)], t0() + Duration::hours(3));
    assert_eq!(r.outcomes[0].status, Status::Saved);
    assert!(!part.exists());
    assert_eq!(list_backups(&d, None, "azienda", "abcdef1234").unwrap().len(), 2);
}

#[test]
fn copia_alterata_non_si_ripristina() {
    let e = Env::new("alter");
    let d = e.folder("disco");
    e.job(vec![jd(&d)], t0());
    let m = list_backups(&d, None, "azienda", "abcdef1234").unwrap().remove(0);
    let f = Path::new(&d.path).join(archive_folder("azienda", "abcdef1234")).join(&m.file);
    let mut b = std::fs::read(&f).unwrap();
    let n = b.len() / 2;
    b[n] ^= 0xff;
    std::fs::write(&f, b).unwrap();
    let out = e.dir.join("no.gadb");
    assert!(restore(&d, None, None, "azienda", "abcdef1234", &m, &out, &e.dir.join("spool")).is_err());
    assert!(!out.exists());
}

#[test]
fn controllo_periodico_scopre_una_copia_danneggiata() {
    let e = Env::new("scrub");
    let d = e.folder("disco");
    let mut st = e.job(vec![jd(&d)], t0()).outcomes[0].state.clone();
    let m = list_backups(&d, None, "azienda", "abcdef1234").unwrap().remove(0);
    let f = Path::new(&d.path).join(archive_folder("azienda", "abcdef1234")).join(&m.file);
    let mut b = std::fs::read(&f).unwrap();
    b[0] ^= 1;
    std::fs::write(&f, b).unwrap();
    e.touch("altro");
    let r = e.job(vec![JobDest { state: st.clone(), ..jd(&d) }], t0() + Duration::days(1));
    st = r.outcomes[0].state.clone();
    assert!(st.warning.contains("danneggiata"), "{}", st.warning);
    assert_eq!(list_backups(&d, None, "azienda", "abcdef1234").unwrap().len(), 1, "non conta più come copia buona");
}

#[test]
fn calo_improvviso_dei_dati_non_elimina_nulla() {
    let e = Env::new("calo");
    let d = e.folder("disco");
    e.job(vec![jd(&d)], t0());
    let dir = Path::new(&d.path).join(archive_folder("azienda", "abcdef1234"));
    // la copia precedente aveva molti documenti
    let name = std::fs::read_dir(&dir).unwrap().flatten().map(|x| x.path()).find(|p| p.extension().map(|e| e == "json").unwrap_or(false)).unwrap();
    let mut m: Manifest = serde_json::from_slice(&std::fs::read(&name).unwrap()).unwrap();
    m.counts.insert("documents".into(), 500);
    std::fs::write(&name, serde_json::to_vec(&m).unwrap()).unwrap();
    e.touch("x");
    let r = e.job(vec![jd(&d)], t0() + Duration::hours(3));
    assert!(r.outcomes[0].state.warning.contains("scesi"));
}

#[test]
fn rotazione_tiene_giorni_settimane_mesi_anni() {
    let r = Retention { recent: 3, daily: 7, weekly: 4, monthly: 6, yearly: 3 };
    let dates: Vec<NaiveDateTime> = (0..1200).map(|i| t0() - Duration::hours(i * 12)).collect();
    let keep = select_keep(&dates, &r);
    assert!(keep.contains(&0), "la più recente resta sempre");
    assert!(keep.len() < 40, "{}", keep.len());
    let ks: Vec<NaiveDateTime> = keep.iter().map(|i| dates[*i]).collect();
    let oldest = ks.iter().min().unwrap();
    assert!((t0() - *oldest).num_days() > 365, "resta almeno una copia di oltre un anno fa");
    let last_days: BTreeSet<_> = ks.iter().filter(|d| (t0() - **d).num_days() < 7).map(|d| d.date()).collect();
    assert!(last_days.len() >= 7);
    // poche copie: nessuna si perde
    let few: Vec<NaiveDateTime> = (0..3).map(|i| t0() - Duration::hours(i)).collect();
    assert_eq!(select_keep(&few, &Retention::default()).len(), 3);
}

#[test]
fn cifratura_protegge_e_rileva_alterazioni() {
    let e = Env::new("cifra");
    let src = e.dir.join("dati.bin");
    // oltre un blocco, e un caso esattamente pari a un blocco
    for len in [0usize, 5, 1 << 20, (3 << 20) + 17] {
        let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        let enc = e.dir.join("dati.enc");
        let out = e.dir.join("dati.out");
        encrypt_file(&src, &enc, "segreta").unwrap();
        decrypt_file(&enc, &out, "segreta").unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), data);
        assert!(decrypt_file(&enc, &out, "sbagliata").is_err());
        if len > 0 {
            let raw = std::fs::read(&enc).unwrap();
            assert!(!raw.windows(5.min(len)).any(|w| w == &data[..5.min(len)]) || len < 5);
            // file troncato a un confine di blocco o alterato
            let mut bad = raw.clone();
            let n = bad.len() - 1;
            bad[n] ^= 1;
            std::fs::write(&enc, &bad).unwrap();
            assert!(decrypt_file(&enc, &out, "segreta").is_err());
            if len > 1 << 20 {
                let cut = 6 + 16 + 4 + 7 + 4 + (1 << 20) + 16;
                std::fs::write(&enc, &raw[..cut]).unwrap();
                assert!(decrypt_file(&enc, &out, "segreta").is_err(), "troncato dopo il primo blocco");
            }
        }
    }
}

#[test]
fn copia_cifrata_su_disco_e_ripristino_da_file() {
    let e = Env::new("cifdisco");
    let mut d = e.folder("disco");
    d.encrypt = true;
    let r = e.job(vec![JobDest { password: Some("parola".into()), ..jd(&d) }], t0());
    assert_eq!(r.outcomes[0].status, Status::Saved, "{}", r.outcomes[0].message);
    let m = list_backups(&d, None, "azienda", "abcdef1234").unwrap().remove(0);
    assert!(m.encrypted && m.file.ends_with(".enc"));
    assert_ne!(m.sha256, m.plain_sha256);
    let f = Path::new(&d.path).join(archive_folder("azienda", "abcdef1234")).join(&m.file);
    // computer nuovo: si parte dal solo file
    let out = e.dir.join("nuovo.gadb");
    assert!(restore_file(&f, Some("altra"), &out).is_err());
    assert!(!out.exists());
    restore_file(&f, Some("parola"), &out).unwrap();
    assert_eq!(openfatt_store::Store::open(&out).unwrap().get_setting::<String>("prova").unwrap(), "valore");
    // senza password la copia non parte e l'errore si vede
    let r = e.job(vec![jd(&d)], t0() + Duration::hours(4));
    assert_eq!(r.outcomes[0].status, Status::Failed);
}

// ---- server WebDAV minimo per provare il percorso completo

#[derive(Default)]
struct Dav {
    files: HashMap<String, Vec<u8>>,
    dirs: Vec<String>,
    /// quante risposte PUT fallire con 503
    fail_puts: usize,
    /// rende diverso ciò che si riscarica
    corrupt_get: bool,
    /// il server rifiuta le credenziali
    need_auth: bool,
}

fn serve(s: Arc<Mutex<Dav>>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            let s = s.clone();
            std::thread::spawn(move || handle(c, s));
        }
    });
    format!("http://127.0.0.1:{}/dav", addr.port())
}

fn handle(mut c: TcpStream, s: Arc<Mutex<Dav>>) {
    let mut r = BufReader::new(c.try_clone().unwrap());
    let mut line = String::new();
    if r.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }
    let mut it = line.split_whitespace();
    let (method, path) = (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string());
    let mut len = 0usize;
    let mut dest = String::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h).unwrap();
        let t = h.trim();
        if t.is_empty() {
            break;
        }
        let low = t.to_lowercase();
        if let Some(v) = low.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
        if low.starts_with("destination:") {
            dest = t[12..].trim().to_string();
        }
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).unwrap();
    let mut g = s.lock().unwrap();
    let mut send = |code: u32, data: &[u8]| {
        let _ = write!(c, "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", code, data.len());
        let _ = c.write_all(data);
    };
    if g.need_auth {
        return send(401, b"");
    }
    let key = path.trim_end_matches('/').to_string();
    match method.as_str() {
        "MKCOL" => {
            if g.dirs.contains(&key) {
                send(405, b"");
            } else {
                g.dirs.push(key);
                send(201, b"");
            }
        }
        "PUT" => {
            if g.fail_puts > 0 {
                g.fail_puts -= 1;
                send(503, b"");
            } else {
                g.files.insert(key, body);
                send(201, b"");
            }
        }
        "GET" => match g.files.get(&key).cloned() {
            Some(mut d) => {
                if g.corrupt_get && !key.ends_with(".json") && !d.is_empty() {
                    d[0] ^= 1;
                }
                send(200, &d);
            }
            None => send(404, b""),
        },
        "DELETE" => {
            g.files.remove(&key);
            send(204, b"");
        }
        "MOVE" => {
            let target = dest.split_once("/dav").map(|(_, b)| format!("/dav{}", b)).unwrap_or(dest);
            match g.files.remove(&key) {
                Some(d) => {
                    g.files.insert(target, d);
                    send(201, b"");
                }
                None => send(404, b""),
            }
        }
        "PROPFIND" => {
            let mut x = format!("<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>{}/</d:href></d:response>", key);
            for k in g.files.keys().filter(|k| k.starts_with(&format!("{}/", key))) {
                x.push_str(&format!("<d:response><d:href>{}</d:href></d:response>", k));
            }
            x.push_str("</d:multistatus>");
            send(207, x.as_bytes());
        }
        _ => send(400, b""),
    }
}

fn dav_dest(url: &str) -> Dest {
    Dest { id: new_id(), name: "Cloud".into(), kind: DestKind::WebDav, url: url.into(), user: "u".into(), enabled: true, ..Default::default() }
}

#[test]
fn webdav_percorso_completo() {
    let e = Env::new("dav");
    let srv = Arc::new(Mutex::new(Dav::default()));
    let d = dav_dest(&serve(srv.clone()));
    prepare_and_test(&d, Some("p"), &e.dir.join("spool")).unwrap();
    assert!(srv.lock().unwrap().files.is_empty(), "la prova non lascia file");
    let r = e.job(vec![JobDest { secret: Some("p".into()), ..jd(&d) }], t0());
    assert_eq!(r.outcomes[0].status, Status::Saved, "{}", r.outcomes[0].message);
    assert!(srv.lock().unwrap().files.keys().all(|k| !k.ends_with(".part")));
    let list = list_backups(&d, Some("p"), "azienda", "abcdef1234").unwrap();
    assert_eq!(list.len(), 1);
    let out = e.dir.join("dalcloud.gadb");
    restore(&d, Some("p"), None, "azienda", "abcdef1234", &list[0], &out, &e.dir.join("spool")).unwrap();
    assert_eq!(openfatt_store::Store::open(&out).unwrap().get_setting::<String>("prova").unwrap(), "valore");
}

#[test]
fn webdav_riprova_gli_errori_temporanei() {
    let e = Env::new("davretry");
    let srv = Arc::new(Mutex::new(Dav { fail_puts: 2, ..Default::default() }));
    let d = dav_dest(&serve(srv.clone()));
    let r = e.job(vec![JobDest { secret: Some("p".into()), ..jd(&d) }], t0());
    assert_eq!(r.outcomes[0].status, Status::Saved, "{}", r.outcomes[0].message);
}

#[test]
fn webdav_scarta_un_caricamento_alterato() {
    let e = Env::new("davbad");
    let srv = Arc::new(Mutex::new(Dav { corrupt_get: true, ..Default::default() }));
    let d = dav_dest(&serve(srv.clone()));
    let r = e.job(vec![JobDest { secret: Some("p".into()), ..jd(&d) }], t0());
    assert_eq!(r.outcomes[0].status, Status::Failed);
    assert!(r.outcomes[0].state.last_ok.is_none());
    assert!(srv.lock().unwrap().files.keys().all(|k| !k.ends_with(".gadb")), "nessuna copia col nome definitivo");
}

#[test]
fn webdav_password_errata_si_dice_chiaramente() {
    let e = Env::new("davauth");
    let srv = Arc::new(Mutex::new(Dav { need_auth: true, ..Default::default() }));
    let d = dav_dest(&serve(srv));
    let r = e.job(vec![JobDest { secret: Some("p".into()), ..jd(&d) }], t0());
    assert_eq!(r.outcomes[0].status, Status::Failed);
    assert!(r.outcomes[0].message.contains("password"), "{}", r.outcomes[0].message);
}

#[test]
fn webdav_rifiuta_http_in_chiaro() {
    let e = Env::new("davhttp");
    let d = dav_dest("http://esempio.it/dav");
    let r = e.job(vec![JobDest { secret: Some("p".into()), ..jd(&d) }], t0());
    assert_eq!(r.outcomes[0].status, Status::Failed);
    assert!(r.outcomes[0].message.contains("https"));
}
