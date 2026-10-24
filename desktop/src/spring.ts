const REDUCE = matchMedia("(prefers-reduced-motion: reduce)").matches;

export type Preset = { k: number; c: number };
export const P = {
  snappy: { k: 520, c: 34 },
  bouncy: { k: 380, c: 17 },
  gentle: { k: 170, c: 22 },
  wobbly: { k: 260, c: 11 },
  rule: { k: 160, c: 20 },
} satisfies Record<string, Preset>;

const active = new Set<Spring>();
let raf = 0;
let last = 0;

export class Spring {
  x: number;
  t: number;
  v = 0;
  private k = 300;
  private c = 26;
  private res: (() => void) | null = null;

  constructor(x: number, private on?: (v: number) => void, private eps = 0.02) {
    this.x = x;
    this.t = x;
  }

  to(t: number, preset: Preset = P.snappy, v?: number): Promise<void> {
    this.t = t;
    this.k = preset.k;
    this.c = REDUCE ? 2 * Math.sqrt(preset.k) * 1.05 : preset.c;
    if (v !== undefined) this.v = v;
    active.add(this);
    kick();
    return new Promise((r) => {
      this.res?.();
      this.res = r;
    });
  }

  set(x: number) {
    this.x = this.t = x;
    this.v = 0;
    active.delete(this);
    this.on?.(x);
  }

  step(dt: number) {
    const a = -this.k * (this.x - this.t) - this.c * this.v;
    this.v += a * dt;
    this.x += this.v * dt;
    if (Math.abs(this.v) < this.eps * 10 && Math.abs(this.x - this.t) < this.eps) {
      this.x = this.t;
      this.v = 0;
      active.delete(this);
      this.on?.(this.x);
      const r = this.res;
      this.res = null;
      r?.();
      return;
    }
    this.on?.(this.x);
  }
}

function kick() {
  if (!raf) {
    last = performance.now();
    raf = requestAnimationFrame(loop);
  }
}

function loop(now: number) {
  const dt = Math.min((now - last) / 1000, 1 / 30);
  last = now;
  for (const s of [...active]) for (let i = 0; i < 4 && active.has(s); i++) s.step(dt / 4);
  raf = active.size ? requestAnimationFrame(loop) : 0;
}

export type Motion = {
  x: number; y: number; s: number; sx: number; r: number; o: number;
  X: Spring; Y: Spring; S: Spring; SX: Spring; R: Spring; O: Spring;
  apply(): void;
};

const motions = new WeakMap<HTMLElement | SVGElement, Motion>();

export function mo(el: HTMLElement | SVGElement): Motion {
  const existing = motions.get(el);
  if (existing) return existing;
  const m = { x: 0, y: 0, s: 1, sx: 1, r: 0, o: 1 } as Motion;
  m.apply = () => {
    el.style.transform = `translate3d(${m.x}px,${m.y}px,0) scale(${m.s}) scaleX(${m.sx}) rotate(${m.r}deg)`;
    el.style.opacity = String(m.o);
  };
  const mk = (k: "x" | "y" | "s" | "sx" | "r" | "o", eps?: number) =>
    new Spring(m[k], (v) => {
      m[k] = v;
      m.apply();
    }, eps);
  m.X = mk("x");
  m.Y = mk("y");
  m.S = mk("s", 0.0005);
  m.SX = mk("sx", 0.0005);
  m.R = mk("r", 0.01);
  m.O = mk("o", 0.002);
  motions.set(el, m);
  return m;
}
