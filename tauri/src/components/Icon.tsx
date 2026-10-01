import type { CSSProperties } from "react";

// 图标 path 内容来自评审版交互稿的内联 SVG，stroke 风格统一（currentColor）。
const PATHS: Record<string, string> = {
  overview:
    '<path d="M12 3a9 9 0 109 9h-9V3z" stroke-linejoin="round"/><path d="M15 3.5A6.5 6.5 0 0120.5 9H15V3.5z" stroke-linejoin="round"/>',
  analysis:
    '<path d="M21 12a9 9 0 11-9-9" stroke-linecap="round"/><path d="M21 12h-9V3a9 9 0 019 9z" stroke-linejoin="round"/>',
  clean:
    '<path d="M12 3l9 6-9 6-9-6 9-6z" stroke-linejoin="round"/><path d="M5 14l7 4.5L19 14" stroke-linecap="round" stroke-linejoin="round"/>',
  dup: '<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M4 16V6a2 2 0 012-2h10"/>',
  uninstall:
    '<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>',
  startup:
    '<path d="M18.4 5.6A8 8 0 106 18.4M22 12h-2M12 2v2M4.9 4.9l1.4 1.4" stroke-linecap="round"/><path d="M12 12l4-4" stroke-linecap="round"/>',
  optimize:
    '<path d="M14.7 6.3a4 4 0 00-5.4 5.4L3 18v3h3l6.3-6.3a4 4 0 005.4-5.4L15 11l-2-2 1.7-2.7z" stroke-linejoin="round"/>',
  settings:
    '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 00.3 1.9l.1.1a2 2 0 11-2.8 2.8l-.1-.1a1.7 1.7 0 00-1.9-.3 1.7 1.7 0 00-1 1.5V21a2 2 0 11-4 0v-.1a1.7 1.7 0 00-1-1.6 1.7 1.7 0 00-1.9.3l-.1.1a2 2 0 11-2.8-2.8l.1.1a1.7 1.7 0 00.3-1.9 1.7 1.7 0 00-1.5-1H3a2 2 0 110-4h.1a1.7 1.7 0 001.6-1 1.7 1.7 0 00-.3-1.9l-.1-.1a2 2 0 112.8-2.8l.1.1a1.7 1.7 0 001.9.3 1.7 1.7 0 001-1.5V3a2 2 0 114 0v.1a1.7 1.7 0 001 1.5 1.7 1.7 0 001.9-.3l.1-.1a2 2 0 112.8 2.8l-.1.1a1.7 1.7 0 00-.3 1.9 1.7 1.7 0 001.5 1H21a2 2 0 110 4h-.1a1.7 1.7 0 00-1.5 1z" stroke-linejoin="round"/>',
  check:
    '<path d="M5 13l4 4L19 7" stroke-linecap="round" stroke-linejoin="round"/>',
  trash:
    '<path d="M3 6h18M8 6V4a1 1 0 011-1h6a1 1 0 011 1v2m3 0l-1 14a2 2 0 01-2 2H8a2 2 0 01-2-2L5 6" stroke-linecap="round" stroke-linejoin="round"/>',
  shield:
    '<path d="M12 2l8 4v6c0 5-3.5 8.5-8 10-4.5-1.5-8-5-8-10V6l8-4z" stroke-linejoin="round"/><path d="M9 12l2 2 4-4" stroke-linecap="round" stroke-linejoin="round"/>',
  spark:
    '<path d="M12 2l1.8 6.2L20 10l-6.2 1.8L12 18l-1.8-6.2L4 10l6.2-1.8L12 2z" stroke-linejoin="round"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" stroke-linecap="round"/>',
  moon: '<path d="M21 12.8A9 9 0 1111.2 3 7 7 0 0021 12.8z" stroke-linejoin="round"/>',
  lock: '<rect x="4" y="11" width="16" height="9" rx="2"/><path d="M8 11V7a4 4 0 018 0v4" stroke-linecap="round"/>',
  folder:
    '<path d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2V7z" stroke-linejoin="round"/>',
  clock:
    '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2" stroke-linecap="round" stroke-linejoin="round"/>',
  chev: '<path d="M9 6l6 6-6 6" stroke-linecap="round" stroke-linejoin="round"/>',
  ext: '<path d="M4 4h16v16H4z"/><path d="M4 9h16M9 4v16" stroke-linecap="round"/>',
  db: '<ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v14c0 1.7 3.6 3 8 3s8-1.3 8-3V5" stroke-linecap="round"/><path d="M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3" stroke-linecap="round"/>',
  cpu: '<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v2M15 2v2M9 20v2M15 20v2M2 9h2M2 15h2M20 9h2M20 15h2M9 9h6v6H9z" stroke-linecap="round"/>',
  model:
    '<rect x="4" y="4" width="7" height="7" rx="1.5"/><rect x="13" y="4" width="7" height="7" rx="1.5"/><rect x="4" y="13" width="7" height="7" rx="1.5"/><rect x="13" y="13" width="7" height="7" rx="1.5"/>',
  container:
    '<path d="M4 8l8-4 8 4v8l-8 4-8-4V8z" stroke-linejoin="round"/><path d="M4 8l8 4 8-4M12 12v8" stroke-linecap="round"/>',
  file:
    '<path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8l-6-6z" stroke-linejoin="round"/><path d="M14 2v6h6" stroke-linejoin="round"/>',
  doc: '<path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8l-6-6z"/><path d="M14 2v6h6M9 13h6M9 17h4" stroke-linecap="round"/>',
  search:
    '<circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3" stroke-linecap="round"/>',
  zap: '<path d="M13 2L4 14h6l-1 8 9-12h-6l1-8z" stroke-linejoin="round"/>',
  wifi: '<path d="M2 9a15 15 0 0120 0M5.5 12.5a10 10 0 0113 0M9 16a5 5 0 016 0" stroke-linecap="round"/><circle cx="12" cy="19" r="1"/>',
  refresh:
    '<path d="M20 12a8 8 0 11-2.3-5.6M20 3v5h-5" stroke-linecap="round" stroke-linejoin="round"/>',
  broom:
    '<path d="M14 4l6 6-2 2-6-6 2-2z" stroke-linejoin="round"/><path d="M4 20l7-7M13 13l-2 2" stroke-linecap="round"/>',
  warning:
    '<path d="M12 9v4M12 17h.01"/><path d="M10.3 3.9L1.8 18a2 2 0 001.7 3h17a2 2 0 001.7-3L13.7 3.9a2 2 0 00-3.4 0z" stroke-linejoin="round"/>',
};

export type IconName = keyof typeof PATHS;

const STROKE: Partial<Record<IconName, number>> = {
  check: 2.5,
};

export function Icon({
  name,
  size = 20,
  style,
  className,
}: {
  name: IconName | string;
  size?: number;
  style?: CSSProperties;
  className?: string;
}) {
  const body = PATHS[name] ?? PATHS.file;
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth={STROKE[name as IconName] ?? 1.8}
      style={style}
      className={className}
      dangerouslySetInnerHTML={{ __html: body }}
    />
  );
}
