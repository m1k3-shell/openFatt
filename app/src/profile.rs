//! Misura dei tempi: con la variabile `OPENFATT_PROFILO=<file>` ogni fotogramma viene
//! annotato nel file (e la finestra si ridisegna di continuo), con il nome della schermata. Serve solo per i collaudi
//! di velocità; senza la variabile non fa nulla (un solo controllo, memorizzato).

use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

fn target() -> &'static Option<std::path::PathBuf> {
    static T: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    T.get_or_init(|| std::env::var_os("OPENFATT_PROFILO").map(std::path::PathBuf::from))
}

pub fn active() -> bool {
    target().is_some()
}

pub struct Frame {
    start: Option<(Instant, String)>,
}

impl Frame {
    pub fn start(name: String) -> Frame {
        Frame { start: target().as_ref().map(|_| (Instant::now(), name)) }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        if let (Some((t, name)), Some(path)) = (self.start.take(), target()) {
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(f, "{:.1}\t{}", ms, name);
                }
            }
        }
    }
}
