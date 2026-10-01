/** 字节数人类可读格式（与交互稿 fmt 一致：GB/MB/KB/B，一位小数） */
export function fmt(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "0 B";
  if (n >= 1e9) return (n / 1e9).toFixed(1) + " GB";
  if (n >= 1e6) return (n / 1e6).toFixed(1) + " MB";
  if (n >= 1e3) return (n / 1e3).toFixed(0) + " KB";
  return Math.round(n) + " B";
}

/** 取路径最后两级做短名 */
export function baseName(p: string): string {
  const parts = p.replace(/\/+$/, "").split("/").filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

/** 把 /Users/<name> 缩成 ~，让长路径在列表里可读 */
export function shortPath(p: string): string {
  return p.replace(/^\/Users\/[^/]+/, "~");
}

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
