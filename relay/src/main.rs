use std::io::IsTerminal;
use std::net::SocketAddr;

use anyhow::{Context, Result, bail};
use copycat_relay::{Limits, RelayServer};
use copycat_transport::tls::{RelayKey, hex};

const DEFAULT_DATA_DIR: &str = "/var/lib/copycat-relay";
const DEFAULT_LISTEN: &str = "0.0.0.0:8443";
const KEY_ENV: &str = "COPYCAT_RELAY_KEY";

struct Args {
    cmd: String,
    data_dir: String,
    listen: String,
    tcp_listen: Option<String>,
    key: Option<String>,
}

fn parse_args() -> Result<Args> {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().unwrap_or_else(|| "help".into());
    let mut a =
        Args { cmd, data_dir: DEFAULT_DATA_DIR.into(), listen: DEFAULT_LISTEN.into(), tcp_listen: None, key: None };
    while let Some(flag) = it.next() {
        let mut val = || it.next().with_context(|| format!("{flag} 缺少参数值"));
        match flag.as_str() {
            "--data-dir" => a.data_dir = val()?,
            "--listen" => a.listen = val()?,
            "--tcp-listen" => a.tcp_listen = Some(val()?),
            "--key" => a.key = Some(val()?),
            other => bail!("未知参数 {other}"),
        }
    }
    Ok(a)
}

fn print_help() {
    println!(
        "CopyCat 中转服务器\n\n\
         用法:\n  \
         copycat-relay run [--data-dir DIR] [--listen ADDR] [--tcp-listen ADDR] [--key 中转密钥]\n\n\
         中转密钥在 CopyCat 电脑端“设置 → 中转服务器”中生成，经环境变量 {KEY_ENV} 传入。\n\
         默认数据目录 {DEFAULT_DATA_DIR}，默认监听 {DEFAULT_LISTEN}（UDP 与 TCP 同一端口）。"
    );
}

fn load_key(arg: Option<&str>) -> Result<RelayKey> {
    let text = match arg {
        Some(k) => k.to_owned(),
        None => std::env::var(KEY_ENV).ok().filter(|v| !v.trim().is_empty()).with_context(|| {
            format!("没有中转密钥：请在 CopyCat 电脑端“设置 → 中转服务器”中生成，填入环境变量 {KEY_ENV}")
        })?,
    };
    Ok(RelayKey::parse(&text)?)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,quinn=warn".into()),
        )
        .with_ansi(std::io::stdout().is_terminal())
        .init();
    let args = parse_args()?;
    match args.cmd.as_str() {
        "run" => {
            let key = load_key(args.key.as_deref())?;
            let listen: SocketAddr = args.listen.parse().context("监听地址格式错误")?;
            let mut limits = Limits::default();
            if let Some(t) = &args.tcp_listen {
                limits.tcp_listen = Some(t.parse().context("TCP 监听地址格式错误")?);
            }
            println!("中转服务器公钥: {}", hex(&key.public()));
            RelayServer::bind(listen, &args.data_dir, &key, limits)?.run().await;
        }
        _ => print_help(),
    }
    Ok(())
}
