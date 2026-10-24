use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::ptr::null_mut;
use std::sync::OnceLock;

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JIntArray, JObjectArray, JString};
use jni::sys::jstring;

use crate::daemon::{Clip, Daemon};
use crate::ipc;

static DAEMON: OnceLock<Daemon> = OnceLock::new();

fn opt_string(env: &mut JNIEnv, s: &JString) -> Option<String> {
    if s.is_null() {
        return None;
    }
    env.get_string(s).ok().map(Into::into)
}

fn to_jstring(env: &mut JNIEnv, s: Option<String>) -> jstring {
    s.and_then(|s| env.new_string(s).ok()).map_or(null_mut(), |j| j.into_raw())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_copycat_daemon_Native_start<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    data_dir: JString<'l>,
    key: JByteArray<'l>,
    device_name: JString<'l>,
) -> jstring {
    let r = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        crate::init_logging();
        crate::harden();
        let dir = opt_string(&mut env, &data_dir).ok_or("缺少数据目录")?;
        let key: [u8; 32] =
            env.convert_byte_array(&key).ok().and_then(|k| k.try_into().ok()).ok_or("主密钥应为 32 字节")?;
        let name = opt_string(&mut env, &device_name).unwrap_or_else(|| "安卓手机".into());
        if DAEMON.get().is_some() {
            return Err("守护进程已经启动".into());
        }
        let d = Daemon::start(PathBuf::from(&dir), key, name)?;
        #[cfg(any(target_os = "android", target_os = "linux"))]
        {
            crate::netlink::spawn(d.svc.clone(), d.rt.handle().clone());
            crate::resume::spawn(d.svc.clone(), d.rt.handle().clone());
            crate::attach::spawn(d.svc.clone(), PathBuf::from(&dir));
        }
        DAEMON.set(d).map_err(|_| "守护进程已经启动".to_owned())
    }))
    .unwrap_or_else(|_| Err("启动时发生内部错误".into()));
    to_jstring(&mut env, r.err())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_copycat_daemon_Native_localClip<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    text: JString<'l>,
    html: JString<'l>,
    source: JString<'l>,
) {
    let Some(d) = DAEMON.get() else { return };
    let clip = Clip { text: opt_string(&mut env, &text), html: opt_string(&mut env, &html) };
    let source = opt_string(&mut env, &source);
    let _ = catch_unwind(AssertUnwindSafe(|| d.local_clip(clip, source)));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_copycat_daemon_Native_request<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    request: JString<'l>,
) -> jstring {
    let out = match (DAEMON.get(), opt_string(&mut env, &request)) {
        (Some(d), Some(req)) => catch_unwind(AssertUnwindSafe(|| d.rt.block_on(ipc::handle(&d.svc, &req))))
            .unwrap_or_else(|_| r#"{"err":"守护进程内部错误"}"#.to_owned()),
        _ => r#"{"err":"守护进程尚未启动"}"#.to_owned(),
    };
    to_jstring(&mut env, Some(out))
}

#[cfg(unix)]
fn adopt(fd: i32) -> Option<std::fs::File> {
    use std::os::fd::{FromRawFd, OwnedFd};

    (fd >= 0).then(|| std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
}

#[cfg(not(unix))]
fn adopt(_fd: i32) -> Option<std::fs::File> {
    None
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_copycat_daemon_Native_sendFiles<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    device: JString<'l>,
    names: JObjectArray<'l>,
    fds: JIntArray<'l>,
) -> jstring {
    let r = catch_unwind(AssertUnwindSafe(|| -> Result<String, String> {
        let n = env.get_array_length(&fds).map_err(|_| "参数无效")? as usize;
        let mut raw = vec![0i32; n];
        env.get_int_array_region(&fds, 0, &mut raw).map_err(|_| "参数无效")?;

        let files: Vec<Option<std::fs::File>> = raw.into_iter().map(adopt).collect();
        let device = opt_string(&mut env, &device).ok_or("缺少目标设备")?;
        let mut out = Vec::with_capacity(n);
        for (i, f) in files.into_iter().enumerate() {
            let obj = env.get_object_array_element(&names, i as i32).map_err(|_| "参数无效")?;
            let name = opt_string(&mut env, &JString::from(obj)).ok_or("缺少文件名")?;
            out.push((name, f.ok_or("文件无法打开")?));
        }
        let d = DAEMON.get().ok_or("守护进程尚未启动")?;
        Ok(d.send_files(&device, out))
    }))
    .unwrap_or_else(|_| Err("守护进程内部错误".into()));
    let json = r.unwrap_or_else(|e| serde_json::json!({ "err": e }).to_string());
    to_jstring(&mut env, Some(json))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_copycat_daemon_Native_nextAction<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
) -> jstring {
    let json = DAEMON.get().and_then(Daemon::next_action).and_then(|a| serde_json::to_string(&a).ok());
    to_jstring(&mut env, json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_copycat_daemon_Native_stop<'l>(_env: JNIEnv<'l>, _class: JClass<'l>) {
    if let Some(d) = DAEMON.get() {
        let _ = catch_unwind(AssertUnwindSafe(|| d.stop()));
    }
}
