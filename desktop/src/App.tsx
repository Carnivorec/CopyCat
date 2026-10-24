import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Check, Minus, Square, X } from "lucide-react";
import { api as apiPromise, type Api, type Overview, type PairRequest } from "./api";
import { mo, P } from "./spring";
import { Odometer, useNudge } from "./motion";
import { ClipboardPage, DevicesPage, SettingsPage } from "./pages";
import { TransferPage } from "./Transfer";
import { Onboarding } from "./Onboarding";

export type Page = "clipboard" | "devices" | "transfer" | "settings";
const PAGES: [Page, string][] = [
  ["clipboard", "剪贴板"],
  ["devices", "设备"],
  ["transfer", "传输"],
  ["settings", "设置"],
];

type Ctx = {
  api: Api;
  ov: Overview;
  refresh(): void;
  toast(msg: string): void;

  pending: string[];
  setPending(p: string[]): void;

  over: boolean;
};
export const AppCtx = createContext<Ctx>(null as never);
export const useApp = () => useContext(AppCtx);

export function App() {
  const [api, setApi] = useState<Api | null>(null);
  const [ov, setOv] = useState<Overview | null>(null);
  const [page, setPage] = useState<Page>((new URLSearchParams(location.search).get("page") as Page) || "clipboard");
  const [pair, setPair] = useState<PairRequest | null>(null);
  const [revoked, setRevoked] = useState(false);
  const [pending, setPending] = useState<string[]>([]);
  const [over, setOver] = useState(false);
  const toastRef = useRef<(m: string) => void>(() => {});
  const timer = useRef(0);

  useEffect(() => {
    apiPromise.then(setApi);
  }, []);

  const refresh = useCallback(() => {
    if (!api) return;
    clearTimeout(timer.current);

    timer.current = window.setTimeout(() => api.overview().then(setOv).catch(() => {}), 40);
  }, [api]);

  useEffect(() => {
    if (!api) return;
    api.overview().then(setOv);
    const offs = [
      api.on("cc:changed", refresh),
      api.on("cc:arrived", refresh),
      api.on("cc:pair-request", (p) => setPair(p)),
      api.on("cc:paired", () => {
        toastRef.current("新设备已加入");
        refresh();
      }),
      api.on("cc:revoked", () => setRevoked(true)),

      api.on("cc:transfer", (t) =>
        setOv((o) => {
          if (!o) return o;
          const i = o.transfers.findIndex((x) => x.id === t.id);
          const transfers = i < 0 ? [t, ...o.transfers] : o.transfers.map((x) => (x.id === t.id ? t : x));
          return { ...o, transfers };
        }),
      ),
      api.on("cc:shared", (s) => toastRef.current(s.opened ? `已打开 ${s.device} 分享的链接` : `${s.device} 分享的文字已复制`)),

      api.onDrop((e) => {
        setOver(e.over);
        if (e.paths?.length) {
          setPage("transfer");
          setPending((p) => Array.from(new Set([...p, ...e.paths!])));
        }
      }),
    ];
    const key = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "g") document.body.classList.toggle("show-grid");
    };
    addEventListener("keydown", key);
    return () => {
      offs.forEach((p) => p.then((off) => off()));
      removeEventListener("keydown", key);
    };
  }, [api, refresh]);

  if (!api || !ov) return <div className="app" />;
  const ctx: Ctx = { api, ov, refresh, toast: (m) => toastRef.current(m), pending, setPending, over };

  return (
    <AppCtx.Provider value={ctx}>
      <div className="app">
        <TitleBar />
        <div className="body">
          <div className="gridlines">{Array.from({ length: 12 }, (_, i) => <i key={i} />)}</div>
          {ov.initialized ? (
            <div className="layout">
              <Side page={page} setPage={setPage} />
              <main className="main">
                {page === "clipboard" && <ClipboardPage />}
                {page === "devices" && <DevicesPage />}
                {page === "transfer" && <TransferPage />}
                {page === "settings" && <SettingsPage />}
              </main>
            </div>
          ) : (
            <Onboarding />
          )}
          {pair && <PairSheet req={pair} onDone={() => setPair(null)} />}
          {revoked && (
            <div className="cover">
              <h2>本机已被移出设备组</h2>
              <p>同步已停止。点“重新开始”删除本机的设备身份与历史，之后可以重新创建或加入设备组。</p>
              <button className="btn-primary" onClick={() => api?.leaveGroup().then(() => setRevoked(false)).catch((e) => toastRef.current(String(e)))}>重新开始</button>
            </div>
          )}
        </div>
        <Toast bind={toastRef} />
      </div>
    </AppCtx.Provider>
  );
}

function TitleBar() {
  const { api, ov } = useApp();
  const online = ov.devices.filter((d) => !d.me && (d.lan || d.relay)).length;
  const lan = ov.devices.some((d) => d.lan);
  const state = !ov.initialized
    ? "未加入设备组"
    : ov.paused
      ? "已暂停同步"
      : ov.relay?.connected
        ? online > 0
          ? `${online} 台在线${lan ? " · 局域网直连" : ""}`
          : "已连接 · 暂无其他设备在线"
        : online > 0
          ? `${online} 台在线 · 仅局域网`
          : !ov.relay?.configured
            ? "等待局域网内的设备"
            : ov.relay.enabled
              ? "正在连接中转服务器"
              : "中转已关闭";
  const live = ov.initialized && !ov.paused && (ov.relay?.connected || online > 0);
  return (
    <header className="titlebar" data-tauri-drag-region onDoubleClick={() => api.win.toggleMaximize()}>
      <div className="wm">
        <i />
        CopyCat
      </div>
      <div className="right">
        <div className="state">
          <span className={`dot ${live ? "on" : ""}`} />
          {state}
        </div>
        <div className="wc">
          <button onClick={() => api.win.minimize()} aria-label="最小化"><Minus /></button>
          <button onClick={() => api.win.toggleMaximize()} aria-label="最大化"><Square size={13} /></button>
          <button className="x" onClick={() => api.win.hide()} aria-label="关闭到托盘"><X /></button>
        </div>
      </div>
    </header>
  );
}

function Side({ page, setPage }: { page: Page; setPage(p: Page): void }) {
  const { api, ov, refresh } = useApp();
  const counts: Record<Page, [number, string]> = {
    clipboard: [ov.history.length, "条剪贴板"],
    devices: [ov.devices.length, "台设备"],
    transfer: [ov.transfers.length, "项传输"],
    settings: [ov.history.length, "条剪贴板"],
  };
  const [n, label] = counts[page];
  return (
    <aside className="side">
      <Odometer value={n} className="count" />
      <div className="count-label">
        <span>{label}</span>
        <span>{page === "clipboard" ? (ov.settings.historyDays > 1 ? `保留 ${ov.settings.historyDays} 天` : "保留 24 H") : page === "transfer" ? `${ov.transfers.filter((t) => ["active", "waiting", "preparing", "asking"].includes(t.state)).length} 进行中` : "CopyCat"}</span>
      </div>
      <nav className="nav">
        {PAGES.map(([p, t]) => (
          <NavItem key={p} on={p === page} label={t} onClick={() => setPage(p)} />
        ))}
      </nav>
      <div className="devs">
        {ov.devices
          .filter((d) => !d.me)
          .map((d) => (
            <div className="row" key={d.id}>
              <b>{d.name}</b>
              <span className={`st ${d.lan ? "lan" : d.relay ? "relay" : ""}`}>{d.lan ? "LAN" : d.relay ? "中转" : "离线"}</span>
            </div>
          ))}
        {ov.devices.length <= 1 && (
          <div className="row">
            <b style={{ color: "#9a9a9a" }}>还没有其他设备</b>
            <span />
          </div>
        )}
        <button
          className={`pause ${ov.paused ? "on" : ""}`}
          onClick={async () => {
            await api.setPaused(!ov.paused);
            refresh();
          }}
        >
          <span>{ov.paused ? "同步已暂停" : "正在同步"}</span>
          <span className="mono">{ov.paused ? "继续" : "暂停"}</span>
        </button>
      </div>
    </aside>
  );
}

function NavItem({ on, label, onClick }: { on: boolean; label: string; onClick(): void }) {
  const ref = useRef<HTMLButtonElement>(null);
  const sq = useRef<HTMLSpanElement>(null);
  useNudge(ref, 6);
  const first = useRef(true);
  useLayoutEffect(() => {
    const m = mo(sq.current!);
    if (first.current) {
      m.S.set(on ? 1 : 0);
      first.current = false;
    } else m.S.to(on ? 1 : 0, P.bouncy);
  }, [on]);
  return (
    <button ref={ref} className={on ? "on" : ""} onClick={onClick}>
      <span ref={sq} className="sq" />
      {label}
    </button>
  );
}

export function Toast({ bind }: { bind: React.RefObject<(m: string) => void> }) {
  const el = useRef<HTMLDivElement>(null);
  const [msg, setMsg] = useState("");
  const hide = useRef(0);
  useLayoutEffect(() => {
    const m = mo(el.current!);
    m.Y.set(120);
    m.X.set(0);
    bind.current = (text: string) => {
      setMsg(text);
      requestAnimationFrame(() => {
        const node = el.current!;
        node.style.marginLeft = `${-node.offsetWidth / 2}px`;
        m.Y.set(Math.min(m.y, 120));
        m.Y.to(0, P.bouncy);
      });
      clearTimeout(hide.current);
      hide.current = window.setTimeout(() => m.Y.to(120, P.snappy), 1600);
    };
  }, [bind]);
  return (
    <div className="toast" ref={el}>
      <Check />
      {msg}
    </div>
  );
}

function PairSheet({ req, onDone }: { req: PairRequest; onDone(): void }) {
  const { api, toast, refresh } = useApp();
  const sheet = useRef<HTMLDivElement>(null);
  const back = useRef<HTMLDivElement>(null);
  const [busy, setBusy] = useState(false);
  useLayoutEffect(() => {
    const m = mo(sheet.current!);
    m.Y.set(320);
    m.Y.to(0, P.bouncy);
    const b = mo(back.current!);
    b.O.set(0);
    b.O.to(1, P.gentle);
  }, []);
  const close = async (accept: boolean) => {
    setBusy(true);
    try {
      await api.respondPairing(accept);
      if (accept) toast(`“${req.name}”已加入`);
    } catch (e) {
      toast(String(e));
    }
    refresh();
    await Promise.all([mo(sheet.current!).Y.to(360, P.snappy), mo(back.current!).O.to(0, P.snappy)]);
    onDone();
  };
  const sas = `${req.sas.slice(0, 3)} ${req.sas.slice(3)}`;
  return (
    <>
      <div className="sheet-backdrop" ref={back} />
      <div className="sheet" ref={sheet}>
        <div className="who">
          <div className="meta"><b>配对请求</b><span className="mono">{req.platform === "android" ? "ANDROID" : "WINDOWS"}</span></div>
          <h3 style={{ marginTop: 10 }}>“{req.name}”请求加入</h3>
          <p>请确认对方屏幕上显示的确认码与右侧完全一致。不一致时请拒绝——这可能意味着有人在冒充。</p>
        </div>
        <div className="code">{sas}</div>
        <div className="btns">
          <button className="btn-primary" disabled={busy} onClick={() => close(true)}>确认一致，同意加入</button>
          <button className="btn-ghost" disabled={busy} onClick={() => close(false)}>拒绝</button>
        </div>
      </div>
    </>
  );
}
