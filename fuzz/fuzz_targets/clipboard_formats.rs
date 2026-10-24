#![no_main]

use copycat_win::content;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = content::cf_html_extract(data);
    if let Some((w, h, rgba)) = content::dib_to_rgba(data) {
        assert_eq!(rgba.len(), w as usize * h as usize * 4);
        assert!(u64::from(w) * u64::from(h) <= content::MAX_PIXELS);
    }
    if let Some((w, h, rgba)) = content::png_to_rgba(data) {
        assert_eq!(rgba.len(), w as usize * h as usize * 4);
    }

    if let Ok(s) = std::str::from_utf8(data) {
        let built = content::cf_html_build(s);
        assert_eq!(content::cf_html_extract(&built), Some(s.replace('\0', "")));
    }
});
