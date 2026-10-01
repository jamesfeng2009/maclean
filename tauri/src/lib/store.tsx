import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { ipc } from "./ipc";
import type { ScanItem, ScanProgress, ScanScope } from "./types";
import type { IconName } from "../components/Icon";

export type Theme = "light" | "dark";
export type Page =
  | "overview"
  | "analysis"
  | "clean"
  | "dup"
  | "uninstall"
  | "startup"
  | "optimize"
  | "settings";

export interface Toast {
  id: number;
  type: "success" | "info" | "warn";
  msg: string;
}

export interface ConfirmRequest {
  title: string;
  sub: string;
  warn?: string;
  items: string[];
  confirmText?: string;
  onConfirm: () => void | Promise<void>;
}

interface AppState {
  page: Page;
  setPage: (p: Page) => void;
  theme: Theme;
  toggleTheme: () => void;
  langEn: boolean;
  setLangEn: (en: boolean) => void;
  toasts: Toast[];
  toast: (type: Toast["type"], msg: string) => void;
  scanning: boolean;
  scanPct: number;
  scanLabel: string;
  startScan: (
    scope: ScanScope,
    onDone?: (items: ScanItem[]) => void
  ) => Promise<void>;
  confirm: (req: ConfirmRequest) => void;
  confirmReq: ConfirmRequest | null;
  closeConfirm: () => void;
}

const Ctx = createContext<AppState | null>(null);

export const PAGE_LABEL: Record<Page, string> = {
  overview: "概览",
  analysis: "磁盘分析",
  clean: "智能清理",
  dup: "重复文件",
  uninstall: "应用卸载",
  startup: "启动项",
  optimize: "系统优化",
  settings: "设置",
};

export const PAGE_ICON: Record<Page, IconName> = {
  overview: "overview",
  analysis: "analysis",
  clean: "clean",
  dup: "dup",
  uninstall: "uninstall",
  startup: "startup",
  optimize: "optimize",
  settings: "settings",
};

export function AppProvider({ children }: { children: ReactNode }) {
  const [page, setPage] = useState<Page>("overview");
  const [theme, setTheme] = useState<Theme>(() => {
    // 优先用户手动选择；首次启动跟随系统外观
    const saved = localStorage.getItem("maclean-theme") as Theme | null;
    if (saved === "light" || saved === "dark") return saved;
    return typeof window !== "undefined" &&
      window.matchMedia?.("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light";
  });
  const [langEn, setLangEnState] = useState(false);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [scanning, setScanning] = useState(false);
  const [scanPct, setScanPct] = useState(0);
  const [scanLabel, setScanLabel] = useState("正在准备…");
  const [confirmReq, setConfirmReq] = useState<ConfirmRequest | null>(null);
  const toastId = useRef(0);

  // 主题即时生效 + 持久化
  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem("maclean-theme", theme);
  }, [theme]);

  // 启动时读 core 配置里的语言
  useEffect(() => {
    ipc
      .settingsGet()
      .then((c) => setLangEnState(!!c.lang_en))
      .catch(() => undefined);
  }, []);

  const toast = useCallback((type: Toast["type"], msg: string) => {
    const id = ++toastId.current;
    setToasts((t) => [...t, { id, type, msg }]);
    setTimeout(() => {
      setToasts((t) => t.filter((x) => x.id !== id));
    }, 3400);
  }, []);

  const startScan = useCallback(
    async (scope: ScanScope, onDone?: (items: ScanItem[]) => void) => {
      setScanning(true);
      setScanPct(0);
      setScanLabel("正在准备…");
      try {
        const items = await ipc.scan(scope, (p: ScanProgress) => {
          setScanPct(Math.max(2, Math.min(99, p.pct)));
          setScanLabel(p.label || p.stage);
        });
        setScanPct(100);
        onDone?.(items);
      } catch (e) {
        toast("warn", "扫描失败：" + String(e));
      } finally {
        setTimeout(() => setScanning(false), 300);
      }
    },
    [toast]
  );

  const setLangEn = useCallback((en: boolean) => {
    setLangEnState(en);
    ipc.settingsSet({ lang_en: en }).catch(() => undefined);
  }, []);

  const value = useMemo<AppState>(
    () => ({
      page,
      setPage,
      theme,
      toggleTheme: () => setTheme((t) => (t === "dark" ? "light" : "dark")),
      langEn,
      setLangEn,
      toasts,
      toast,
      scanning,
      scanPct,
      scanLabel,
      startScan,
      confirm: (req) => setConfirmReq(req),
      confirmReq,
      closeConfirm: () => setConfirmReq(null),
    }),
    [
      page,
      theme,
      langEn,
      setLangEn,
      toasts,
      toast,
      scanning,
      scanPct,
      scanLabel,
      startScan,
      confirmReq,
    ]
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useApp(): AppState {
  const v = useContext(Ctx);
  if (!v) throw new Error("useApp must be used within AppProvider");
  return v;
}
