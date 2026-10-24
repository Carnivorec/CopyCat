const pad = (n: number) => String(n).padStart(2, "0");

export function fmtTime(ms: number): string {
  const d = new Date(ms);
  const now = new Date();
  if (d.toDateString() === now.toDateString()) return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(2)} GB`;
}

export const fmtRate = (n: number) => `${fmtSize(n)}/s`;

export function fmtEta(sec: number): string {
  if (!isFinite(sec) || sec <= 0) return "";
  if (sec < 60) return `约 ${Math.ceil(sec)} 秒`;
  if (sec < 3600) return `约 ${Math.ceil(sec / 60)} 分钟`;
  return `约 ${(sec / 3600).toFixed(1)} 小时`;
}

export const baseName = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;

const CJK = /[㐀-鿿]/;
export const isCJK = (s: string) => CJK.test(s);

export const KIND_LABEL: Record<string, string> = {
  text: "文本",
  otp: "验证码",
  link: "链接",
  code: "代码",
  image: "图片",
  other: "其他",
};

export function otpDigits(text: string): string {
  const m = text.match(/\d[\d\s-]{2,10}\d/);
  return (m ? m[0] : text).replace(/[\s-]/g, "");
}
