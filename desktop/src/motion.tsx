import { useEffect, useLayoutEffect, useRef, type RefObject } from "react";
import { mo, P, Spring } from "./spring";

export function useFlip(box: RefObject<HTMLElement | null>, enter: "down" | "right" = "down") {
  const prev = useRef<Map<string, number> | null>(null);
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const next = new Map<string, number>();
    const first = prev.current === null;
    for (const child of Array.from(el.children) as HTMLElement[]) {
      const id = child.dataset.id;
      if (!id || child.dataset.leaving) continue;
      const m = mo(child);
      const natural = child.getBoundingClientRect().top - m.y;
      next.set(id, natural);
      const before = prev.current?.get(id);
      if (before !== undefined) {
        const dy = before - natural;
        if (Math.abs(dy) > 0.5) {
          m.Y.x += dy;
          m.y = m.Y.x;
          m.apply();
          m.Y.to(0, P.snappy);
        }
      } else if (!first) {
        if (enter === "down") {
          m.Y.x -= 50;
          m.y = m.Y.x;
          m.Y.to(0, P.bouncy);
        } else {
          m.X.set(120);
          m.X.to(0, P.bouncy);
        }
        m.O.set(0);
        m.O.to(1, P.gentle);
      }
    }
    prev.current = next;
  });
}

export function useSwipe(
  ref: RefObject<HTMLElement | null>,
  onRemove: () => void,
  onTap: () => void,
) {
  const cb = useRef({ onRemove, onTap });
  cb.current = { onRemove, onTap };
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    let sx = 0, sy = 0, dx = 0, lastX = 0, lastT = 0, vx = 0;
    let down = false, dragging = false;
    const m = mo(el);
    const bin = el.querySelector<HTMLElement>(".bin");
    const pd = (e: PointerEvent) => {
      if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
      down = true; dragging = false;
      sx = lastX = e.clientX; sy = e.clientY; lastT = e.timeStamp; dx = 0; vx = 0;
      el.setPointerCapture(e.pointerId);
      m.S.to(0.985, P.snappy);
    };
    const pm = (e: PointerEvent) => {
      if (!down) return;
      const ddx = e.clientX - sx;
      if (!dragging && Math.abs(ddx) > 6 && Math.abs(ddx) > Math.abs(e.clientY - sy)) dragging = true;
      if (!dragging) return;
      const dt = Math.max(e.timeStamp - lastT, 1);
      vx = 0.8 * (((e.clientX - lastX) / dt) * 1000) + 0.2 * vx;
      lastX = e.clientX; lastT = e.timeStamp;
      dx = ddx > 0 ? ddx * 0.3 : ddx;
      m.X.set(dx);
      if (bin) bin.style.opacity = String(Math.min(1, -dx / 120));
    };
    const pu = () => {
      if (!down) return;
      down = false;
      m.S.to(1, P.bouncy);
      if (!dragging) {
        cb.current.onTap();
        return;
      }
      if (dx < -110 || vx < -700) {
        el.dataset.leaving = "1";
        m.X.to(-el.offsetWidth - 60, { k: 260, c: 30 }, Math.min(vx, -800)).then(() => cb.current.onRemove());
      } else {
        m.X.to(0, P.bouncy, vx);
        if (bin) bin.style.opacity = "0";
      }
    };
    el.addEventListener("pointerdown", pd);
    el.addEventListener("pointermove", pm);
    el.addEventListener("pointerup", pu);
    el.addEventListener("pointercancel", pu);
    return () => {
      el.removeEventListener("pointerdown", pd);
      el.removeEventListener("pointermove", pm);
      el.removeEventListener("pointerup", pu);
      el.removeEventListener("pointercancel", pu);
    };
  }, [ref]);
}

export function Odometer({ value, className }: { value: number; className?: string }) {
  const digits = String(Math.max(0, value)).split("");
  return (
    <div className={className} aria-label={String(value)} data-len={digits.length}>
      {digits.map((d, i) => (
        <Digit key={digits.length - i} d={+d} />
      ))}
    </div>
  );
}

function Digit({ d }: { d: number }) {
  const col = useRef<HTMLSpanElement>(null);
  const sp = useRef<Spring | null>(null);
  useLayoutEffect(() => {
    if (!sp.current) {
      sp.current = new Spring(d, (v) => {
        if (col.current) col.current.style.transform = `translateY(${-v * 1.2}em)`;
      }, 0.002);
      sp.current.set(d);
    } else sp.current.to(d, P.bouncy);
  }, [d]);
  return (
    <span className="odo-digit">
      <span ref={col} className="odo-col">
        {"0123456789".split("").map((n) => (
          <span key={n}>{n}</span>
        ))}
      </span>
    </span>
  );
}

export function useNudge(ref: RefObject<HTMLElement | null>, dx = 6) {
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const m = mo(el);
    const enter = () => m.X.to(dx, P.bouncy);
    const leave = () => { m.X.to(0, P.bouncy); m.S.to(1, P.bouncy); };
    const down = () => m.S.to(0.96, P.snappy);
    const up = () => m.S.to(1, P.wobbly);
    el.addEventListener("pointerenter", enter);
    el.addEventListener("pointerleave", leave);
    el.addEventListener("pointerdown", down);
    el.addEventListener("pointerup", up);
    return () => {
      el.removeEventListener("pointerenter", enter);
      el.removeEventListener("pointerleave", leave);
      el.removeEventListener("pointerdown", down);
      el.removeEventListener("pointerup", up);
    };
  }, [ref, dx]);
}
