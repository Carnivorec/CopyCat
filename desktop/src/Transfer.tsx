import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ArrowDownLeft, ArrowRight, ArrowUpRight, FolderOpen, Plus, X } from "lucide-react";
import { AppCtx, Toast, useApp } from "./App";
import { api as apiPromise, type Api, type Device, type Overview, type Transfer } from "./api";
import { baseName, fmtEta, fmtRate, fmtSize, fmtTime } from "./format";
import { useFlip, useNudge } from "./motion";
import { mo, P } from "./spring";

const MB = 1024 * 1024;

const rank = (d: Device) => (d.lan ? 0 : d.relay ? 1 : 2);

const uniq = (a: string[]) => Array.from(new Set(a));

type PanelProps = {
  paths: string[];
  setPaths(p: string[]): void;

  over: boolean;
  compact?: boolean;
  onSent?(d: Device): void;
};

export function SendPanel({ paths, setPaths, over, compact, onSent }: PanelProps) {
  const { api, ov } = useApp();
  const others = ov.devices.filter((d) => !d.me);
  const [picked, setPicked] = useState<string | null>(null);
  const [sum, setSum] = useState<{ count: number; total: number } | null>(null);
  const [err, setErr] = useState("");
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const zone = useRef<HTMLDivElement>(null);
  const go = useRef<HTMLButtonElement>(null);
  useNudge(go, 4);

  const target = others.find((d) => d.id === picked) ?? [...others].sort((a, b) => rank(a) - rank(b))[0] ?? null;

  useEffect(() => {
    setConfirm(false);
    setErr("");
    if (!paths.length) return setSum(null);
    let live = true;
    setSum(null);
    api
      .summarize(paths)
      .then((s) => live && setSum(s))
      .catch((e) => live && setErr(String(e)));
    return () => {
      live = false;
    };
  }, [api, paths]);

  useLayoutEffect(() => {
    if (zone.current) mo(zone.current).S.to(over ? 1.02 : 1, over ? P.snappy : P.bouncy);
  }, [over]);

  const limit = ov.settings.relayConfirmMb * MB;
  const needConfirm = !!(target && sum && !target.lan && ov.settings.relayConfirmMb > 0 && sum.total > limit);

  const add = async () => {
    const more = await api.pickFiles().catch(() => []);
    if (more.length) setPaths(uniq([...paths, ...more]));
  };

  const send = async () => {
    if (!target || !sum) return;
    if (needConfirm && !confirm) return setConfirm(true);
    setBusy(true);
    try {
      await api.sendPaths(target.id, paths);
      setPaths([]);
      onSent?.(target);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const label = !target
    ? "没有可发送的设备"
    : confirm
      ? `确认经中转发送 ${sum ? fmtSize(sum.total) : ""}`
      : target.lan || target.relay
        ? `发送给 ${target.name}`
        : `等 ${target.name} 上线后发送`;

  return (
    <div className={`send ${compact ? "compact" : ""}`}>
      <div className={`drop ${paths.length ? "has" : ""} ${over ? "over" : ""}`} ref={zone}>
        {paths.length === 0 ? (
          <div className="drop-empty">
            <b>拖入文件或文件夹</b>
            <button onClick={add}><Plus />选择文件</button>
          </div>
        ) : (
          <>
            <div className="drop-count">
              <span className="n">{sum?.count ?? "…"}</span>
              <span>个文件<br /><em className="mono">{sum ? fmtSize(sum.total) : "统计中"}</em></span>
            </div>
            <ul className="drop-list">
              {paths.map((p) => (
                <li key={p} title={p}>
                  <span>{baseName(p)}</span>
                  <button aria-label={`移除 ${baseName(p)}`} title="移除" onClick={() => setPaths(paths.filter((x) => x !== p))}>
                    <X />
                  </button>
                </li>
              ))}
            </ul>
            <div className="drop-ops">
              <button onClick={add}><Plus />添加</button>
              <button onClick={() => setPaths([])}><X />清空</button>
            </div>
          </>
        )}
      </div>
      <div className="targets">
        <h4><span>发送给</span><span className="mono">{others.length} {others.length === 1 ? "DEVICE" : "DEVICES"}</span></h4>
        <div className="target-list">
          {others.map((d) => (
            <button
              key={d.id}
              className={`target ${d.id === target?.id ? "on" : ""}`}
              onClick={() => {
                setPicked(d.id);
                setConfirm(false);
              }}
            >
              <span className="sq" />
              <b>{d.name}</b>
              <span className={`st ${d.lan ? "lan" : d.relay ? "relay" : ""}`}>{d.lan ? "局域网" : d.relay ? "经中转" : "离线"}</span>
            </button>
          ))}
          {others.length === 0 && <div className="list-empty">还没有其他设备，先在“设备”页添加。</div>}
        </div>
        {err ? (
          <div className="send-note err">{err}</div>
        ) : confirm ? (
          <div className="send-note warn">只能经中转服务器发送，会占用服务器流量。再点一次确认。</div>
        ) : needConfirm ? (
          <div className="send-note">不在同一局域网，将经中转服务器发送。</div>
        ) : null}
        <button ref={go} className={`btn-primary send-go ${confirm ? "confirm" : ""}`} disabled={!target || !sum || busy} onClick={send}>
          <span>{label}</span>
          <ArrowRight />
        </button>
      </div>
    </div>
  );
}

function title(t: Transfer): string {
  if (t.text !== null) return t.text.replace(/\s+/g, " ");
  const first = t.files[0]?.name ?? "";
  const top = first.split("/")[0];
  if (t.count === 1) return baseName(first);

  if (first.includes("/") && t.files.every((f) => f.name.startsWith(top + "/"))) return top;
  return `${baseName(first)} 等`;
}

function status(t: Transfer, online: boolean): [string, string] {
  const pct = t.total ? Math.floor((t.done / t.total) * 100) : 0;
  switch (t.state) {
    case "preparing":
      return ["正在准备", "计算校验值"];
    case "waiting":
      return t.outgoing ? [online ? "等待对方" : "等对方上线", t.done ? `${pct}%` : ""] : ["已暂停", `${pct}%`];
    case "asking":
      return ["等待确认", "选择保存位置"];
    case "active":
      return [`${pct}%`, t.rate ? `${fmtRate(t.rate)} · ${fmtEta((t.total - t.done) / t.rate)}` : ""];
    case "done":
      return [t.outgoing ? "已送达" : t.text !== null ? "已收到" : "已保存", fmtTime(t.finished ?? t.time)];
    case "failed":
      return ["失败", t.error ?? ""];
    case "cancelled":
      return ["已取消", fmtTime(t.finished ?? t.time)];
  }
}

const running = (t: Transfer) => !["done", "failed", "cancelled"].includes(t.state);

function TransferRow({ t, index }: { t: Transfer; index: number }) {
  const { api, ov, toast } = useApp();
  const online = ov.devices.some((d) => d.id === t.deviceId && (d.lan || d.relay));
  const [main, sub] = status(t, online);
  const pct = t.total ? t.done / t.total : t.state === "done" ? 1 : 0;
  const bar = useRef<HTMLElement>(null);

  useLayoutEffect(() => {
    if (bar.current) mo(bar.current).SX.to(pct, P.gentle);
  }, [pct]);
  useLayoutEffect(() => {
    if (bar.current) mo(bar.current).SX.set(pct);

    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const size = t.text !== null ? `${t.text.length} 字` : `${t.count > 1 ? `${t.count} 个文件 · ` : ""}${fmtSize(t.total)}`;
  return (
    <div className={`row xfer ${t.state}`} data-id={t.id}>
      <span className="ix">{String(index).padStart(2, "0")}</span>
      <div className="xf-name">
        <span className="tx">
          <span className="k">{t.kind === "screenshot" ? "截图" : t.text !== null ? "分享" : t.outgoing ? "发送" : "接收"}</span>
          {title(t)}
        </span>
        <small className="mono">{size}</small>
      </div>
      <span className="dv">
        {t.outgoing ? <ArrowUpRight /> : <ArrowDownLeft />} {t.device}
        <small className="mono">{running(t) ? (t.lan ? "局域网" : "中转") : ""}</small>
      </span>
      <div className="xf-st">
        <b>{main}</b>
        <small title={sub}>{sub}</small>
        {running(t) ? (
          <button onClick={() => api.cancelTransfer(t.id).catch((e) => toast(String(e)))}>{t.state === "asking" ? "拒绝" : "取消"}</button>
        ) : !t.outgoing && t.state === "done" && t.text === null ? (
          <button onClick={() => api.revealTransfer(t.id).catch((e) => toast(String(e)))}><FolderOpen />打开</button>
        ) : null}
      </div>
      {running(t) && <i className="xf-bar" ref={bar} />}
    </div>
  );
}

export function TransferPage() {
  const { api, ov, toast, pending, setPending, over } = useApp();
  const box = useRef<HTMLDivElement>(null);
  useFlip(box);
  const hotkey = ov.settings.sendHotkey;
  const done = ov.transfers.filter((t) => !running(t)).length;
  return (
    <div className="page">
      <div className="meta">
        <b>文件</b>
        <span className="mono">{hotkey ? `${hotkey} 随时发送` : ""}</span>
      </div>
      <h1 className="page-title" style={{ marginTop: 14 }}>传输</h1>
      <SendPanel compact paths={pending} setPaths={setPending} over={over} onSent={(d) => toast(`正在发送给 ${d.name}`)} />
      <div className="list-head xfer-head">
        <span>No.</span>
        <span>内容</span>
        <span>设备</span>
        <span className="st-head">
          {done > 0 && <button className="clear" onClick={() => api.clearTransfers().catch((e) => toast(String(e)))}>清除已结束</button>}
          状态
        </span>
      </div>
      <div className="list" ref={box}>
        {ov.transfers.map((t, i) => (
          <TransferRow key={t.id} t={t} index={i + 1} />
        ))}
        {ov.transfers.length === 0 && (
          <div className="list-empty">
            还没有传输记录。收到的文件保存在 <span className="mono">{ov.receiveDir}</span>
          </div>
        )}
      </div>
    </div>
  );
}

export function SendApp() {
  const [api, setApi] = useState<Api | null>(null);
  const [ov, setOv] = useState<Overview | null>(null);
  const [paths, setPaths] = useState<string[]>([]);
  const [over, setOver] = useState(false);
  const [sent, setSent] = useState<string | null>(null);
  const toastRef = useRef<(m: string) => void>(() => {});

  useEffect(() => {
    apiPromise.then(setApi);
  }, []);

  useEffect(() => {
    if (!api) return;
    const load = () => api.overview().then(setOv).catch(() => {});

    const takeSelected = () =>
      api
        .takeSendPaths()
        .then((more) => more.length && setPaths((p) => uniq([...p, ...more])))
        .catch(() => {});
    load();
    takeSelected();
    const offs = [
      api.on("cc:changed", load),
      api.on("cc:send-open", () => {
        setPaths([]);
        setSent(null);
        load();
      }),
      api.on("cc:send-paths", takeSelected),
      api.onDrop((e) => {
        setOver(e.over);
        if (e.paths?.length) {
          setSent(null);
          setPaths((p) => uniq([...p, ...e.paths!]));
        }
      }),
    ];
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") api.win.hide();
    };
    addEventListener("keydown", key);
    return () => {
      offs.forEach((p) => p.then((off) => off()));
      removeEventListener("keydown", key);
    };
  }, [api]);

  useEffect(() => {
    if (!sent || !api) return;
    const t = setTimeout(() => api.win.hide(), 1100);
    return () => clearTimeout(t);
  }, [sent, api]);

  if (!api || !ov) return <div className="app" />;
  const ctx = {
    api,
    ov,
    refresh: () => void api.overview().then(setOv),
    toast: (m: string) => toastRef.current(m),
    pending: paths,
    setPending: setPaths,
    over,
  };
  return (
    <AppCtx.Provider value={ctx}>
      <div className="app send-app">
        <header className="titlebar" data-tauri-drag-region>
          <div className="wm"><i />发送文件</div>
          <div className="right">
            <div className="state">ESC 收起</div>
            <div className="wc">
              <button className="x" onClick={() => api.win.hide()} aria-label="收起"><X /></button>
            </div>
          </div>
        </header>
        <div className="send-body">
          {sent ? (
            <div className="send-done">
              <b>已开始发送</b>
              <span>发给 {sent}。进度见主窗口的“传输”页，完成后会有系统通知。</span>
              <button className="btn-ghost" onClick={() => api.showMain()}>打开主窗口</button>
            </div>
          ) : (
            <SendPanel paths={paths} setPaths={setPaths} over={over} onSent={(d) => setSent(d.name)} />
          )}
        </div>
        <Toast bind={toastRef} />
      </div>
    </AppCtx.Provider>
  );
}
