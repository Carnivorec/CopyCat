pub const MIME_TEXT: &str = "text/plain";
pub const MIME_HTML: &str = "text/html";
pub const MIME_PNG: &str = "image/png";

pub const MIME_FILES: &str = "application/x-copycat-files";

pub const MAX_PIXELS: u64 = 100_000_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub reps: Vec<(String, Vec<u8>)>,

    pub sensitive: bool,

    pub ours: bool,

    pub files: Vec<String>,

    pub owner_exe: Option<String>,
}

pub fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

pub fn to_crlf(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 32);
    let mut prev = '\0';
    for c in s.chars() {
        if c == '\n' && prev != '\r' {
            out.push('\r');
        }
        out.push(c);
        prev = c;
    }
    out
}

pub fn dropfiles_build(paths: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&20u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 12]);
    out.extend_from_slice(&1u32.to_le_bytes());
    for p in paths {
        out.extend(p.encode_utf16().chain(Some(0)).flat_map(u16::to_le_bytes));
    }
    out.extend_from_slice(&[0, 0]);
    out
}

fn header_value(header: &str, key: &str) -> Option<usize> {
    header.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':')?.trim().parse().ok())
}

pub fn cf_html_extract(raw: &[u8]) -> Option<String> {
    let raw = raw.split(|b| *b == 0).next()?;
    let head_len = raw.len().min(512);
    let header = String::from_utf8_lossy(&raw[..head_len]);
    let (start, end) = match (header_value(&header, "StartFragment"), header_value(&header, "EndFragment")) {
        (Some(s), Some(e)) => (s, e),
        _ => (header_value(&header, "StartHTML")?, header_value(&header, "EndHTML")?),
    };
    if start > end || end > raw.len() {
        return None;
    }
    String::from_utf8(raw[start..end].to_vec()).ok()
}

pub fn cf_html_build(fragment: &str) -> Vec<u8> {
    let owned;
    let fragment = if fragment.contains('\0') {
        owned = fragment.replace('\0', "");
        owned.as_str()
    } else {
        fragment
    };
    const PRE: &str = "<html><body>\r\n<!--StartFragment-->";
    const POST: &str = "<!--EndFragment-->\r\n</body></html>";

    let header_template = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let start_html = header_template.len();
    let start_frag = start_html + PRE.len();
    let end_frag = start_frag + fragment.len();
    let end_html = end_frag + POST.len();
    let header = format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_frag:010}\r\nEndFragment:{end_frag:010}\r\n"
    );
    debug_assert_eq!(header.len(), start_html);
    let mut out = Vec::with_capacity(end_html + 1);
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(PRE.as_bytes());
    out.extend_from_slice(fragment.as_bytes());
    out.extend_from_slice(POST.as_bytes());
    out.push(0);
    out
}

fn rd_u16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn rd_u32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn rd_i32(b: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
const BI_ALPHABITFIELDS: u32 = 6;

fn mask_shift(mask: u32) -> (u32, u32) {
    if mask == 0 {
        return (0, 0);
    }
    let shift = mask.trailing_zeros();
    (mask, shift)
}

fn extract(px: u32, (mask, shift): (u32, u32)) -> u8 {
    if mask == 0 {
        return 0;
    }
    let v = u64::from((px & mask) >> shift);

    let span = 32 - (mask >> shift).leading_zeros();
    if span >= 8 { (v >> (span - 8)).min(255) as u8 } else { (v * 255 / ((1u64 << span) - 1)).min(255) as u8 }
}

pub fn dib_to_rgba(dib: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let hsize = rd_u32(dib, 0)? as usize;
    if hsize < 40 || hsize > dib.len() {
        return None;
    }
    let width = rd_i32(dib, 4)?;
    let height = rd_i32(dib, 8)?;
    let bitcount = rd_u16(dib, 14)?;
    let compression = rd_u32(dib, 16)?;
    let clr_used = rd_u32(dib, 32)?;
    if width <= 0 || height == 0 || height == i32::MIN {
        return None;
    }
    let (w, h) = (width as u32, height.unsigned_abs());
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return None;
    }
    let top_down = height < 0;

    let (masks, mut data_off) = match compression {
        BI_RGB => (None, hsize),
        BI_BITFIELDS | BI_ALPHABITFIELDS => {
            if hsize >= 56 {
                (Some([rd_u32(dib, 40)?, rd_u32(dib, 44)?, rd_u32(dib, 48)?, rd_u32(dib, 52)?]), hsize)
            } else {
                let n = if compression == BI_ALPHABITFIELDS { 4 } else { 3 };
                let a = if n == 4 { rd_u32(dib, hsize + 12)? } else { 0 };
                (Some([rd_u32(dib, hsize)?, rd_u32(dib, hsize + 4)?, rd_u32(dib, hsize + 8)?, a]), hsize + n * 4)
            }
        }
        _ => return None,
    };
    if !matches!(bitcount, 24 | 32) {
        return None;
    }

    data_off += clr_used as usize * 4;

    let stride = (w as usize * bitcount as usize).div_ceil(32) * 4;
    let needed = stride.checked_mul(h as usize)?;
    let pixels = dib.get(data_off..data_off.checked_add(needed)?)?;

    let mut rgba = vec![0u8; w as usize * h as usize * 4];
    let mut any_alpha = false;
    let ms = masks.map(|m| [mask_shift(m[0]), mask_shift(m[1]), mask_shift(m[2]), mask_shift(m[3])]);
    for y in 0..h as usize {
        let src_row = if top_down { y } else { h as usize - 1 - y };
        let row = &pixels[src_row * stride..src_row * stride + stride];
        for x in 0..w as usize {
            let o = (y * w as usize + x) * 4;
            let (r, g, b, a) = if bitcount == 24 {
                let p = &row[x * 3..x * 3 + 3];
                (p[2], p[1], p[0], 255)
            } else {
                let p = u32::from_le_bytes(row[x * 4..x * 4 + 4].try_into().expect("4 字节"));
                match &ms {
                    Some(m) => (
                        extract(p, m[0]),
                        extract(p, m[1]),
                        extract(p, m[2]),
                        if m[3].0 == 0 { 255 } else { extract(p, m[3]) },
                    ),
                    None => ((p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8),
                }
            };
            any_alpha |= a != 0;
            rgba[o..o + 4].copy_from_slice(&[r, g, b, a]);
        }
    }

    if !any_alpha {
        for px in rgba.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
    }
    Some((w, h, rgba))
}

pub fn rgba_to_png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    Some(out)
}

pub fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let (w, h, rgba) = dib_to_rgba(dib)?;
    rgba_to_png(w, h, &rgba)
}

pub fn png_to_rgba(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut dec = png::Decoder::new_with_limits(std::io::Cursor::new(data), png::Limits { bytes: 512 * 1024 * 1024 });
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().ok()?;
    let info = reader.info();
    if u64::from(info.width) * u64::from(info.height) > MAX_PIXELS {
        return None;
    }
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (frame.width, frame.height);
    let src = &buf[..frame.buffer_size()];
    let rgba: Vec<u8> = match frame.color_type {
        png::ColorType::Rgba => src.to_vec(),
        png::ColorType::Rgb => src.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::Grayscale => src.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => {
            src.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
        }
        png::ColorType::Indexed => return None,
    };
    (rgba.len() == w as usize * h as usize * 4).then_some((w, h, rgba))
}

pub fn png_to_dib(data: &[u8]) -> Option<Vec<u8>> {
    let (w, h, rgba) = png_to_rgba(data)?;
    let stride = w as usize * 4;
    let mut out = Vec::with_capacity(40 + stride * h as usize);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&BI_RGB.to_le_bytes());
    out.extend_from_slice(&((stride * h as usize) as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    for y in (0..h as usize).rev() {
        for px in rgba[y * stride..(y + 1) * stride].as_chunks::<4>().0 {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    Some(out)
}

pub fn is_png(data: &[u8]) -> bool {
    data.starts_with(b"\x89PNG\r\n\x1a\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newline_conversion() {
        assert_eq!(normalize_newlines("a\r\nb\r\n"), "a\nb\n");
        assert_eq!(to_crlf("a\nb\r\nc"), "a\r\nb\r\nc");
        let s = "第一行\n第二行\n";
        assert_eq!(normalize_newlines(&to_crlf(s)), s);
    }

    #[test]
    fn cf_html_roundtrip() {
        let frag = "<b>你好</b>，<i>喵</i>";
        let raw = cf_html_build(frag);
        assert_eq!(cf_html_extract(&raw).as_deref(), Some(frag));
    }

    #[test]
    fn cf_html_drops_nul() {
        assert_eq!(cf_html_extract(&cf_html_build("a\0b")).as_deref(), Some("ab"));
        assert_eq!(cf_html_extract(&cf_html_build("\0")).as_deref(), Some(""));
    }

    #[test]
    fn cf_html_real_world_sample() {
        let body = "<html><body><!--StartFragment--><p>喵 hi</p><!--EndFragment--></body></html>";
        let header_len =
            "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n"
                .len();
        let sf = header_len + body.find("<p>").unwrap();
        let ef = header_len + body.find("<!--EndFragment").unwrap();
        let raw = format!(
            "Version:0.9\r\nStartHTML:{header_len:010}\r\nEndHTML:{:010}\r\nStartFragment:{sf:010}\r\nEndFragment:{ef:010}\r\n{body}",
            header_len + body.len()
        );
        assert_eq!(cf_html_extract(raw.as_bytes()).as_deref(), Some("<p>喵 hi</p>"));
    }

    #[test]
    fn cf_html_rejects_bad_offsets() {
        assert!(cf_html_extract(b"Version:0.9\r\nStartFragment:0000009999\r\nEndFragment:0000099999\r\n").is_none());
        assert!(cf_html_extract(b"Version:0.9\r\nStartFragment:0000000050\r\nEndFragment:0000000010\r\n").is_none());
        assert!(cf_html_extract(b"garbage").is_none());
    }

    fn sample_rgba(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| [(i % 256) as u8, (i / 7 % 256) as u8, 200, if i % 3 == 0 { 128 } else { 255 }])
            .collect()
    }

    #[test]
    fn noncontiguous_masks_do_not_overflow() {
        let _ = extract(u32::MAX, mask_shift(0b1000_0001));
        let _ = extract(u32::MAX, mask_shift(0x8000_0001));
        assert_eq!(extract(0, mask_shift(0b1000_0001)), 0);
        assert_eq!(extract(0xff00, mask_shift(0xff00)), 255);
        assert_eq!(extract(0x1f, mask_shift(0x1f)), 255);
    }

    #[test]
    fn png_dib_roundtrip() {
        let (w, h) = (37, 11);
        let rgba = sample_rgba(w, h);
        let png = rgba_to_png(w, h, &rgba).unwrap();
        assert!(is_png(&png));
        let dib = png_to_dib(&png).unwrap();
        let (w2, h2, back) = dib_to_rgba(&dib).unwrap();
        assert_eq!((w2, h2), (w, h));
        assert_eq!(back, rgba);
    }

    #[test]
    fn dib_24bit_bottom_up() {
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0u8; 24]);
        dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
        dib.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]);
        let (_, _, rgba) = dib_to_rgba(&dib).unwrap();
        assert_eq!(&rgba[0..4], &[0, 0, 255, 255]);
        assert_eq!(&rgba[4..8], &[255, 255, 255, 255]);
        assert_eq!(&rgba[8..12], &[255, 0, 0, 255]);
        assert_eq!(&rgba[12..16], &[0, 255, 0, 255]);
    }

    #[test]
    fn dib_rejects_malformed() {
        assert!(dib_to_rgba(&[0u8; 10]).is_none());
        let mut huge = Vec::new();
        huge.extend_from_slice(&40u32.to_le_bytes());
        huge.extend_from_slice(&100_000i32.to_le_bytes());
        huge.extend_from_slice(&100_000i32.to_le_bytes());
        huge.extend_from_slice(&1u16.to_le_bytes());
        huge.extend_from_slice(&32u16.to_le_bytes());
        huge.extend_from_slice(&[0u8; 24]);
        assert!(dib_to_rgba(&huge).is_none(), "超大尺寸必须拒绝");

        let mut short = huge.clone();
        short[4..8].copy_from_slice(&10i32.to_le_bytes());
        short[8..12].copy_from_slice(&10i32.to_le_bytes());
        assert!(dib_to_rgba(&short).is_none());
    }

    #[test]
    fn dropfiles_layout() {
        let d = dropfiles_build(&[r"C:\a.txt", r"D:\图"]);
        assert_eq!(&d[..4], &20u32.to_le_bytes());
        assert_eq!(&d[16..20], &1u32.to_le_bytes());
        let w: Vec<u16> = d[20..].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        let expect: Vec<u16> = "C:\\a.txt\0D:\\图\0\0".encode_utf16().collect();
        assert_eq!(w, expect);
    }

    #[test]
    fn png_garbage_rejected() {
        assert!(png_to_rgba(b"\x89PNG\r\n\x1a\nnot really").is_none());
        assert!(png_to_dib(b"").is_none());
    }
}
