import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ArrowRight, CircleAlert } from "lucide-react";
import { useApp } from "./App";
import { mo, P } from "./spring";

type Mode = "join" | "create";

export function Onboarding() {
  const { api, ov, refresh } = useApp();
  const [mode, setMode] = useState<Mode>("create");
  const [name, setName] = useState(ov.settings.deviceName);
  const [uri, setUri] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [sas, setSas] = useState("");

  const tabs = useRef<HTMLDivElement>(null);
  const ink = useRef<HTMLSpanElement>(null);
  const form = useRef<HTMLDivElement>(null);
  const word = useRef<HTMLDivElement>(null);
  const rule = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const w = mo(word.current!);
    w.Y.set(80);
    w.O.set(0);
    w.Y.to(0, P.bouncy);
    w.O.to(1, P.gentle);
    const r = mo(rule.current!);
    r.SX.set(0);
    setTimeout(() => r.SX.to(1, P.rule), 180);
  }, []);

  useLayoutEffect(() => {
    const b = tabs.current!.querySelector<HTMLButtonElement>(`[data-m="${mode}"]`)!;
    const m = mo(ink.current!);
    ink.current!.style.width = `${b.offsetWidth}px`;
    m.X.to(b.offsetLeft, P.bouncy);
    const f = mo(form.current!);
    f.X.set(mode === "join" ? -24 : 24);
    f.X.to(0, P.bouncy);
    setErr("");
  }, [mode]);

  useEffect(() => {
    const off = api.on("cc:join-sas", (code) => setSas(code));
    return () => void off.then((f) => f());
  }, [api]);

  const shake = () => {
    const f = mo(form.current!);
    f.X.set(-14);
    f.X.to(0, P.wobbly, 900);
  };

  const submit = async () => {
    if (!name.trim()) {
      setErr("请给这台设备起个名字");
      return shake();
    }
    setBusy(true);
    setErr("");
    setSas("");
    try {
      if (mode === "join") await api.joinGroup({ uri: uri.trim(), name: name.trim() });
      else await api.createGroup({ name: name.trim() });
      refresh();
    } catch (e) {
      setErr(String(e));
      setSas("");
      shake();
    } finally {
      setBusy(false);
    }
  };

  const ready = mode === "join" ? uri.trim().startsWith("copycat://") : true;

  return (
    <div className="onboard">
      <section className="brand">
        <div className="meta"><b>欢迎</b><span className="mono">01 / 设置</span></div>
        <div className="word" ref={word} style={{ marginTop: 18 }}>CopyCat</div>
        <div className="rule" ref={rule} />
        <p>
          在所有设备之间，
          <br />
          同步剪贴板。
        </p>
        <div className="foot">
          内容在设备之间端到端加密：局域网内直接连接，经中转服务器时它也只能看到密文。配对需要扫码，并在两台设备上核对确认码。
        </div>
      </section>
      <section className="form">
        <div className="tabs" ref={tabs}>
          <button data-m="create" className={mode === "create" ? "on" : ""} onClick={() => setMode("create")}>创建设备组</button>
          <button data-m="join" className={mode === "join" ? "on" : ""} onClick={() => setMode("join")}>加入设备组</button>
          <span className="ink" ref={ink} />
        </div>
        <div ref={form}>
          <label className="field">
            <span>本机名称</span>
            <input className="cjk" value={name} onChange={(e) => setName(e.target.value)} maxLength={32} spellCheck={false} />
          </label>
          {mode === "join" ? (
            <label className="field">
              <span>配对码<em>在已加入的设备上点“添加设备”获取</em></span>
              <textarea rows={3} value={uri} onChange={(e) => setUri(e.target.value)} placeholder="copycat://pair?…" spellCheck={false} />
            </label>
          ) : (
            <ol className="steps">
              <li><b>01</b>在本机创建设备组，不需要服务器。</li>
              <li><b>02</b>在“设备”页点“添加设备”，同一局域网内的手机或电脑扫码即可加入。</li>
              <li><b>03</b>需要跨网络同步时，再到“设置 → 中转服务器”生成部署密钥，填进 VPS 的 compose.yml。</li>
            </ol>
          )}
          <div className="go">
            <button className="btn-primary" disabled={busy || !ready} onClick={submit}>
              {busy ? (mode === "join" ? "正在连接…" : "正在创建…") : mode === "join" ? "加入" : "创建设备组"}
              {!busy && <ArrowRight />}
            </button>
          </div>
          {err && (
            <div className="err"><CircleAlert />{err}</div>
          )}
          {sas && busy && (
            <div className="sas">
              <div className="meta"><b>确认码</b><span className="mono">请在邀请方核对</span></div>
              <div className="code">{sas.slice(0, 3)} {sas.slice(3)}</div>
              <p>请确认邀请方屏幕上显示的确认码与这里一致，并在那边点同意。</p>
            </div>
          )}
        </div>
      </section>
    </div>
  );
}
