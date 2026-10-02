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
import type { ResultScope, ScanItem, ScanProgress, ScanScope } from "./types";
import type { IconName } from "../components/Icon";

/**
 * 全量体检（scope=full）的模块产出顺序。
 * 点一次全量扫描，结果按此顺序逐模块写入全局缓存，所有页面复用同一份数据。
 */
export const FULL_SEQUENCE: ResultScope[] = ["all", "large", "dup", "apps"];

export const FULL_MODULE_LABEL: Record<ResultScope, string> = {
  all: "缓存与应用数据",
  large: "磁盘大文件",
  dup: "重复文件",
  apps: "已安装应用",
};

const emptyResults = (): Record<ResultScope, ScanItem[]> => ({
  all: [],
  large: [],
  dup: [],
  apps: [],
});
const emptyReady = (): Record<ResultScope, boolean> => ({
  all: false,
  large: false,
  dup: false,
  apps: false,
});

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
  /** 是否正在跑全量体检（false 表示单模块扫描或空闲） */
  fullRunning: boolean;
  /** 全量体检当前进行中的模块 */
  runningScope: ResultScope | null;
  /** 单模块重新扫描时的目标模块 */
  singleScope: ResultScope | null;
  /** 各模块扫描结果（跨页面共享、切页复用） */
  results: Record<ResultScope, ScanItem[]>;
  /** 各模块是否已完成过至少一次扫描 */
  ready: Record<ResultScope, boolean>;
  /** 每次某模块扫描结果写入时递增（供页面初始化勾选态） */
  scanTick: number;
  /** 最近一次写入结果的模块 */
  lastScope: ResultScope | null;
  /** 全量体检：一次扫描全部模块，结果逐模块入库 */
  startFullScan: () => Promise<void>;
  /** 删除/卸载后，从某模块缓存中移除已处理项（不触发勾选重置） */
  removePaths: (scope: ResultScope, paths: Set<string>) => void;
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
  const [fullRunning, setFullRunning] = useState(false);
  const [runningScope, setRunningScope] = useState<ResultScope | null>(null);
  const [singleScope, setSingleScope] = useState<ResultScope | null>(null);
  const [results, setResults] = useState<Record<ResultScope, ScanItem[]>>(emptyResults);
  const [ready, setReady] = useState<Record<ResultScope, boolean>>(emptyReady);
  const [scanTick, setScanTick] = useState(0);
  const [lastScope, setLastScope] = useState<ResultScope | null>(null);
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

  /** 把一个模块的扫描结果写入全局缓存并标记就绪（供全量/单模块共用） */
  const putModule = useCallback((scope: ResultScope, items: ScanItem[]) => {
    setResults((prev) => ({ ...prev, [scope]: items }));
    setReady((prev) => ({ ...prev, [scope]: true }));
    setLastScope(scope);
    setScanTick((t) => t + 1);
  }, []);

  /** 单模块扫描（用于各页「重新扫描」）；结果同样写入全局缓存 */
  const startScan = useCallback(
    async (scope: ScanScope, onDone?: (items: ScanItem[]) => void) => {
      if (scanning) return;
      const isModule = (FULL_SEQUENCE as readonly string[]).includes(scope);
      setScanning(true);
      setFullRunning(false);
      setSingleScope(isModule ? (scope as ResultScope) : null);
      setScanPct(0);
      setScanLabel("正在准备…");
      try {
        const items = await ipc.scan(scope, (p: ScanProgress) => {
          setScanPct(Math.max(2, Math.min(99, p.pct)));
          setScanLabel(p.label || p.stage);
        });
        setScanPct(100);
        if (isModule) putModule(scope as ResultScope, items);
        onDone?.(items);
      } catch (e) {
        toast("warn", "扫描失败：" + String(e));
      } finally {
        setTimeout(() => {
          setScanning(false);
          setSingleScope(null);
        }, 300);
      }
    },
    [toast, putModule, scanning]
  );

  /** 全量体检：一次扫描全部模块，结果逐模块入库，扫描中可切换页面 */
  const startFullScan = useCallback(async () => {
    if (scanning) return;
    // 新一轮体检：清空旧结果，逐模块重新填入
    setResults(emptyResults());
    setReady(emptyReady());
    setLastScope(null);
    setFullRunning(true);
    setSingleScope(null);
    setRunningScope("all");
    setScanning(true);
    setScanPct(2);
    setScanLabel("开发者缓存");
    try {
      await ipc.scan(
        "full",
        (p: ScanProgress) => {
          setScanPct(Math.max(2, Math.min(99, p.pct)));
          setScanLabel(p.label || p.stage);
        },
        (m) => {
          putModule(m.scope, m.items);
          const idx = FULL_SEQUENCE.indexOf(m.scope);
          setRunningScope(
            idx >= 0 && idx + 1 < FULL_SEQUENCE.length ? FULL_SEQUENCE[idx + 1] : null
          );
        }
      );
      setScanPct(100);
    } catch (e) {
      toast("warn", "全量体检失败：" + String(e));
    } finally {
      setTimeout(() => {
        setScanning(false);
        setFullRunning(false);
        setRunningScope(null);
      }, 300);
    }
  }, [scanning, putModule, toast]);

  /** 删除/卸载成功后从某模块缓存中移除已处理项（不重置页面勾选） */
  const removePaths = useCallback((scope: ResultScope, paths: Set<string>) => {
    setResults((prev) => ({
      ...prev,
      [scope]: prev[scope].filter((i) => !paths.has(i.path)),
    }));
  }, []);

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
      fullRunning,
      runningScope,
      singleScope,
      results,
      ready,
      scanTick,
      lastScope,
      startFullScan,
      removePaths,
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
      fullRunning,
      runningScope,
      singleScope,
      results,
      ready,
      scanTick,
      lastScope,
      startFullScan,
      removePaths,
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
