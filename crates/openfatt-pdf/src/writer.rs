//! Scrittore PDF minimale (PDF 1.4) con i font standard Helvetica in WinAnsiEncoding:
//! testo (anche a colori), linee, rettangoli, immagini JPEG e PNG, più pagine, formato
//! della pagina libero (A4 per i documenti, misure dell'etichetta per le etichettatrici).

pub const A4_W: f32 = 595.28;
pub const A4_H: f32 = 841.89;

const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // 32-47
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, // 48-63
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, // 64-79
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, // 80-95
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, // 96-111
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, // 112-126
];

const HELV_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, // 32-47
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, // 48-63
    975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, // 64-79
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556, // 80-95
    333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, // 96-111
    611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584, // 112-126
];

/// Converte in WinAnsiEncoding (cp1252): ASCII, Latin-1 ed euro.
pub fn win_ansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            '€' => 0x80,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            '…' => 0x85,
            '\t' | '\n' | '\r' => b' ',
            c if (c as u32) < 0x20 => b' ',
            c if (c as u32) <= 0xFF => c as u32 as u8,
            _ => b'?',
        })
        .collect()
}

fn base_char_width(b: u8, bold: bool) -> u16 {
    let table = if bold { &HELV_BOLD } else { &HELV };
    let ascii = match b {
        32..=126 => b,
        0x80 => b'0', // euro: larghezza di una cifra
        0x91 | 0x92 => b'\'',
        0x93 | 0x94 => b'"',
        0x96 | 0x97 => b'-',
        0xC0..=0xC5 => b'A',
        0xC7 => b'C',
        0xC8..=0xCB => b'E',
        0xCC..=0xCF => b'I',
        0xD1 => b'N',
        0xD2..=0xD6 | 0xD8 => b'O',
        0xD9..=0xDC => b'U',
        0xE0..=0xE5 => b'a',
        0xE7 => b'c',
        0xE8..=0xEB => b'e',
        0xEC..=0xEF => b'i',
        0xF1 => b'n',
        0xF2..=0xF6 | 0xF8 => b'o',
        0xF9..=0xFC => b'u',
        0xB0 => b'o',
        _ => b'n',
    };
    table[(ascii - 32) as usize]
}

pub fn text_width(s: &str, size: f32, bold: bool) -> f32 {
    win_ansi(s).iter().map(|&b| base_char_width(b, bold) as f32).sum::<f32>() * size / 1000.0
}

/// Spezza un testo in righe che stanno in `width` punti.
pub fn wrap(s: &str, width: f32, size: f32, bold: bool) -> Vec<String> {
    let mut out = Vec::new();
    for para in s.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{} {}", line, word) };
            if text_width(&candidate, size, bold) <= width {
                line = candidate;
            } else {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                // parola più lunga della riga: si spezza a caratteri
                let mut w = String::new();
                for ch in word.chars() {
                    let c = format!("{}{}", w, ch);
                    if text_width(&c, size, bold) > width && !w.is_empty() {
                        out.push(std::mem::take(&mut w));
                        w.push(ch);
                    } else {
                        w = c;
                    }
                }
                line = w;
            }
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn escape_pdf(bytes: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(bytes.len() + 8);
    for &b in bytes {
        match b {
            b'(' | b')' | b'\\' => {
                o.push(b'\\');
                o.push(b);
            }
            _ => o.push(b),
        }
    }
    o
}

/// Immagine pronta per il PDF.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Dati JPEG (DCTDecode) o RGB/grigio compressi (FlateDecode).
    data: Vec<u8>,
    jpeg: bool,
    /// Componenti di colore: 1 grigio, 3 RGB, 4 CMYK.
    components: u8,
    /// Trasparenza (grigio a 8 bit, compresso), per i PNG con canale alfa.
    alpha: Option<Vec<u8>>,
}

impl Image {
    /// Riconosce JPEG e PNG dai primi byte.
    pub fn from_bytes(b: &[u8]) -> Result<Image, String> {
        if b.starts_with(&[0xFF, 0xD8]) {
            Self::jpeg(b)
        } else if b.starts_with(b"\x89PNG") {
            Self::png(b)
        } else {
            Err("formato immagine non supportato (usare JPEG o PNG)".into())
        }
    }

    fn jpeg(b: &[u8]) -> Result<Image, String> {
        // cerca il marcatore SOFn con dimensioni e componenti
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = b[i + 1];
            if (0xC0..=0xCF).contains(&m) && m != 0xC4 && m != 0xC8 && m != 0xCC {
                let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
                let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
                let c = b[i + 9];
                if w == 0 || h == 0 || !matches!(c, 1 | 3 | 4) {
                    return Err("JPEG non valido".into());
                }
                return Ok(Image { width: w, height: h, data: b.to_vec(), jpeg: true, components: c, alpha: None });
            }
            if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) {
                i += 2;
                continue;
            }
            let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
            i += 2 + len;
        }
        Err("JPEG senza dimensioni".into())
    }

    fn png(b: &[u8]) -> Result<Image, String> {
        let mut dec = png::Decoder::new(std::io::Cursor::new(b));
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = dec.read_info().map_err(|e| format!("PNG non valido: {}", e))?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).map_err(|e| format!("PNG non valido: {}", e))?;
        let px = &buf[..info.buffer_size()];
        let (w, h) = (info.width, info.height);
        let (color, alpha): (Vec<u8>, Option<Vec<u8>>) = match info.color_type {
            png::ColorType::Rgb => (px.to_vec(), None),
            png::ColorType::Grayscale => (px.to_vec(), None),
            png::ColorType::Rgba => (px.chunks(4).flat_map(|c| [c[0], c[1], c[2]]).collect(), Some(px.chunks(4).map(|c| c[3]).collect())),
            png::ColorType::GrayscaleAlpha => (px.chunks(2).map(|c| c[0]).collect(), Some(px.chunks(2).map(|c| c[1]).collect())),
            png::ColorType::Indexed => return Err("PNG a tavolozza non espanso".into()),
        };
        let components = if matches!(info.color_type, png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha) { 1 } else { 3 };
        let alpha = alpha.filter(|a| a.iter().any(|&v| v < 255)).map(|a| miniz_oxide::deflate::compress_to_vec_zlib(&a, 6));
        Ok(Image { width: w, height: h, data: miniz_oxide::deflate::compress_to_vec_zlib(&color, 6), jpeg: false, components, alpha })
    }
}

#[derive(Default)]
pub struct Pdf {
    pages: Vec<Vec<u8>>,
    current: Vec<u8>,
    has_page: bool,
    pub title: String,
    w: f32,
    h: f32,
    images: Vec<Image>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Right,
    Center,
}

impl Pdf {
    pub fn new(title: &str) -> Self {
        Self::with_size(title, A4_W, A4_H)
    }

    /// Pagine della misura indicata (punti tipografici, 1 mm = 2,8346 pt).
    pub fn with_size(title: &str, w: f32, h: f32) -> Self {
        let mut p = Pdf { title: title.to_string(), w, h, ..Default::default() };
        p.new_page();
        p
    }

    pub fn width(&self) -> f32 {
        self.w
    }

    pub fn height(&self) -> f32 {
        self.h
    }

    /// Registra un'immagine e restituisce il suo indice (da usare con `image`).
    pub fn add_image(&mut self, img: &Image) -> usize {
        if let Some(i) = self.images.iter().position(|x| x == img) {
            return i;
        }
        self.images.push(img.clone());
        self.images.len() - 1
    }

    /// Disegna un'immagine registrata nel riquadro (x, y in alto a sinistra).
    pub fn image(&mut self, idx: usize, x: f32, y: f32, w: f32, h: f32) {
        self.current.extend(format!("q {:.2} 0 0 {:.2} {:.2} {:.2} cm /Im{} Do Q\n", w, h, x, self.h - y - h, idx + 1).into_bytes());
    }

    /// Testo colorato (r, g, b da 0 a 1).
    pub fn text_rgb(&mut self, x: f32, y: f32, size: f32, bold: bool, s: &str, align: Align, rgb: (f32, f32, f32)) {
        self.current.extend(format!("q {:.3} {:.3} {:.3} rg\n", rgb.0, rgb.1, rgb.2).into_bytes());
        self.text(x, y, size, bold, s, align);
        self.current.extend(b"Q\n");
    }

    /// Rettangolo pieno colorato.
    pub fn rect_rgb(&mut self, x: f32, y: f32, w: f32, h: f32, rgb: (f32, f32, f32)) {
        self.current.extend(format!("q {:.3} {:.3} {:.3} rg {:.2} {:.2} {:.2} {:.2} re f Q\n", rgb.0, rgb.1, rgb.2, x, self.h - y - h, w, h).into_bytes());
    }

    /// Linea colorata.
    pub fn line_rgb(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, rgb: (f32, f32, f32)) {
        self.current.extend(format!("q {:.3} {:.3} {:.3} RG {:.2} w {:.2} {:.2} m {:.2} {:.2} l S Q\n", rgb.0, rgb.1, rgb.2, width, x1, self.h - y1, x2, self.h - y2).into_bytes());
    }

    pub fn new_page(&mut self) {
        if self.has_page {
            self.pages.push(std::mem::take(&mut self.current));
        }
        self.has_page = true;
    }

    pub fn page_count(&self) -> usize {
        self.pages.len() + if self.has_page { 1 } else { 0 }
    }

    /// Testo con origine in alto a sinistra (y cresce verso il basso).
    pub fn text(&mut self, x: f32, y: f32, size: f32, bold: bool, s: &str, align: Align) {
        if s.is_empty() {
            return;
        }
        let w = text_width(s, size, bold);
        let x = match align {
            Align::Left => x,
            Align::Right => x - w,
            Align::Center => x - w / 2.0,
        };
        let font = if bold { "F2" } else { "F1" };
        let mut cmd = format!("BT /{} {:.2} Tf {:.2} {:.2} Td (", font, size, x, self.h - y).into_bytes();
        cmd.extend(escape_pdf(&win_ansi(s)));
        cmd.extend(b") Tj ET\n");
        self.current.extend(cmd);
    }

    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32) {
        self.current
            .extend(format!("{:.2} w {:.2} {:.2} m {:.2} {:.2} l S\n", width, x1, self.h - y1, x2, self.h - y2).into_bytes());
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, gray_fill: Option<f32>, stroke: bool) {
        let mut s = String::new();
        if let Some(g) = gray_fill {
            s.push_str(&format!("{:.3} g ", g));
        }
        s.push_str(&format!("{:.2} {:.2} {:.2} {:.2} re ", x, self.h - y - h, w, h));
        s.push_str(match (gray_fill.is_some(), stroke) {
            (true, true) => "B",
            (true, false) => "f",
            (false, true) => "S",
            (false, false) => "n",
        });
        s.push_str(" 0 g\n");
        self.current.extend(s.into_bytes());
    }

    pub fn finish(mut self) -> Vec<u8> {
        if self.has_page {
            self.pages.push(std::mem::take(&mut self.current));
        }
        let n = self.pages.len();
        // oggetti: 1 catalogo, 2 pagine, 3 F1, 4 F2, 5 info, poi (pagina, contenuto) per ogni pagina
        let mut out: Vec<u8> = Vec::new();
        let mut offsets: Vec<usize> = Vec::new();
        out.extend(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
        let obj = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: Vec<u8>| {
            offsets.push(out.len());
            let id = offsets.len();
            out.extend(format!("{} 0 obj\n", id).into_bytes());
            out.extend(body);
            out.extend(b"\nendobj\n");
        };
        obj(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 6 + i * 2)).collect();
        obj(&mut out, &mut offsets, format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), n).into_bytes());
        obj(&mut out, &mut offsets, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec());
        obj(&mut out, &mut offsets, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".to_vec());
        let mut info = b"<< /Producer (openFatt) /Title (".to_vec();
        info.extend(escape_pdf(&win_ansi(&self.title)));
        info.extend(b") >>");
        obj(&mut out, &mut offsets, info);
        let pages = std::mem::take(&mut self.pages);
        // immagini dopo le pagine: per ciascuna l'oggetto immagine ed eventualmente la maschera
        let mut next = 6 + n * 2;
        let mut image_ids = Vec::new();
        for img in &self.images {
            let id = next;
            next += if img.alpha.is_some() { 2 } else { 1 };
            image_ids.push(id);
        }
        let xobjects: String = image_ids.iter().enumerate().map(|(i, id)| format!("/Im{} {} 0 R ", i + 1, id)).collect();
        let resources = if xobjects.is_empty() {
            "<< /Font << /F1 3 0 R /F2 4 0 R >> >>".to_string()
        } else {
            format!("<< /Font << /F1 3 0 R /F2 4 0 R >> /XObject << {}>> >>", xobjects)
        };
        for (i, content) in pages.into_iter().enumerate() {
            let content_id = 7 + i * 2;
            obj(
                &mut out,
                &mut offsets,
                format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] /Resources {} /Contents {} 0 R >>", self.w, self.h, resources, content_id).into_bytes(),
            );
            let mut body = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
            body.extend(content);
            body.extend(b"\nendstream");
            obj(&mut out, &mut offsets, body);
        }
        for (img, id) in self.images.iter().zip(&image_ids) {
            let cs = match img.components {
                1 => "/DeviceGray",
                4 => "/DeviceCMYK",
                _ => "/DeviceRGB",
            };
            let filter = if img.jpeg { "/DCTDecode" } else { "/FlateDecode" };
            let smask = if img.alpha.is_some() { format!(" /SMask {} 0 R", id + 1) } else { String::new() };
            let decode = if img.jpeg && img.components == 4 { " /Decode [1 0 1 0 1 0 1 0]" } else { "" };
            let mut body = format!(
                "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {} /BitsPerComponent 8 /Filter {}{}{} /Length {} >>\nstream\n",
                img.width,
                img.height,
                cs,
                filter,
                smask,
                decode,
                img.data.len()
            )
            .into_bytes();
            body.extend(&img.data);
            body.extend(b"\nendstream");
            obj(&mut out, &mut offsets, body);
            if let Some(a) = &img.alpha {
                let mut body = format!("<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n", img.width, img.height, a.len()).into_bytes();
                body.extend(a);
                body.extend(b"\nendstream");
                obj(&mut out, &mut offsets, body);
            }
        }
        let xref_pos = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).into_bytes());
        for o in &offsets {
            out.extend(format!("{:010} 00000 n \n", o).into_bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R /Info 5 0 R >>\nstartxref\n{}\n%%EOF\n", offsets.len() + 1, xref_pos).into_bytes());
        out
    }
}
