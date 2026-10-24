import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Copy, FolderOpen, Keyboard, Laptop, LogOut, Monitor, Plus, Search, Smartphone, Trash2, X } from "lucide-react";
import { useApp } from "./App";
import type { Clip, Device, RelaySetup, Settings } from "./api";
import { fmtSize, fmtTime, isCJK, KIND_LABEL, otpDigits } from "./format";
import { useFlip, useNudge, useSwipe } from "./motion";
import { mo, P } from "./spring";

export function ClipboardPage() {
  const { api, ov, toast } = useApp();
  const [q, setQ] = useState("");
  const list = q.trim()
    ? ov.history.filter((c) => (c.text ?? "").toLowerCase().includes(q.trim().toLowerCase()) || c.device.includes(q.trim()))
    : ov.history;
  const [head, ...rest] = list;
  const box = useRef<HTMLDivElement>(null);
  useFlip(box);

  const copy = async (c: Clip) => {
    try {
      await api.copyItem(c.id);
      toast(c.kind === "otp" ? `验证码 ${otpDigits(c.text ?? "")} 已复制` : "已复制，并同步到其他设备");
    } catch (e) {
      toast(String(e));
    }
  };
  const forget = (c: Clip) => api.forgetItem(c.id).catch((e) => toast(String(e)));

  return (
    <div className="page">
      <section className="hero">
        <div className="meta">
          <b>{head ? (head.mine ? "本机复制" : `来自 ${head.device}`) : "剪贴板"}</b>
          <span className="mono">{head ? `${KIND_LABEL[head.kind]} · ${fmtTime(head.time)}` : ""}</span>
        </div>
        <HeroBody clip={head ?? null} searching={!!q.trim()} />
        <Rule trigger={head?.id ?? ""} />
        <div className="actions">
          {head && (
            <>
              <button onClick={() => copy(head)}><Copy />复制</button>
              <button onClick={() => forget(head)}><Trash2 />删除</button>
            </>
          )}
          <span className="hint">{head ? (head.kind === "image" ? fmtSize(head.size) : `${head.chars} 字`) : ""}</span>
        </div>
      </section>
      <div className="list-head">
        <span>No.</span>
        <label className="search">
          <Search size={13} />
          <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="搜索历史" spellCheck={false} />
          {q && <button onClick={() => setQ("")} aria-label="清除搜索"><X size={12} /></button>}
        </label>
        <span>来源</span>
        <span style={{ textAlign: "right" }}>时间</span>
      </div>
      <div className="list" ref={box}>
        {rest.map((c, i) => (
          <Row key={c.id} clip={c} index={i + 2} onCopy={() => copy(c)} onForget={() => forget(c)} />
        ))}
        {rest.length === 0 && head && <div className="list-empty">{q ? "没有更多匹配的内容" : "更早的内容会出现在这里"}</div>}
      </div>
    </div>
  );
}

function Rule({ trigger }: { trigger: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const first = useRef(true);
  useLayoutEffect(() => {
    const m = mo(ref.current!);
    if (first.current) {
      first.current = false;
      return;
    }
    m.SX.set(0);
    m.SX.to(1, P.rule);
  }, [trigger]);
  return <div className="rule" ref={ref} />;
}

function HeroBody({ clip, searching }: { clip: Clip | null; searching: boolean }) {
  const [shown, setShown] = useState<{ clip: Clip | null; key: string }[]>([{ clip, key: clip?.id ?? "empty" }]);
  const key = clip?.id ?? "empty";
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    setShown((s) => (s[s.length - 1].key === key ? s : [...s.slice(-1), { clip, key }]));
  }, [key, clip]);
  return (
    <div className="hero-body">
      {shown.map((s, i) => (
        <HeroItem
          key={s.key}
          clip={s.clip}
          searching={searching}
          leaving={i < shown.length - 1}
          animate={shown.length > 1}
          onGone={() => setShown((x) => x.filter((y) => y.key !== s.key))}
        />
      ))}
    </div>
  );
}

function HeroItem(props: { clip: Clip | null; searching: boolean; leaving: boolean; animate: boolean; onGone(): void }) {
  const { clip, leaving, animate, onGone, searching } = props;
  const ref = useRef<HTMLDivElement>(null);
  const { api } = useApp();
  const [img, setImg] = useState<string | null>(null);
  const gone = useRef(onGone);
  gone.current = onGone;

  useLayoutEffect(() => {
    if (!animate) return;
    const m = mo(ref.current!);
    m.Y.set(-90);
    m.O.set(0);
    m.Y.to(0, P.bouncy);
    m.O.to(1, P.gentle);
  }, []);
  useLayoutEffect(() => {
    if (!leaving) return;
    const m = mo(ref.current!);
    m.Y.to(60, P.snappy);
    m.O.to(0, P.snappy).then(() => gone.current());
  }, [leaving]);
  useEffect(() => {
    setImg(null);
    if (clip?.hasImage && !clip.pending) api.itemImage(clip.id).then(setImg).catch(() => {});
  }, [clip?.id, clip?.hasImage, clip?.pending, api]);

  if (!clip) {
    return (
      <div className="hero-empty" ref={ref}>
        {searching ? "没有匹配的内容。" : <>复制任何内容，<br />它会出现在这里。</>}
      </div>
    );
  }
  const text = clip.text ?? "";
  if (clip.kind === "image") {
    return (
      <div className="hero-text image" ref={ref}>
        {img && <img src={img} alt="" draggable={false} />}
        {clip.pending && <span className="pending">尚未下载到本机<br />复制时从其他设备取回</span>}
        <span className="cap">图片<span className="mono">PNG · {fmtSize(clip.size)}</span></span>
      </div>
    );
  }
  if (clip.kind === "otp") return <div className="hero-text otp" ref={ref}>{otpDigits(text)}</div>;
  const cls = clip.kind === "code" ? "code" : clip.kind === "link" ? "link" : isCJK(text) ? "cjk" : "";
  return <div className={`hero-text ${cls}`} ref={ref}>{text.slice(0, 160)}</div>;
}

function Row({ clip, index, onCopy, onForget }: { clip: Clip; index: number; onCopy(): void; onForget(): void }) {
  const ref = useRef<HTMLDivElement>(null);
  const tx = useRef<HTMLSpanElement>(null);
  useSwipe(ref, onForget, () => {
    const m = mo(ref.current!);
    m.X.set(10);
    m.X.to(0, P.wobbly);
    onCopy();
  });
  useEffect(() => {
    const el = ref.current!;
    const m = () => mo(tx.current!);
    const enter = () => m().X.to(10, P.bouncy);
    const leave = () => m().X.to(0, P.bouncy);
    el.addEventListener("pointerenter", enter);
    el.addEventListener("pointerleave", leave);
    return () => {
      el.removeEventListener("pointerenter", enter);
      el.removeEventListener("pointerleave", leave);
    };
  }, []);
  const mono = clip.kind === "code" || clip.kind === "link";
  const label = clip.kind === "otp" ? "OTP" : clip.kind === "image" ? "IMG" : null;
  const text =
    clip.kind === "image" ? `图片 · ${fmtSize(clip.size)}${clip.pending ? " · 未下载" : ""}` : (clip.text ?? "").replace(/\s+/g, " ");
  return (
    <div className="row" ref={ref} data-id={clip.id} title="单击复制 · 向左滑动删除">
      <span className="ix">{String(index).padStart(2, "0")}</span>
      <span className={`tx ${mono ? "mono" : ""}`} ref={tx}>
        {label && <span className="k">{label}</span>}
        {text}
      </span>
      <span className="dv">{clip.mine ? "本机" : clip.device}</span>
      <span className="tm">{fmtTime(clip.time)}</span>
      <Trash2 className="bin" />
    </div>
  );
}

const DevIcon = ({ d }: { d: Device }) =>
  d.platform === "android" ? <Smartphone /> : d.platform === "windows" && /笔记本|laptop/i.test(d.name) ? <Laptop /> : <Monitor />;

export function DevicesPage() {
  const { ov } = useApp();
  const box = useRef<HTMLDivElement>(null);
  useFlip(box, "right");
  return (
    <div className="page">
      <div className="meta"><b>设备组</b><span className="mono">{ov.relay?.addr || "仅局域网"}</span></div>
      <h1 className="page-title" style={{ marginTop: 14 }}>设备</h1>
      <p className="page-sub">设备之间的所有内容都经过端到端加密；中转服务器只转发无法解读的密文。移除设备会立即生效，被移除的设备无法用原来的身份重新加入。</p>
      <div className="scroll" style={{ marginTop: 26 }}>
        <Topology />
        <div ref={box}>
          {ov.devices.map((d, i) => (
            <DeviceRow key={d.id} d={d} index={i + 1} />
          ))}
        </div>
        <InvitePanel />
      </div>
    </div>
  );
}

function Topology() {
  const { ov } = useApp();
  const ref = useRef<SVGSVGElement>(null);
  const [W, setW] = useState(880);

  const [names, setNames] = useState({ me: 110, peer: 180 });
  const me = ov.devices.find((d) => d.me);
  const others = ov.devices.filter((d) => !d.me);
  const shown = !!me && others.length > 0;
  const measure = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const len = (sel: string) =>
      Math.max(0, ...Array.from(el.querySelectorAll<SVGTextElement>(sel), (t) => Math.ceil(t.getComputedTextLength())));
    const next = { me: len(".nd.me text"), peer: len(".nd.peer text.nm") };
    setNames((cur) => (next.me && next.peer && (next.me !== cur.me || next.peer !== cur.peer) ? next : cur));
  }, []);

  useLayoutEffect(measure);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      setW(Math.max(480, Math.round(el.getBoundingClientRect().width)));
      measure();
    });
    ro.observe(el);

    let alive = true;
    const onVis = () => document.visibilityState === "visible" && measure();
    document.fonts.ready.then(() => alive && measure());
    document.fonts.addEventListener("loadingdone", measure);
    document.addEventListener("visibilitychange", onVis);
    return () => {
      alive = false;
      ro.disconnect();
      document.fonts.removeEventListener("loadingdone", measure);
      document.removeEventListener("visibilitychange", onVis);
    };
  }, [measure, shown]);
  if (!shown) return null;

  const ROW = 44;
  const top = 64;
  const H = top + (others.length - 1) * ROW + 22;
  const meY = top + ((others.length - 1) * ROW) / 2;
  const STATUS = 64;
  const x1 = Math.min(names.me + 12, W * 0.3);
  const x2 = Math.max(x1 + 160, W - STATUS - 22 - names.peer);
  const relay = { x: Math.round((x1 + x2) / 2), y: 26 };
  const relayOn = !!ov.relay?.connected;
  const relayLabel = ov.relay?.configured ? (ov.relay.enabled ? (relayOn ? "中转服务器 · 已连接" : "中转服务器 · 连接中") : "中转服务器 · 已关闭") : "仅局域网";
  return (
    <svg className="topo" ref={ref} width="100%" height={H} viewBox={`0 0 ${W} ${H}`} role="img" aria-label="设备连接拓扑">
      {others.map((d, i) => {
        const y = top + i * ROW;
        if (d.lan) {
          const mid = (x1 + x2) / 2;
          return <path key={d.id} className="ln lan" d={`M${x1 + 6},${meY} C${mid},${meY} ${mid},${y} ${x2},${y}`} />;
        }
        if (d.relay) return <polyline key={d.id} className="ln relay" points={`${x1 + 6},${meY} ${relay.x},${relay.y + 6} ${x2},${y}`} />;
        return <line key={d.id} className="ln off" x1={x1 + 6} y1={meY} x2={x2} y2={y} />;
      })}
      <g className={`nd hub ${relayOn ? "on" : ""}`}>
        <rect x={relay.x - 6} y={relay.y} width={12} height={12} />
        <text x={relay.x} y={relay.y - 10} textAnchor="middle">{relayLabel}</text>
      </g>
      <g className="nd me">
        <rect x={x1} y={meY - 6} width={12} height={12} />
        <text x={x1 - 12} y={meY + 5} textAnchor="end">{me.name}</text>
      </g>
      {others.map((d, i) => {
        const y = top + i * ROW;
        const state = d.lan ? "lan" : d.relay ? "relay" : "off";
        return (
          <g key={d.id} className={`nd peer ${state}`}>
            <rect x={x2} y={y - 6} width={12} height={12} />
            <text className="nm" x={x2 + 22} y={y + 5}>{d.name}</text>
            <text className="st" x={W} y={y + 5} textAnchor="end">{{ lan: "局域网", relay: "经中转", off: "离线" }[state]}</text>
          </g>
        );
      })}
    </svg>
  );
}

function DeviceRow({ d, index }: { d: Device; index: number }) {
  const { api, toast, refresh } = useApp();
  const [confirm, setConfirm] = useState(false);
  useEffect(() => {
    if (!confirm) return;
    const t = setTimeout(() => setConfirm(false), 3000);
    return () => clearTimeout(t);
  }, [confirm]);
  const path = d.me ? "me" : d.lan ? "lan" : d.relay ? "relay" : "off";
  const pathLabel = { me: "本机", lan: "局域网直连", relay: "经中转", off: "离线" }[path];
  return (
    <div className="dev-row" data-id={d.id}>
      <span className="ix">{String(index).padStart(2, "0")}</span>
      <div className="nm">
        {d.name}
        <small>
          <DevIcon d={d} /> {d.platform.toUpperCase()} · {d.short}
        </small>
      </div>
      <div className={`path ${path}`}><span className="sq" />{pathLabel}</div>
      {d.me ? (
        <span />
      ) : (
        <button
          className={`op ${confirm ? "confirm" : ""}`}
          onClick={async () => {
            if (!confirm) return setConfirm(true);
            try {
              await api.removeDevice(d.id);
              toast(`已移除“${d.name}”`);
            } catch (e) {
              toast(String(e));
            }
            refresh();
          }}
        >
          <Trash2 />
          {confirm ? "确认移除？" : "移除"}
        </button>
      )}
    </div>
  );
}

const INVITE_TTL = 120;

function QrCode({ matrix }: { matrix: string[] }) {
  const n = matrix.length;
  let d = "";
  matrix.forEach((row, y) => {
    for (let x = 0; x < row.length; x++) if (row[x] === "1") d += `M${x} ${y}h1v1h-1z`;
  });
  return (
    <div className="qr">
      <svg viewBox={`0 0 ${n} ${n}`} shapeRendering="crispEdges" role="img" aria-label="配对二维码">
        <rect width={n} height={n} fill="#fff" />
        <path d={d} fill="#0a0a0a" />
      </svg>
    </div>
  );
}

function InvitePanel() {
  const { api, toast } = useApp();
  const [inv, setInv] = useState<{ uri: string; matrix: string[]; at: number } | null>(null);
  const [left, setLeft] = useState(INVITE_TTL);
  const panel = useRef<HTMLDivElement>(null);
  const bar = useRef<HTMLDivElement>(null);
  const addBtn = useRef<HTMLButtonElement>(null);
  useNudge(addBtn, 0);

  useEffect(() => {
    if (!inv) return;
    const t = setInterval(() => {
      const l = Math.max(0, INVITE_TTL - Math.floor((Date.now() - inv.at) / 1000));
      setLeft(l);
      if (l === 0) setInv(null);
    }, 250);
    return () => clearInterval(t);
  }, [inv]);

  useLayoutEffect(() => {
    if (!inv || !panel.current) return;
    const m = mo(panel.current);
    m.Y.set(40);
    m.O.set(0);
    m.Y.to(0, P.bouncy);
    m.O.to(1, P.gentle);

    const b = bar.current!;
    b.style.transition = "none";
    b.style.transform = "scaleX(1)";
    requestAnimationFrame(() => {
      b.style.transition = `transform ${INVITE_TTL}s linear`;
      b.style.transform = "scaleX(0)";
    });
  }, [inv]);

  if (!inv) {
    return (
      <div style={{ marginTop: 28 }}>
        <button
          ref={addBtn}
          className="btn-primary"
          onClick={async () => {
            try {
              const r = await api.createInvite();
              setInv({ ...r, at: Date.now() });
              setLeft(INVITE_TTL);
            } catch (e) {
              toast(String(e));
            }
          }}
        >
          <Plus /> 添加设备
        </button>
      </div>
    );
  }
  return (
    <div className="invite" ref={panel}>
      <QrCode matrix={inv.matrix} />
      <div>
        <div className="meta"><b>配对码</b><span className="mono">单次有效</span></div>
        <h3 style={{ marginTop: 10 }}>用新设备扫描二维码</h3>
        <p>
          手机：打开 CopyCat，选择“扫码加入”。<br />
          电脑：复制下面的配对码，在新电脑上选择“加入设备组”并粘贴。<br />
          对方连上后，这里会弹出确认码，请核对两边一致再同意。
        </p>
        <div className="ttl" ref={bar} />
        <div className="ttl-label">
          <span>剩余 {left} 秒</span>
          <span style={{ display: "flex", gap: 16 }}>
            <button
              onClick={async () => {
                await navigator.clipboard.writeText(inv.uri).catch(() => {});
                toast("配对码已复制（不会同步到其他设备）");
              }}
            >
              复制配对码
            </button>
            <button onClick={() => setInv(null)}>取消</button>
          </span>
        </div>
        <div className="uri">{inv.uri}</div>
      </div>
    </div>
  );
}

function Toggle({ on, onChange, disabled }: { on: boolean; onChange(v: boolean): void; disabled?: boolean }) {
  const knob = useRef<HTMLElement>(null);
  const first = useRef(true);
  useLayoutEffect(() => {
    const m = mo(knob.current!);
    if (first.current) {
      first.current = false;
      m.X.set(on ? 22 : 0);
    } else {
      m.X.to(on ? 22 : 0, P.bouncy);
      m.SX.set(1.5);
      m.SX.to(1, P.wobbly);
    }
  }, [on]);
  return (
    <button className={`toggle ${on ? "on" : ""}`} onClick={() => !disabled && onChange(!on)} role="switch" aria-checked={on} style={disabled ? { opacity: 0.4 } : undefined}>
      <i ref={knob} />
    </button>
  );
}

function SetRow({ title, desc, sub, disabled, children }: { title: string; desc?: string; sub?: boolean; disabled?: boolean; children: React.ReactNode }) {
  return (
    <div className={`set-row${sub ? " sub" : ""}${disabled ? " off" : ""}`}>
      <div className="l">
        <b>{title}</b>
        {desc && <span>{desc}</span>}
      </div>
      {children}
    </div>
  );
}

const MODS: [keyof KeyboardEvent, string][] = [
  ["ctrlKey", "Ctrl"],
  ["altKey", "Alt"],
  ["shiftKey", "Shift"],
  ["metaKey", "Super"],
];

function comboOf(e: React.KeyboardEvent): string | null {
  const code = e.code;
  if (/^(Control|Alt|Shift|Meta|OS)/.test(code) || !code) return null;
  const key = code.replace(/^Key/, "").replace(/^Digit/, "");
  const mods = MODS.filter(([k]) => e[k as "ctrlKey"]).map(([, n]) => n);

  if (!mods.length && !/^F\d+$/.test(key)) return null;
  return [...mods, key].join("+");
}

function HotkeyInput({ value, onChange }: { value: string; onChange(v: string): void }) {
  const [rec, setRec] = useState(false);
  return (
    <button
      className={`hotkey ${rec ? "rec" : ""}`}
      onClick={() => setRec(true)}
      onBlur={() => setRec(false)}
      onKeyDown={(e) => {
        if (!rec) return;
        e.preventDefault();
        if (e.key === "Escape") return setRec(false);
        if (e.key === "Backspace" && !e.ctrlKey && !e.altKey && !e.shiftKey) {
          setRec(false);
          return onChange("");
        }
        const c = comboOf(e);
        if (c) {
          setRec(false);
          onChange(c);
        }
      }}
    >
      <Keyboard />
      {rec ? "请按下组合键…" : value || "未设置"}
    </button>
  );
}

function MbInput({ value, onChange }: { value: number; onChange(v: number): void }) {
  const [v, setV] = useState(String(value));
  useEffect(() => setV(String(value)), [value]);
  const commit = () => {
    const n = Math.max(0, Math.min(100000, Math.round(Number(v))));
    if (Number.isFinite(n) && n !== value) onChange(n);
    else setV(String(value));
  };
  return (
    <span className="mb-input">
      <input value={v} inputMode="numeric" onChange={(e) => setV(e.target.value.replace(/\D/g, ""))} onBlur={commit} onKeyDown={(e) => e.key === "Enter" && commit()} />
      MB
    </span>
  );
}

function ConfirmButton({
  label,
  confirmLabel = "确认清空？",
  icon = <Trash2 />,
  onConfirm,
}: {
  label: string;
  confirmLabel?: string;
  icon?: React.ReactNode;
  onConfirm(): Promise<void>;
}) {
  const { toast } = useApp();
  const [confirm, setConfirm] = useState(false);
  useEffect(() => {
    if (!confirm) return;
    const t = setTimeout(() => setConfirm(false), 3000);
    return () => clearTimeout(t);
  }, [confirm]);
  return (
    <button
      className="danger"
      onClick={async () => {
        if (!confirm) return setConfirm(true);
        setConfirm(false);
        await onConfirm().catch((e) => toast(String(e)));
      }}
    >
      {icon}
      {confirm ? confirmLabel : label}
    </button>
  );
}

const ADDR_RE = /^[A-Za-z0-9.\-[\]:]+:\d{1,5}$/;

function RelaySection() {
  const { api, ov, toast, refresh } = useApp();
  const relay = ov.relay;
  const configured = !!relay?.configured;
  const [setup, setSetup] = useState<RelaySetup | null>(null);
  const [deploy, setDeploy] = useState(false);
  const [editing, setEditing] = useState(false);
  const [addr, setAddr] = useState("");
  const [busy, setBusy] = useState(false);
  const [arm, setArm] = useState<"regen" | "remove" | null>(null);

  useEffect(() => {
    if (!configured) return setSetup(null);
    api.relaySetup().then(setSetup).catch(() => setSetup(null));
  }, [api, configured, relay?.addr]);
  useEffect(() => {
    if (!arm) return;
    const t = setTimeout(() => setArm(null), 3000);
    return () => clearTimeout(t);
  }, [arm]);

  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
      refresh();
    } catch (e) {
      toast(String(e));
    } finally {
      setBusy(false);
    }
  };
  const generate = (a: string) =>
    run(async () => {
      const r = await api.setupRelay(a.trim());
      setSetup(r);
      setDeploy(true);
      setEditing(false);
      setAddr("");
    });
  const saveAddr = () =>
    run(async () => {
      await api.setRelayAddr(addr.trim());
      setEditing(false);
      toast("中转地址已修改，服务器上无需改动");
    });
  const submit = () => (editing ? saveAddr() : generate(addr));
  const valid = ADDR_RE.test(addr.trim());

  return (
    <>
      <SetRow
        title="中转服务器"
        desc={
          configured
            ? "跨网络同步经过这台服务器转发，它只能看到密文。配置会自动同步到设备组中的其他设备。"
            : "不配置也能在同一局域网内同步。需要跨网络同步时，填写 VPS 的域名或 IP 与端口，生成部署密钥。"
        }
      >
        {!configured || editing ? (
          <span className="relay-edit">
            <input
              value={addr}
              placeholder="relay.example.com:8443"
              spellCheck={false}
              autoFocus={editing}
              onChange={(e) => setAddr(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && valid && !busy) submit();
                if (e.key === "Escape" && editing) setEditing(false);
              }}
            />
            <button className="go" disabled={busy || !valid} onClick={submit}>{editing ? "保存" : "生成部署密钥"}</button>
            {editing && <button onClick={() => setEditing(false)}>取消</button>}
          </span>
        ) : (
          <div className="dir">
            <span className={`relay-v ${relay!.connected ? "ok" : ""}`}>
              {relay!.addr}
              <i>{relay!.connected ? "已连接" : relay!.enabled ? "连接中" : "本机已关闭"}</i>
            </span>
            <span className="dir-ops">
              {setup && <button onClick={() => setDeploy(!deploy)}>{deploy ? "收起部署配置" : "部署配置"}</button>}
              <button onClick={() => { setAddr(relay!.addr); setEditing(true); }}>修改地址</button>
              <button
                className={arm === "regen" ? "armed" : ""}
                disabled={busy}
                onClick={() => (arm === "regen" ? (setArm(null), generate(relay!.addr)) : setArm("regen"))}
              >
                {arm === "regen" ? "旧密钥将失效，确认？" : "重新生成密钥"}
              </button>
              <button
                className={arm === "remove" ? "armed" : ""}
                disabled={busy}
                onClick={() =>
                  arm === "remove"
                    ? (setArm(null), run(async () => { await api.removeRelay(); setDeploy(false); toast("已停用中转服务器，只在局域网内同步"); }))
                    : setArm("remove")
                }
              >
                {arm === "remove" ? "确认停用？" : "停用"}
              </button>
            </span>
          </div>
        )}
      </SetRow>
      {deploy && setup && <RelayDeploy setup={setup} onClose={() => setDeploy(false)} />}
      {configured && (
        <SetRow title="本机使用中转服务器" desc="只影响本机：关闭后本机只在同一局域网内同步（仅局域网模式）。">
          <Toggle on={!!relay?.enabled} onChange={async (v) => { await api.setRelayEnabled(v); refresh(); }} />
        </SetRow>
      )}
    </>
  );
}

function RelayDeploy({ setup, onClose }: { setup: RelaySetup; onClose(): void }) {
  const { toast } = useApp();
  const panel = useRef<HTMLDivElement>(null);
  const port = setup.addr.split(":").pop();
  useLayoutEffect(() => {
    const m = mo(panel.current!);
    m.Y.set(24);
    m.O.set(0);
    m.Y.to(0, P.bouncy);
    m.O.to(1, P.gentle);
  }, []);
  const copy = async (text: string, what: string) => {
    await navigator.clipboard.writeText(text).catch(() => {});
    toast(`${what}已复制（不会同步到其他设备）`);
  };
  return (
    <div className="deploy" ref={panel}>
      <div className="meta"><b>部署</b><span className="mono">COPYCAT_RELAY_KEY</span></div>
      <h3>把密钥填到 VPS 上</h3>
      <ol className="steps">
        <li><b>01</b>在 VPS 上新建目录，下载仓库中的 deploy/docker/setup.sh。</li>
        <li><b>02</b>点“复制 compose.yml”，在该目录中保存为同名文件（端口 {port} 与密钥已经填好）。</li>
        <li><b>03</b>运行 bash setup.sh，自动拉取镜像并启动。云服务商的安全组需放行 UDP 与 TCP {port} 端口。</li>
      </ol>
      <div className="key">{setup.key}</div>
      <div className="ops">
        <button className="btn-primary" onClick={() => copy(setup.compose, "compose.yml ")}><Copy /> 复制 compose.yml</button>
        <button className="btn-ghost" onClick={() => copy(setup.key, "密钥")}>复制密钥</button>
        <button className="btn-ghost" onClick={onClose}>收起</button>
      </div>
      <p className="note">密钥就是服务器的身份，只保存在本机，请勿外传。重新生成后旧密钥立即失效，需要同时更新服务器。</p>
    </div>
  );
}

function AppChips({ apps, placeholder, onChange }: { apps: string[]; placeholder: string; onChange(v: string[]): void }) {
  const [v, setV] = useState("");
  return (
    <div className="chips">
      {apps.map((a) => (
        <span className="chip" key={a}>
          {a}
          <button onClick={() => onChange(apps.filter((x) => x !== a))} aria-label={`移除 ${a}`}><X /></button>
        </span>
      ))}
      <span className="chip-add">
        <input
          value={v}
          placeholder={placeholder}
          spellCheck={false}
          onChange={(e) => setV(e.target.value)}
          onKeyDown={(e) => {
            const t = v.trim();
            if (e.key === "Enter" && t && !apps.some((x) => x.toLowerCase() === t.toLowerCase())) {
              onChange([...apps, t]);
              setV("");
            }
          }}
        />
      </span>
    </div>
  );
}

export function SettingsPage() {
  const { api, ov, toast, refresh } = useApp();
  const s = ov.settings;
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const shotPhones = ov.devices.filter((d) => !d.me && d.screenshots && (d.relay || d.lan));
  useEffect(() => {
    api.autostart.get().then(setAutostart).catch(() => setAutostart(false));
  }, [api]);
  const save = async (patch: Partial<Settings>, note?: string) => {
    try {
      await api.saveSettings({ ...s, ...patch });
      if (note) toast(note);
      refresh();
    } catch (e) {
      toast(String(e));
    }
  };
  const me = ov.me;
  const relay = ov.relay;
  const relayState = !relay?.configured ? "LAN ONLY" : relay.connected ? "ONLINE" : "OFFLINE";

  return (
    <div className="page">
      <div className="meta"><b>偏好</b><span className="mono">v{__APP_VERSION__}</span></div>
      <h1 className="page-title" style={{ marginTop: 14 }}>设置</h1>
      <div className="scroll" style={{ marginTop: 16 }}>
        <div className="set-group">
          <h4><span>连接</span><span className="mono">{relayState}</span></h4>
          <RelaySection />
          <SetRow title="局域网直连" desc={`同一网络下设备直接连接，延迟更低。${ov.lanPort ? `监听端口 ${ov.lanPort}。` : ""}重启应用后生效。`}>
            <Toggle on={s.lanEnabled} onChange={(v) => save({ lanEnabled: v }, "重启应用后生效")} />
          </SetRow>
          <SetRow title="离线信箱" desc="对方离线时，把小于 256 KB 的内容加密暂存在中转服务器，最长 24 小时。关闭后获得完整的前向保密。重启应用后生效。">
            <Toggle on={s.mailbox} onChange={(v) => save({ mailbox: v }, "重启应用后生效")} />
          </SetRow>
        </div>

        <div className="set-group">
          <h4><span>同步</span><span className="mono">{ov.paused ? "PAUSED" : "ACTIVE"}</span></h4>
          <SetRow title="暂停同步" desc="暂停期间既不发送本机复制的内容，也不写入收到的内容。">
            <Toggle on={ov.paused} onChange={async (v) => { await api.setPaused(v); refresh(); }} />
          </SetRow>
          <SetRow title="同步图片" desc="复制的图片同步到其他电脑，单张最大 64 MB。较大的图片先只同步说明，对方需要粘贴时再取回原图。">
            <Toggle on={s.syncImages} onChange={(v) => save({ syncImages: v })} />
          </SetRow>
          <SetRow title="不同步这些程序" desc="从这些程序复制的内容不会离开本机。密码管理器通常还会自行标记为敏感内容，同样不会同步。">
            <AppChips apps={s.excludedApps} placeholder="添加，如 WeChat.exe" onChange={(v) => save({ excludedApps: v })} />
          </SetRow>
        </div>

        <div className="set-group">
          <h4><span>截图</span><span className="mono">{shotPhones.length ? `→ ${shotPhones.map((d) => d.name).join(" · ")}` : "没有手机接收"}</span></h4>
          <SetRow title="截图自动发到手机" desc="截图后自动把图片发到手机的 screenshot 文件夹。手机上也要打开“接收电脑截图”，右上角是当前在线、会收到截图的手机。">
            <Toggle on={s.screenshots} onChange={(v) => save({ screenshots: v })} />
          </SetRow>
          <SetRow title="截图工具" desc="从这些程序复制的图片算作截图，按 PrtScn 键截的也算。加入 Weixin.exe 后，微信里复制的图片也会被发送。">
            <AppChips apps={s.screenshotTools} placeholder="添加，如 Weixin.exe" onChange={(v) => save({ screenshotTools: v })} />
          </SetRow>
        </div>

        <div className="set-group">
          <h4><span>文件传输</span><span className="mono">{ov.transfers.length} TRANSFERS</span></h4>
          <SetRow title="保存位置" desc={s.askReceiveDir ? "每次收到文件时选择；对话框从这里开始。" : "其他设备发来的文件直接保存到这里。"}>
            <div className="dir">
              <span className="v" title={ov.receiveDir}>{ov.receiveDir}</span>
              <span className="dir-ops">
                <button onClick={() => api.openReceiveDir().catch((e) => toast(String(e)))}><FolderOpen />打开</button>
                <button
                  onClick={async () => {
                    const d = await api.pickFolder("选择接收文件的保存位置").catch(() => null);
                    if (d) save({ receiveDir: d }, "保存位置已更改");
                  }}
                >
                  更改
                </button>
                {s.receiveDir && <button onClick={() => save({ receiveDir: "" }, "已恢复默认位置")}>恢复默认</button>}
              </span>
            </div>
          </SetRow>
          <SetRow title="每次询问保存位置" desc="收到文件时弹出对话框选择文件夹；取消即拒绝接收。">
            <Toggle on={s.askReceiveDir} onChange={(v) => save({ askReceiveDir: v })} />
          </SetRow>
          <SetRow title="收到后复制到剪贴板" desc="收到文件后直接复制，在资源管理器、聊天窗口里粘贴即可。">
            <Toggle on={s.copyReceived} onChange={(v) => save({ copyReceived: v })} />
          </SetRow>
          <SetRow sub title="图片复制为图片" desc="只收到一张图片时复制图片内容，可直接粘贴进文档或聊天框。" disabled={!s.copyReceived}>
            <Toggle on={s.copyReceivedImage} disabled={!s.copyReceived} onChange={(v) => save({ copyReceivedImage: v })} />
          </SetRow>
          <SetRow title="用浏览器打开分享的链接" desc="手机上把链接“分享”到 CopyCat 时，直接在默认浏览器中打开。关闭后只复制到剪贴板。">
            <Toggle on={s.openSharedLinks} onChange={(v) => save({ openSharedLinks: v })} />
          </SetRow>
          <SetRow title="发送文件快捷键" desc="在任何地方按下即弹出发送窗口。点击后按新的组合键修改，按退格键清除。">
            <HotkeyInput value={s.sendHotkey} onChange={(v) => save({ sendHotkey: v }, v ? `快捷键已改为 ${v}` : "已关闭快捷键")} />
          </SetRow>
          <SetRow title="经中转发送前确认" desc="不在同一局域网、只能经中转服务器发送时，超过这个大小先确认。设为 0 则从不确认。">
            <MbInput value={s.relayConfirmMb} onChange={(v) => save({ relayConfirmMb: v })} />
          </SetRow>
        </div>

        <div className="set-group">
          <h4><span>系统</span><span /></h4>
          <SetRow title="开机自动启动" desc="登录 Windows 后在托盘中静默运行。">
            <Toggle
              on={!!autostart}
              disabled={autostart === null}
              onChange={async (v) => {
                try {
                  await api.autostart.set(v);
                  setAutostart(v);
                } catch (e) {
                  toast(String(e));
                }
              }}
            />
          </SetRow>
        </div>

        <div className="set-group">
          <h4><span>数据</span><span className="mono">{ov.history.length} ITEMS</span></h4>
          <SetRow title="历史保留" desc="超过这个时间的记录自动删除，每台设备最多保留 50、200 或 500 条。">
            <div className="seg" role="radiogroup">
              {[1, 7, 30].map((d) => (
                <button key={d} role="radio" aria-checked={s.historyDays === d} className={s.historyDays === d ? "on" : ""} onClick={() => s.historyDays !== d && save({ historyDays: d })}>
                  {d === 1 ? "1 天" : `${d} 天`}
                </button>
              ))}
            </div>
          </SetRow>
          <SetRow title="清空本机历史" desc="只删除本机保存的记录，不影响其他设备。">
            <ConfirmButton label="清空" onConfirm={async () => { await api.clearHistory(); toast("本机历史已清空"); }} />
          </SetRow>
          <SetRow title="清空所有设备的历史" desc="同时请当前在线的其他设备删除全部记录，例如误复制了密码时。离线的设备不受影响。">
            <ConfirmButton
              label="全部清空"
              onConfirm={async () => {
                const n = await api.clearHistory(true);
                toast(n ? `已清空本机，并通知了 ${n} 台在线设备` : "本机历史已清空（没有在线的其他设备）");
              }}
            />
          </SetRow>
          <SetRow title="本机" desc={me ? `设备 ID ${me.short}` : undefined}>
            <span className="v">{me?.name}</span>
          </SetRow>
          <SetRow title="退出设备组" desc="通知其他设备把本机移出，然后删除本机的设备身份、剪贴板历史与中转密钥，回到初始设置。本机设置与收到的文件保留。">
            <ConfirmButton
              label="退出"
              confirmLabel="确认退出？"
              icon={<LogOut />}
              onConfirm={async () => {
                const r = await api.leaveGroup();
                toast(
                  r.peers
                    ? `已退出设备组，并通知了 ${r.peers} 台在线设备`
                    : r.relay
                      ? "已退出设备组，离线的设备连上中转服务器后会收到通知"
                      : "已退出设备组。没有在线的设备，请在其他设备上手动移除本机",
                );
              }}
            />
          </SetRow>
        </div>
      </div>
    </div>
  );
}
