export type Platform = "windows" | "android" | "other";
export type Kind = "text" | "otp" | "link" | "code" | "image" | "other";

export const VIEW: "main" | "send" =
  (window as unknown as { __CC_VIEW?: "send" }).__CC_VIEW ?? (new URLSearchParams(location.search).get("view") === "send" ? "send" : "main");

export interface Device {
  id: string;
  short: string;
  name: string;
  platform: Platform;
  me: boolean;
  relay: boolean;
  lan: boolean;

  screenshots: boolean;
}

export interface Clip {
  id: string;
  device: string;
  platform: Platform;
  mine: boolean;
  time: number;
  kind: Kind;
  text: string | null;
  chars: number;
  hasImage: boolean;

  pending: boolean;
  size: number;
}

export interface Settings {
  deviceName: string;
  syncImages: boolean;
  lanEnabled: boolean;
  mailbox: boolean;
  relayEnabled: boolean;
  excludedApps: string[];
  receiveDir: string;
  askReceiveDir: boolean;
  copyReceived: boolean;
  copyReceivedImage: boolean;
  openSharedLinks: boolean;
  sendHotkey: string;
  relayConfirmMb: number;
  screenshots: boolean;
  screenshotTools: string[];
  historyDays: number;
}

export type TransferState = "preparing" | "waiting" | "asking" | "active" | "done" | "failed" | "cancelled";

export interface Transfer {
  id: string;
  deviceId: string;
  device: string;
  platform: Platform;
  outgoing: boolean;
  kind: "files" | "screenshot";
  files: { name: string; size: number }[];
  count: number;
  text: string | null;
  total: number;
  done: number;
  rate: number;
  state: TransferState;
  error: string | null;
  lan: boolean;
  dir: string | null;
  saved: string[];
  time: number;
  finished: number | null;
}

export interface Overview {
  initialized: boolean;
  me: Device | null;

  relay: { addr: string; configured: boolean; connected: boolean; enabled: boolean } | null;
  devices: Device[];
  history: Clip[];
  settings: Settings;
  paused: boolean;
  lanPort: number | null;
  transfers: Transfer[];
  receiveDir: string;
}

export interface RelaySetup {
  addr: string;

  key: string;

  compose: string;
}

export interface Firewall {
  state: "open" | "noRule" | "blocked" | "blockAll";
  public: boolean;
}

export interface PairRequest {
  name: string;
  platform: Platform;
  sas: string;
}

export interface SharedText {
  device: string;
  text: string;
  link: string | null;
  opened: boolean;
}

export type Events = {
  "cc:changed": string;
  "cc:arrived": string;
  "cc:pair-request": PairRequest;
  "cc:paired": string;
  "cc:revoked": null;
  "cc:join-sas": string;
  "cc:transfer": Transfer;
  "cc:shared": SharedText;
  "cc:send-open": null;
  "cc:send-paths": null;
};

export interface Api {
  overview(): Promise<Overview>;
  createGroup(a: { name: string }): Promise<void>;
  joinGroup(a: { uri: string; name: string }): Promise<void>;

  createInvite(): Promise<{ uri: string; matrix: string[] }>;
  respondPairing(accept: boolean): Promise<void>;
  removeDevice(id: string): Promise<void>;

  leaveGroup(): Promise<{ peers: number; relay: boolean }>;
  copyItem(id: string): Promise<void>;
  forgetItem(id: string): Promise<void>;

  clearHistory(everywhere?: boolean): Promise<number>;
  itemImage(id: string): Promise<string | null>;
  setRelayEnabled(enabled: boolean): Promise<void>;

  setupRelay(addr: string): Promise<RelaySetup>;
  relaySetup(): Promise<RelaySetup | null>;
  setRelayAddr(addr: string): Promise<void>;
  removeRelay(): Promise<void>;
  setPaused(paused: boolean): Promise<void>;
  saveSettings(settings: Settings): Promise<void>;
  networkChanged(): Promise<void>;
  firewall(): Promise<Firewall>;
  allowFirewall(): Promise<Firewall>;
  summarize(paths: string[]): Promise<{ count: number; total: number }>;
  sendPaths(device: string, paths: string[]): Promise<string>;
  cancelTransfer(id: string): Promise<void>;
  clearTransfers(): Promise<void>;
  revealTransfer(id: string): Promise<void>;
  openReceiveDir(): Promise<void>;
  pickFiles(): Promise<string[]>;
  pickFolder(title: string): Promise<string | null>;
  showSend(): Promise<void>;

  takeSendPaths(): Promise<string[]>;
  showMain(): Promise<void>;

  onDrop(cb: (e: { over: boolean; paths?: string[] }) => void): Promise<() => void>;
  autostart: { get(): Promise<boolean>; set(on: boolean): Promise<void> };
  win: { minimize(): void; toggleMaximize(): void; hide(): void; startDrag(): void };
  on<E extends keyof Events>(event: E, cb: (payload: Events[E]) => void): Promise<() => void>;
}

export const isTauri = "__TAURI_INTERNALS__" in window;

async function tauriApi(): Promise<Api> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  const autostart = await import("@tauri-apps/plugin-autostart");
  const w = getCurrentWindow();
  return {
    overview: () => invoke("overview"),
    createGroup: (a) => invoke("create_group", a),
    joinGroup: (a) => invoke("join_group", a),
    createInvite: () => invoke("create_invite"),
    respondPairing: (accept) => invoke("respond_pairing", { accept }),
    removeDevice: (id) => invoke("remove_device", { id }),
    leaveGroup: () => invoke("leave_group"),
    copyItem: (id) => invoke("copy_item", { id }),
    forgetItem: (id) => invoke("forget_item", { id }),
    clearHistory: (everywhere) => invoke("clear_history", { everywhere: !!everywhere }),
    itemImage: (id) => invoke("item_image", { id }),
    setRelayEnabled: (enabled) => invoke("set_relay_enabled", { enabled }),
    setupRelay: (addr) => invoke("setup_relay", { addr }),
    relaySetup: () => invoke("relay_setup"),
    setRelayAddr: (addr) => invoke("set_relay_addr", { addr }),
    removeRelay: () => invoke("remove_relay"),
    setPaused: (paused) => invoke("set_paused", { paused }),
    saveSettings: (settings) => invoke("save_settings", { settings }),
    networkChanged: () => invoke("network_changed"),
    firewall: () => invoke("firewall_status"),
    allowFirewall: () => invoke("firewall_allow"),
    summarize: (paths) => invoke("summarize", { paths }),
    sendPaths: (device, paths) => invoke("send_paths", { device, paths }),
    cancelTransfer: (id) => invoke("cancel_transfer", { id }),
    clearTransfers: () => invoke("clear_transfers"),
    revealTransfer: (id) => invoke("reveal_transfer", { id }),
    openReceiveDir: () => invoke("open_receive_dir"),
    pickFiles: () => invoke("pick_files"),
    pickFolder: (title) => invoke("pick_folder", { title }),
    showSend: () => invoke("show_send"),
    takeSendPaths: () => invoke("take_send_paths"),
    showMain: () => invoke("show_main"),
    onDrop: (cb) =>
      getCurrentWebview().onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === "drop") cb({ over: false, paths: p.paths });
        else cb({ over: p.type !== "leave" });
      }),
    autostart: { get: () => autostart.isEnabled(), set: (on) => (on ? autostart.enable() : autostart.disable()) },
    win: {
      minimize: () => void w.minimize(),
      toggleMaximize: () => void w.toggleMaximize(),
      hide: () => void w.hide(),
      startDrag: () => void w.startDragging(),
    },
    on: async (event, cb) => listen(event, (e) => cb(e.payload as never)),
  };
}

export const api: Promise<Api> = isTauri ? tauriApi() : import("./mock").then((m) => m.mockApi());
