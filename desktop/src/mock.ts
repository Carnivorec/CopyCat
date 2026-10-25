import type { Api, Clip, Device, Events, Firewall, Kind, Overview, Settings, Transfer } from "./api";

const q = new URLSearchParams(location.search);
let fw = (q.get("fw") ?? "open") as Firewall["state"];
const listeners = new Map<string, Set<(p: unknown) => void>>();
function emit<E extends keyof Events>(e: E, p: Events[E]) {
  listeners.get(e)?.forEach((cb) => cb(p));
}

const devices: Device[] = [
  { id: "aaaa", short: "c4y7imnr", name: "台式机", platform: "windows", me: true, relay: false, lan: false, screenshots: false },
  { id: "bbbb", short: "q2kd8fha", name: "示例手机", platform: "android", me: false, relay: true, lan: true, screenshots: true },
  { id: "cccc", short: "m3n0pwla", name: "笔记本", platform: "windows", me: false, relay: true, lan: false, screenshots: false },
];

const POOL: [string, Kind, string | null][] = [
  ["bbbb", "otp", "482913"],
  ["aaaa", "text", "会议纪要：周五 14:00，三楼会议室，记得带上打印好的方案"],
  ["cccc", "link", "github.com/Carnivorec/CopyCat/releases"],
  ["aaaa", "code", "cargo test --workspace"],
  ["bbbb", "image", null],
  ["cccc", "text", "Zürich · Basel · Bern"],
  ["bbbb", "text", "今晚七点，老地方见"],
  ["aaaa", "link", "https://docs.rs/quinn/latest/quinn/"],
  ["cccc", "otp", "您的验证码是 730165，五分钟内有效"],
  ["aaaa", "text", "Grid systems in graphic design — Josef Müller-Brockmann"],
  ["bbbb", "text", "快递柜取件码 8-2-0-4，东门第三排"],
  ["cccc", "code", "git push origin main"],
];

let seq = 0;
let pi = 0;
function make(t: number): Clip {
  const [dev, kind, text] = POOL[pi++ % POOL.length];
  const d = devices.find((x) => x.id === dev)!;
  return {
    id: String(++seq).padStart(32, "0"),
    device: d.name,
    platform: d.platform,
    mine: d.me,
    time: t,
    kind,
    text,
    chars: text?.length ?? 0,
    hasImage: kind === "image",
    pending: false,
    size: kind === "image" ? 4_240_000 : (text?.length ?? 0) * 3,
  };
}

const now = Date.now();
let history: Clip[] = Array.from({ length: 9 }, (_, i) => make(now - (i + 1) * 7 * 60_000));

if (q.has("bigimage")) {
  history = [{ ...make(now - 60_000), kind: "image", text: null, chars: 0, hasImage: true, pending: true, size: 6_800_000 }, ...history];
}
let initialized = !q.has("empty");
let paused = false;
let relayEnabled = true;

let relayAddr = q.has("lanonly") ? "" : "relay.example.com:8443";
let relayKey = "";
const fakeKey = () =>
  "ccr1-" + Array.from({ length: 86 }, (_, i) => "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"[(i * 37 + Date.now()) % 64]).join("");
const fakeSetup = () => ({
  addr: relayAddr,
  key: relayKey,
  compose: `services:\n  relay:\n    environment:\n      COPYCAT_RELAY_KEY: "${relayKey}"\n`,
});
let settings: Settings = {
  deviceName: "台式机",
  syncImages: true,
  lanEnabled: true,
  mailbox: true,
  relayEnabled: true,
  excludedApps: ["KeePass.exe", "1Password.exe"],
  receiveDir: "",
  askReceiveDir: false,
  copyReceived: false,
  copyReceivedImage: true,
  openSharedLinks: true,
  sendHotkey: "Ctrl+Shift+F",
  relayConfirmMb: 50,
  screenshots: true,
  screenshotTools: ["SnippingTool.exe", "ScreenClippingHost.exe", "Snipaste.exe", "PixPin.exe"],
  historyDays: 1,
};

const MB = 1024 * 1024;
let tseq = 0;
function transfer(p: Partial<Transfer>): Transfer {
  return {
    id: String(++tseq).padStart(32, "0"),
    deviceId: "bbbb",
    device: "示例手机",
    platform: "android",
    outgoing: false,
    kind: "files",
    files: [{ name: "IMG_20261005_141233.jpg", size: 4.2 * MB }],
    count: 1,
    text: null,
    total: 4.2 * MB,
    done: 4.2 * MB,
    rate: 0,
    state: "done",
    error: null,
    lan: true,
    dir: "C:\\Users\\me\\Downloads\\CopyCat",
    saved: ["C:\\Users\\me\\Downloads\\CopyCat\\IMG_20261005_141233.jpg"],
    time: Date.now() - 3 * 60_000,
    finished: Date.now() - 3 * 60_000,
    ...p,
  };
}

let transfers: Transfer[] = q.has("notransfers")
  ? []
  : [
      transfer({
        outgoing: true,
        device: "笔记本",
        deviceId: "cccc",
        platform: "windows",
        files: [{ name: "季度报告/正文.docx", size: 2 * MB }, { name: "季度报告/附图/图1.png", size: 1 * MB }],
        count: 38,
        total: 812 * MB,
        done: 301 * MB,
        rate: 11.4 * MB,
        state: "active",
        lan: false,
        dir: null,
        saved: [],
        time: Date.now() - 40_000,
        finished: null,
      }),
      transfer({ text: "看看这个 https://example.com/xYz 很有意思", files: [], count: 0, total: 40, done: 40 }),
      transfer({
        outgoing: true,
        kind: "screenshot",
        files: [{ name: "截图_20261005_141502.png", size: 2.6 * MB }],
        total: 2.6 * MB,
        done: 2.6 * MB,
        saved: [],
        dir: null,
        time: Date.now() - 60_000,
        finished: Date.now() - 60_000,
      }),
      transfer({}),
      transfer({
        outgoing: true,
        files: [{ name: "安装包.exe", size: 96 * MB }],
        total: 96 * MB,
        done: 12 * MB,
        state: "failed",
        error: "对方取消了接收",
        saved: [],
        dir: null,
        time: Date.now() - 50 * 60_000,
        finished: Date.now() - 49 * 60_000,
      }),
    ];

function tickTransfers() {
  for (const t of transfers) {
    if (t.state !== "active") continue;
    t.done = Math.min(t.total, t.done + t.rate * 0.25);
    if (t.done >= t.total) {
      t.state = "done";
      t.finished = Date.now();
      t.rate = 0;
    }
    emit("cc:transfer", { ...t });
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function overview(): Overview {
  return {
    initialized,
    me: initialized ? devices[0] : null,
    relay: initialized
      ? { addr: relayAddr, configured: !!relayAddr, connected: !!relayAddr && relayEnabled, enabled: relayEnabled }
      : null,
    devices: initialized ? devices : [],
    history: initialized ? history : [],
    settings,
    paused,
    lanPort: 47821,
    transfers: initialized ? transfers : [],
    receiveDir: settings.receiveDir || "C:\\Users\\me\\Downloads\\CopyCat",
  };
}

function arrive() {
  history = [make(Date.now()), ...history].slice(0, 30);
  emit("cc:arrived", history[0].id);
  emit("cc:changed", "history");
}

const IMG = `data:image/svg+xml;utf8,${encodeURIComponent(
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 440 300"><rect width="440" height="300" fill="#ece9e1"/><circle cx="170" cy="130" r="96" fill="#e1251b"/><rect x="40" y="200" width="300" height="26" fill="#0a0a0a" transform="rotate(-14 190 213)"/><rect x="360" y="0" width="14" height="300" fill="#0a0a0a"/></svg>`,
)}`;

function fakeQr(): string[] {
  let r = 7;
  const rows: string[] = [];
  for (let y = 0; y < 29; y++) {
    let row = "";
    for (let x = 0; x < 29; x++) {
      r = (r * 1103515245 + 12345) & 0x7fffffff;
      const finder = (x < 7 && y < 7) || (x > 21 && y < 7) || (x < 7 && y > 21);
      const ring = finder && (x % 22 === 0 || y % 22 === 0 || x % 22 === 6 || y % 22 === 6 || (x % 22 > 1 && x % 22 < 5 && y % 22 > 1 && y % 22 < 5));
      row += (finder ? ring : r % 3 === 0) ? "1" : "0";
    }
    rows.push(row);
  }
  return rows;
}

export function mockApi(): Api {
  if (!q.has("still")) setInterval(() => initialized && !paused && arrive(), 6500);
  if (!q.has("still")) setInterval(tickTransfers, 250);

  if (q.has("revoked")) setTimeout(() => emit("cc:revoked", null), 300);
  return {
    overview: async () => overview(),
    createGroup: async (a) => {
      await sleep(500);
      settings = { ...settings, deviceName: a.name };
      relayAddr = "";
      initialized = true;
      emit("cc:changed", "node");
    },
    joinGroup: async (a) => {
      if (!a.uri.startsWith("copycat://pair?")) throw "这不是有效的 CopyCat 配对码";
      await sleep(700);
      emit("cc:join-sas", "482 913".replace(" ", ""));
      await sleep(2600);
      initialized = true;
      emit("cc:changed", "node");
    },
    createInvite: async () => {
      await sleep(300);
      setTimeout(() => emit("cc:pair-request", { name: "示例手机 Pro", platform: "android", sas: "305718" }), 5000);
      return { uri: "copycat://pair?v=1&id=c4y7imnr…", matrix: fakeQr() };
    },
    respondPairing: async () => {
      await sleep(300);
      emit("cc:changed", "status");
    },
    removeDevice: async (id) => {
      devices.splice(devices.findIndex((d) => d.id === id), 1);
      emit("cc:changed", "status");
    },
    leaveGroup: async () => {
      await sleep(600);
      initialized = false;
      history = [];
      emit("cc:changed", "node");
      return { peers: 1, relay: true };
    },
    copyItem: async (id) => {
      const it = history.find((h) => h.id === id);
      if (it) history = [{ ...it, id: String(++seq).padStart(32, "0"), time: Date.now(), mine: true, device: "台式机" }, ...history.filter((h) => h.id !== id)];
      emit("cc:changed", "history");
    },
    forgetItem: async (id) => {
      history = history.filter((h) => h.id !== id);
      emit("cc:changed", "history");
    },
    clearHistory: async (everywhere) => {
      history = [];
      emit("cc:changed", "history");
      return everywhere ? devices.filter((d) => !d.me && (d.lan || d.relay)).length : 0;
    },
    itemImage: async () => IMG,
    setRelayEnabled: async (b) => {
      relayEnabled = b;
      settings = { ...settings, relayEnabled: b };
      emit("cc:changed", "status");
    },
    setupRelay: async (addr) => {
      await sleep(300);
      relayAddr = addr;
      relayKey = fakeKey();
      emit("cc:changed", "status");
      return fakeSetup();
    },
    relaySetup: async () => (relayAddr && relayKey ? fakeSetup() : null),
    setRelayAddr: async (addr) => {
      relayAddr = addr;
      emit("cc:changed", "status");
    },
    removeRelay: async () => {
      relayAddr = "";
      relayKey = "";
      emit("cc:changed", "status");
    },
    setPaused: async (b) => {
      paused = b;
      emit("cc:changed", "status");
    },
    saveSettings: async (s) => {
      settings = s;
      emit("cc:changed", "settings");
    },
    networkChanged: async () => {},
    firewall: async () => ({ state: fw, public: fw !== "open" }),
    allowFirewall: async () => {
      await sleep(900);
      if (fw !== "blockAll") fw = "open";
      return { state: fw, public: true };
    },
    summarize: async (paths) => {
      await sleep(120);
      return { count: paths.length * 3 - 1, total: paths.length * 71.3 * MB };
    },
    sendPaths: async (device, paths) => {
      const d = devices.find((x) => x.id === device)!;
      const t = transfer({
        outgoing: true,
        deviceId: d.id,
        device: d.name,
        platform: d.platform,
        files: paths.map((p) => ({ name: p.split("\\").pop()!, size: 71.3 * MB })),
        count: paths.length,
        total: paths.length * 71.3 * MB,
        done: 0,
        rate: d.lan ? 48 * MB : 9 * MB,
        state: "active",
        lan: d.lan,
        saved: [],
        dir: null,
        time: Date.now(),
        finished: null,
      });
      transfers = [t, ...transfers];
      emit("cc:transfer", t);
      return t.id;
    },
    cancelTransfer: async (id) => {
      const t = transfers.find((x) => x.id === id);
      if (t) {
        t.state = "cancelled";
        t.finished = Date.now();
        emit("cc:transfer", { ...t });
      }
    },
    clearTransfers: async () => {
      transfers = transfers.filter((t) => ["active", "waiting", "asking", "preparing"].includes(t.state));
      emit("cc:changed", "transfers");
    },
    revealTransfer: async () => {},
    openReceiveDir: async () => {},
    pickFiles: async () => ["C:\\Users\\me\\Pictures\\旅行照片.zip"],
    pickFolder: async () => "D:\\收件箱",
    showSend: async () => {},
    takeSendPaths: async () => (q.has("selected") ? ["C:\\Users\\me\\Desktop\\报价单.pdf", "C:\\Users\\me\\Pictures\\旅行照片"] : []),
    showMain: async () => {},
    onDrop: async (cb) => {
      const many = Array.from({ length: 14 }, (_, i) => `C:\\Users\\me\\Desktop\\素材\\现场照片_${String(i + 1).padStart(2, "0")}${i === 3 ? "_很长很长的文件名用来检查省略号是否正常显示_最终版" : ""}.jpg`);
      const two = ["C:\\Users\\me\\Desktop\\设计稿 v3", "C:\\Users\\me\\Desktop\\会议录音.m4a"];
      if (q.has("drop")) setTimeout(() => cb({ over: false, paths: q.get("drop") === "many" ? many : two }), 300);
      return () => {};
    },
    autostart: { get: async () => true, set: async () => {} },
    win: { minimize() {}, toggleMaximize() {}, hide() {}, startDrag() {} },
    on: async (event, cb) => {
      const set = listeners.get(event) ?? new Set();
      set.add(cb as (p: unknown) => void);
      listeners.set(event, set);
      return () => set.delete(cb as (p: unknown) => void);
    },
  };
}
