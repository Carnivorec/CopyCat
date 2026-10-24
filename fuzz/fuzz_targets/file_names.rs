#![no_main]

use copycat_core::fuzzing::sanitize_name;
use libfuzzer_sys::fuzz_target;

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fuzz_target!(|data: &[u8]| {
    let Ok(name) = std::str::from_utf8(data) else { return };
    let Some(clean) = sanitize_name(name) else { return };
    let parts: Vec<&str> = clean.split('/').collect();
    assert!(!parts.is_empty() && parts.len() <= 16);
    for p in parts {
        assert!(!p.is_empty() && p.len() <= 200);
        assert!(p != "." && p != "..");
        assert!(!p.ends_with('.') && !p.ends_with(' '), "结尾的点或空格会被 Windows 悄悄去掉：{p:?}");
        assert!(!p.chars().any(|c| c.is_control() || "<>:\"|?*\\".contains(c)));
        let stem = p.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
        assert!(!RESERVED.contains(&stem.as_str()), "保留设备名：{p:?}");
    }

    assert_eq!(sanitize_name(&clean).as_deref(), Some(clean.as_str()));
});
