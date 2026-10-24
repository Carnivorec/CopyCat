#[cfg(any(target_os = "android", target_os = "linux"))]
fn main() {
    use std::ffi::CString;

    const SHELL: libc::uid_t = 2000;

    const GROUPS: [libc::gid_t; 6] = [2000, 1004, 1007, 1011, 3003, 3009];

    let args: Vec<CString> = std::env::args().skip(1).map(|a| CString::new(a).expect("参数不含 NUL")).collect();
    if args.is_empty() {
        eprintln!("用法: copycat-launch <程序> [参数...]");
        std::process::exit(2);
    }

    unsafe {
        if libc::setgroups(GROUPS.len(), GROUPS.as_ptr()) != 0
            || libc::setresgid(SHELL, SHELL, SHELL) != 0
            || libc::setresuid(SHELL, SHELL, SHELL) != 0
        {
            eprintln!("降权失败: {}", std::io::Error::last_os_error());
            std::process::exit(1);
        }

        if libc::setuid(0) == 0 {
            eprintln!("降权后仍能恢复 root，拒绝继续");
            std::process::exit(1);
        }
        let mut argv: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
        argv.push(std::ptr::null());
        libc::execvp(argv[0], argv.as_ptr());
    }
    eprintln!("启动 {:?} 失败: {}", args[0], std::io::Error::last_os_error());
    std::process::exit(1);
}

#[cfg(not(any(target_os = "android", target_os = "linux")))]
fn main() {
    eprintln!("仅支持安卓");
}
