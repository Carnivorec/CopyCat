use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, VARIANT_FALSE, VARIANT_TRUE};
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRule, NET_FW_ACTION_ALLOW, NET_FW_ACTION_BLOCK, NET_FW_IP_PROTOCOL_ANY,
    NET_FW_IP_PROTOCOL_UDP, NET_FW_PROFILE2_ALL, NET_FW_PROFILE2_DOMAIN, NET_FW_PROFILE2_PRIVATE,
    NET_FW_PROFILE2_PUBLIC, NET_FW_RULE_DIR_IN, NetFwPolicy2, NetFwRule,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch,
};
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{IsUserAnAdmin, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{BSTR, HSTRING, Interface, PCWSTR, w};

use crate::{Result, WinError};

fn rule_name(exe: &str) -> String {
    let h = exe.to_lowercase().bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    format!("CopyCat 局域网直连 {h:08x}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    NoRule,
    Blocked,
    BlockAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    pub public: bool,
}

fn api(e: windows::core::Error) -> WinError {
    WinError::Api("Windows 防火墙", e.code().0 as u32)
}

fn with_policy<T: Send + 'static>(f: impl FnOnce(&INetFwPolicy2) -> Result<T> + Send + 'static) -> Result<T> {
    std::thread::spawn(move || {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().map_err(api)?;
            let r = CoCreateInstance::<_, INetFwPolicy2>(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)
                .map_err(api)
                .and_then(|p| f(&p));
            CoUninitialize();
            r
        }
    })
    .join()
    .unwrap_or(Err(WinError::Api("Windows 防火墙", 0)))
}

fn same_path(a: &str, b: &str) -> bool {
    a.trim_matches('"').eq_ignore_ascii_case(b)
}

unsafe fn exe_rules(policy: &INetFwPolicy2, exe: &str) -> windows::core::Result<Vec<INetFwRule>> {
    unsafe {
        let it: IEnumVARIANT = policy.Rules()?._NewEnum()?.cast()?;
        let mut out = Vec::new();
        loop {
            let mut v = [VARIANT::default()];
            let mut n = 0;
            if it.Next(&mut v, &mut n).is_err() || n == 0 {
                break;
            }
            let Ok(rule) = IDispatch::try_from(&v[0]).and_then(|d| d.cast::<INetFwRule>()) else { continue };
            let Ok(app) = rule.ApplicationName() else { continue };
            if !same_path(&app.to_string(), exe)
                || rule.Direction()? != NET_FW_RULE_DIR_IN
                || rule.Enabled()? != VARIANT_TRUE
                || !matches!(rule.Protocol()?, p if p == NET_FW_IP_PROTOCOL_UDP.0 || p == NET_FW_IP_PROTOCOL_ANY.0)
            {
                continue;
            }
            out.push(rule);
        }
        Ok(out)
    }
}

pub fn status(exe: &Path) -> Result<Status> {
    let exe = exe.to_string_lossy().into_owned();
    with_policy(move |p| {
        unsafe {
            let active = p.CurrentProfileTypes().map_err(api)?;
            let (mut allow, mut block) = (0, 0);
            for r in exe_rules(p, &exe).map_err(api)? {
                let profiles = r.Profiles().map_err(api)?;
                let local_any = any_port(&r.LocalPorts().map_err(api)?.to_string());
                if r.Action().map_err(api)? == NET_FW_ACTION_BLOCK {
                    if local_any {
                        block |= profiles;
                    }
                } else if local_any
                    && any_port(&r.RemotePorts().map_err(api)?.to_string())
                    && any_lan(&r.RemoteAddresses().map_err(api)?.to_string())
                {
                    allow |= profiles;
                }
            }
            let mut state = State::Open;
            for t in [NET_FW_PROFILE2_DOMAIN, NET_FW_PROFILE2_PRIVATE, NET_FW_PROFILE2_PUBLIC] {
                if active & t.0 == 0 || p.get_FirewallEnabled(t).map_err(api)? != VARIANT_TRUE {
                    continue;
                }
                let s = if p.get_BlockAllInboundTraffic(t).map_err(api)? == VARIANT_TRUE {
                    State::BlockAll
                } else if block & t.0 != 0 {
                    State::Blocked
                } else if allow & t.0 == 0 && p.get_DefaultInboundAction(t).map_err(api)? != NET_FW_ACTION_ALLOW {
                    State::NoRule
                } else {
                    State::Open
                };
                state = worse(state, s);
            }
            Ok(Status { state, public: active & NET_FW_PROFILE2_PUBLIC.0 != 0 })
        }
    })
}

fn worse(a: State, b: State) -> State {
    let rank = |s| match s {
        State::Open => 0,
        State::BlockAll => 1,
        State::NoRule => 2,
        State::Blocked => 3,
    };
    if rank(b) > rank(a) { b } else { a }
}

fn any_port(ports: &str) -> bool {
    matches!(ports.trim(), "" | "*")
}

fn any_lan(addrs: &str) -> bool {
    let a = addrs.trim();
    a.is_empty() || a == "*" || a.split(',').any(|x| x.trim().eq_ignore_ascii_case("LocalSubnet"))
}

unsafe fn remove_own(policy: &INetFwPolicy2, exe: &str) -> windows::core::Result<()> {
    unsafe {
        let rules = policy.Rules()?;
        let name = BSTR::from(rule_name(exe));
        for _ in 0..16 {
            if rules.Item(&name).is_err() {
                break;
            }
            rules.Remove(&name)?;
        }
        Ok(())
    }
}

pub fn allow(exe: &Path) -> Result<()> {
    let exe = exe.to_string_lossy().into_owned();
    with_policy(move |p| {
        unsafe {
            for r in exe_rules(p, &exe).map_err(api)? {
                if r.Action().map_err(api)? == NET_FW_ACTION_BLOCK
                    && r.Protocol().map_err(api)? == NET_FW_IP_PROTOCOL_UDP.0
                {
                    r.SetEnabled(VARIANT_FALSE).map_err(api)?;
                }
            }
            remove_own(p, &exe).map_err(api)?;
            let r: INetFwRule = CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER).map_err(api)?;
            r.SetName(&BSTR::from(rule_name(&exe))).map_err(api)?;
            r.SetDescription(&BSTR::from("让同一局域网内的设备直接连到本机（剪贴板与文件同步）")).map_err(api)?;
            r.SetGrouping(&BSTR::from("CopyCat")).map_err(api)?;
            r.SetApplicationName(&BSTR::from(exe.as_str())).map_err(api)?;
            r.SetProtocol(NET_FW_IP_PROTOCOL_UDP.0).map_err(api)?;
            r.SetDirection(NET_FW_RULE_DIR_IN).map_err(api)?;
            r.SetProfiles(NET_FW_PROFILE2_ALL.0).map_err(api)?;
            r.SetAction(NET_FW_ACTION_ALLOW).map_err(api)?;
            r.SetEnabled(VARIANT_TRUE).map_err(api)?;
            p.Rules().map_err(api)?.Add(&r).map_err(api)
        }
    })
}

pub fn has_own_rule(exe: &Path) -> Result<bool> {
    let name = rule_name(&exe.to_string_lossy());
    with_policy(move |p| unsafe { Ok(p.Rules().map_err(api)?.Item(&BSTR::from(name)).is_ok()) })
}

pub fn remove(exe: &Path) -> Result<()> {
    let exe = exe.to_string_lossy().into_owned();
    with_policy(move |p| unsafe { remove_own(p, &exe).map_err(api) })
}

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

pub fn run_elevated(exe: &Path, args: &str) -> Result<Option<u32>> {
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            if e.code() == ERROR_CANCELLED.to_hresult() {
                return Ok(None);
            }
            return Err(api(e));
        }
        if info.hProcess.is_invalid() {
            return Err(WinError::Api("ShellExecuteExW", 0));
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let r = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        r.map_err(api)?;
        Ok(Some(code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_of_current_exe() {
        let exe = std::env::current_exe().unwrap();
        status(&exe).unwrap();
        has_own_rule(&exe).unwrap();
    }

    #[test]
    fn rule_name_per_install() {
        assert_eq!(rule_name(r"C:\A\copycat.exe"), rule_name(r"c:\a\COPYCAT.EXE"));
        assert_ne!(rule_name(r"C:\A\copycat.exe"), rule_name(r"C:\B\copycat.exe"));
    }

    #[test]
    fn remote_scope() {
        assert!(any_lan("*") && any_lan("") && any_lan("LocalSubnet") && any_lan("10.0.0.1, LocalSubnet"));
        assert!(!any_lan("192.168.1.5/255.255.255.255"));
    }

    #[test]
    fn path_compare() {
        assert!(same_path("\"C:\\A\\copycat.exe\"", "c:\\a\\COPYCAT.exe"));
        assert!(!same_path("C:\\A\\copycat.exe", "C:\\B\\copycat.exe"));
    }
}
