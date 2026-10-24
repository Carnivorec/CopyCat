use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

pub fn local_stamp() -> String {
    let mut t =
        SYSTEMTIME { wYear: 0, wMonth: 0, wDayOfWeek: 0, wDay: 0, wHour: 0, wMinute: 0, wSecond: 0, wMilliseconds: 0 };

    unsafe { GetLocalTime(&mut t) };
    format!("{:04}{:02}{:02}_{:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

#[cfg(test)]
mod tests {
    #[test]
    fn stamp_shape() {
        let s = super::local_stamp();
        assert_eq!(s.len(), 15);
        assert_eq!(&s[8..9], "_");
        assert!(s.starts_with("20"));
    }
}
