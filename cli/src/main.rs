mod bridge;
mod keyfile;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use copycat_core::{FileSource, Node, NodeConfig, NodeEvent, OutgoingFile, Platform, TransferState};
use copycat_store::Store;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

struct Args {
    cmd: String,
    positional: Vec<String>,
    data: PathBuf,
    name: Option<String>,
}

fn default_data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CopyCat")
        .join("cli")
}

fn parse_args() -> Result<Args> {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().unwrap_or_else(|| "help".into());
    let mut a = Args { cmd, positional: vec![], data: default_data_dir(), name: None };
    while let Some(arg) = it.next() {
        let mut val = || it.next().with_context(|| format!("{arg} 缺少参数值"));
        match arg.as_str() {
            "--data" => a.data = PathBuf::from(val()?),
            "--name" => a.name = Some(val()?),
            s if s.starts_with("--") => bail!("未知参数 {s}"),
            _ => a.positional.push(arg),
        }
    }
    Ok(a)
}

fn host_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows 电脑".into())
}

fn receive_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
        .join("CopyCat")
}

fn platform() -> Platform {
    if cfg!(windows) { Platform::Windows } else { Platform::Other(0) }
}

fn open_store(data: &PathBuf) -> Result<Store> {
    std::fs::create_dir_all(data)?;
    let key = keyfile::load_or_create(&data.join("master.key"))?;
    Ok(Store::open(data.join("copycat.db"), &key)?)
}

fn print_help() {
    println!(
        "CopyCat 命令行客户端\n\n\
         用法:\n  \
         copycat init [--name 设备名]\n  \
         copycat join \"copycat://pair?...\" [--name 设备名]\n  \
         copycat run\n\n\
         公共参数: --data 目录（默认 %LOCALAPPDATA%\\CopyCat\\cli）"
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn,copycat=info".into()),
        )
        .init();
    let args = parse_args()?;
    let name = args.name.clone().unwrap_or_else(host_name);
    match args.cmd.as_str() {
        "init" => {
            let (node, _events) =
                Node::create_group(open_store(&args.data)?, NodeConfig::new(name, platform())).await?;
            println!("✓ 设备组已创建，本机 ID：{}", node.id().short());
            println!("现在可以运行 `copycat run`，再输入 invite 生成配对码（同一局域网内的设备可直接加入）。");
            println!("需要跨网络同步时，在 run 中输入 relay <域名:端口> 生成中转服务器的部署密钥。");
            node.shutdown().await;
        }
        "join" => {
            let uri = args.positional.first().context("缺少配对码（copycat://pair?...）")?;
            println!("正在连接邀请方……");
            let (node, _events) = Node::join(open_store(&args.data)?, NodeConfig::new(name, platform()), uri, |sas| {
                println!("请核对确认码：{sas}（与邀请方屏幕上的一致后，在邀请方确认）");
            })
            .await?;
            println!("✓ 已加入设备组，本机 ID：{}", node.id().short());
            node.shutdown().await;
        }
        "run" => run(open_store(&args.data)?, NodeConfig::new(name, platform())).await?,
        _ => print_help(),
    }
    Ok(())
}

fn print_qr(text: &str) {
    if let Ok(code) = qrcode::QrCode::new(text.as_bytes()) {
        let s = code
            .render::<qrcode::render::unicode::Dense1x2>()
            .dark_color(qrcode::render::unicode::Dense1x2::Light)
            .light_color(qrcode::render::unicode::Dense1x2::Dark)
            .build();
        println!("{s}");
    }
}

async fn run(store: Store, cfg: NodeConfig) -> Result<()> {
    let (node, mut events) = Node::open(store, cfg).await?;
    println!("CopyCat 已启动，本机 ID：{}。输入 help 查看命令。", node.id().short());

    let (apply_tx, apply_rx) = mpsc::unbounded_channel();
    bridge::start(node.clone(), apply_rx)?;

    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                match ev {
                    NodeEvent::Apply(item) => {
                        let _ = apply_tx.send(item);
                    }
                    NodeEvent::PeerUp(id) => println!("● 已连接 {}", id.short()),
                    NodeEvent::PeerDown(id) => println!("○ 已断开 {}", id.short()),
                    NodeEvent::LanPath { peer, up: true } => println!("⚡ 与 {} 局域网直连", peer.short()),
                    NodeEvent::LanPath { peer, up: false } => println!("与 {} 的局域网直连断开，改走中转", peer.short()),
                    NodeEvent::RelayConnected => println!("中转服务器：已连接"),
                    NodeEvent::RelayDisconnected(r) => println!("中转服务器：断开（{r}），自动重连中"),
                    NodeEvent::RelayChanged => println!("中转配置已更新"),
                    NodeEvent::PairRequest { name, sas, .. } => {
                        println!("\n“{name}”请求加入，确认码 {sas}。核对一致后输入 yes 同意，no 拒绝。");
                    }
                    NodeEvent::Paired(id) => println!("✓ 新设备 {} 已加入", id.short()),
                    NodeEvent::Revoked => {
                        println!("本机已被移出设备组，同步停止。");
                        break;
                    }
                    NodeEvent::Transfer(t) => match &t.state {
                        TransferState::Asking => {
                            let dir = receive_dir();
                            println!("↓ {} 发来 {} 个文件，保存到 {}", t.peer.short(), t.files.len(), dir.display());
                            if let Err(e) = node.accept_transfer(t.id, dir).await {
                                println!("接收失败：{e}");
                            }
                        }
                        TransferState::Done if t.outgoing => println!("✓ 已发送到 {}", t.peer.short()),
                        TransferState::Done => {
                            for p in &t.saved {
                                println!("✓ 已收到 {}", p.display());
                            }
                        }
                        TransferState::Failed(e) => println!("✗ 传输失败：{e}"),
                        _ => {}
                    },
                    NodeEvent::SharedText { from, text } => println!("{} 分享了：{text}", from.short()),
                    NodeEvent::HistoryChanged | NodeEvent::RosterChanged => {}
                }
            }
            line = stdin.next_line() => {
                let Ok(Some(line)) = line else { break };
                if !command(&node, line.trim()).await? {
                    break;
                }
            }
        }
    }
    node.shutdown().await;
    Ok(())
}

async fn command(node: &Node, line: &str) -> Result<bool> {
    let mut parts = line.split_whitespace();
    match parts.next().unwrap_or("") {
        "" => {}
        "help" => println!(
            "命令：\n  invite        生成配对码（2 分钟有效）\n  yes / no      同意 / 拒绝配对请求\n  \
             devices       查看设备\n  remove <ID>   移除设备（短 ID 前缀即可）\n  history       最近的同步记录\n  \
             send <ID> <路径>  发送文件或文件夹\n  \
             relay         查看中转服务器\n  relay <域名:端口>  生成新的中转密钥并使用该服务器\n  \
             relay addr <域名:端口>  只修改地址\n  relay off     不再使用中转服务器\n  \
             net           网络已切换，立即重连\n  quit          退出"
        ),
        "invite" => match node.create_invite().await {
            Ok(uri) => {
                print_qr(&uri);
                println!("配对码（在新设备上运行 copycat join \"<配对码>\"）：\n{uri}");
            }
            Err(e) => println!("生成失败：{e}"),
        },
        "yes" | "no" => match node.respond_pairing(line.starts_with('y')).await {
            Ok(id) => println!("已同意，设备 {} 加入", id.short()),
            Err(e) => println!("{e}"),
        },
        "devices" => {
            let online = node.online_peers().await?;
            for m in node.members().await? {
                let mark = if m.id == node.id() {
                    "（本机）"
                } else if online.contains(&m.id) {
                    "● 在线"
                } else {
                    "○ 离线"
                };
                println!("  {}  {:<20} {mark}", m.id.short(), m.name);
            }
        }
        "remove" => {
            let prefix = parts.next().unwrap_or("");
            let matches: Vec<_> = node
                .members()
                .await?
                .into_iter()
                .filter(|m| !prefix.is_empty() && m.id.to_string().starts_with(prefix))
                .collect();
            match matches.as_slice() {
                [m] if m.id == node.id() => println!("不能在这里移除本机"),
                [m] => match node.remove_device(m.id).await {
                    Ok(()) => println!("已移除 {}（{}）", m.name, m.id.short()),
                    Err(e) => println!("移除失败：{e}"),
                },
                [] => println!("没有匹配的设备"),
                _ => println!("匹配到多台设备，请输入更长的 ID"),
            }
        }
        "history" => {
            for it in node.history().await?.iter().take(10) {
                let preview = it
                    .reps
                    .iter()
                    .find(|r| r.mime == "text/plain")
                    .and_then(|r| r.inline.as_ref())
                    .map(|b| String::from_utf8_lossy(b).chars().take(40).collect::<String>().replace('\n', "⏎"))
                    .unwrap_or_else(|| format!("[{}]", it.reps.first().map(|r| r.mime.as_str()).unwrap_or("?")));
                println!("  {}  {preview}", it.origin.short());
            }
        }
        "send" => {
            let prefix = parts.next().unwrap_or("");
            let path = PathBuf::from(parts.collect::<Vec<_>>().join(" "));
            let target: Vec<_> = node
                .members()
                .await?
                .into_iter()
                .filter(|m| !prefix.is_empty() && m.id != node.id() && m.id.to_string().starts_with(prefix))
                .collect();
            let [m] = target.as_slice() else {
                println!("用法：send <设备 ID 前缀> <路径>（ID 见 devices）");
                return Ok(true);
            };
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                println!("路径无效");
                return Ok(true);
            };
            if !path.is_file() {
                println!("命令行只支持发送单个文件");
                return Ok(true);
            }
            match node.send_files(m.id, vec![OutgoingFile { name, source: FileSource::Path(path) }]).await {
                Ok(_) => println!("正在发送给 {}……", m.name),
                Err(e) => println!("发送失败：{e}"),
            }
        }
        "relay" => {
            let args: Vec<&str> = parts.collect();
            let r = match args.as_slice() {
                [] => {
                    let st = node.status().await?;
                    match st.relay_addr {
                        Some(a) => {
                            println!("中转服务器：{a}（{}）", if st.relay_connected { "已连接" } else { "未连接" })
                        }
                        None => println!("未配置中转服务器，只在局域网内同步"),
                    }
                    if let Some(key) = node.relay_secret().await? {
                        println!("部署密钥（填入服务器的 COPYCAT_RELAY_KEY）：\n{key}");
                    }
                    Ok(())
                }
                ["off"] => node.remove_relay().await.map(|()| println!("已停用中转服务器")),
                ["addr", addr] => node.set_relay_addr((*addr).to_owned()).await.map(|()| println!("已改用 {addr}")),
                [addr] => node.setup_relay((*addr).to_owned()).await.map(|key| {
                    println!("已生成新的中转密钥。请把它填入服务器 compose.yml 的 COPYCAT_RELAY_KEY 后重启服务器：");
                    println!("{key}");
                }),
                _ => {
                    println!("用法：relay | relay <域名:端口> | relay addr <域名:端口> | relay off");
                    Ok(())
                }
            };
            if let Err(e) = r {
                println!("{e}");
            }
        }
        "net" => {
            node.network_changed().await;
            println!("正在重连……");
        }
        "quit" | "exit" => return Ok(false),
        other => println!("未知命令 {other}，输入 help 查看"),
    }
    Ok(true)
}
