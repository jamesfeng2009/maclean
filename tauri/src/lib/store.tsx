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
import type {
  CleanItemReq,
  CleanLogEntry,
  CleanProgress,
  CleanReport,
  ResultScope,
  ScanItem,
  ScanProgress,
  ScanScope,
} from "./types";
import type { IconName } from "../components/Icon";

/**
 * 可模块化扫描、结果写入全局缓存并跨页复用的全部模块。
 * 单页「重新扫描」时用它判断 scope 是否为模块（含重复文件）。
 */
export const MODULE_SCOPES: readonly ResultScope[] = ["all", "large", "dup", "apps"];

/**
 * 一键「全量体检」（scope=full）实际并行执行的模块。
 *
 * 刻意不含 `dup`（重复文件）：重复文件需对候选文件逐个做内容哈希（SHA256），
 * 是天然的重计算，真机实测在重开发机上单独就要跑数百秒；把它塞进全量会
 * 长期拖住"体检完成"，并与缓存扫描争抢磁盘 IO。主流清理工具也把"重复文件"
 * 作为独立工具按需运行。因此全量只跑 缓存/大文件/应用 三个核心模块，
 * 重复文件请在「重复文件」页点「开始扫描」单独触发（结果同样全局复用）。
 */
export const FULL_SEQUENCE: ResultScope[] = ["all", "large", "apps"];

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
  /**
   * 从概览等页面跳转到「智能清理」并聚焦某个分类：自动展开该组、滚动定位、高亮。
   * nonce 保证重复点击同一分类也能重新触发（展开 + 高亮 + 微信占用分析）。
   */
  cleanFocus: { category: string; nonce: number } | null;
  focusClean: (category: string) => void;
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
  /** 全量体检中**当前正在并行扫描**的模块（可多个同时进行） */
  runningScopes: ResultScope[];
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
  /** 是否正在执行删除/清理（全局，供进度遮罩拦截重复操作） */
  cleaning: boolean;
  /** 删除进度（已处理/总数/拦截等） */
  cleanProgress: CleanProgress | null;
  /** 本次删除实时日志（最近若干条，遮罩内滚动展示） */
  cleanLogs: CleanLogEntry[];
  /**
   * 统一删除入口：所有页面（智能清理/重复文件/磁盘分析）都走这里，
   * 由 store 维护全局进度与日志；调用方仍负责预览、确认弹窗与结果后处理。
   */
  executeClean: (reqs: CleanItemReq[]) => Promise<CleanReport>;
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
  const [cleanFocus, setCleanFocus] = useState<{ category: string; nonce: number } | null>(null);
  const cleanFocusNonce = useRef(0);
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
  const [runningScopes, setRunningScopes] = useState<ResultScope[]>([]);
  const [singleScope, setSingleScope] = useState<ResultScope | null>(null);
  const [results, setResults] = useState<Record<ResultScope, ScanItem[]>>(emptyResults);
  const [ready, setReady] = useState<Record<ResultScope, boolean>>(emptyReady);
  const [scanTick, setScanTick] = useState(0);
  const [lastScope, setLastScope] = useState<ResultScope | null>(null);
  const [confirmReq, setConfirmReq] = useState<ConfirmRequest | null>(null);
  const [cleaning, setCleaning] = useState(false);
  const [cleanProgress, setCleanProgress] = useState<CleanProgress | null>(null);
  const [cleanLogs, setCleanLogs] = useState<CleanLogEntry[]>([]);
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

  /** 跳转到智能清理页并请求聚焦某个分类（每次点击 nonce 递增，重复点击同项也生效） */
  const focusClean = useCallback((category: string) => {
    cleanFocusNonce.current += 1;
    setCleanFocus({ category, nonce: cleanFocusNonce.current });
    setPage("clean");
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
      const isModule = (MODULE_SCOPES as readonly string[]).includes(scope);
      setScanning(true);
      setFullRunning(false);
      setSingleScope(isModule ? (scope as ResultScope) : null);
      setScanPct(0);
      setScanLabel("正在准备…");
      try {
        const items = await ipc.scan(scope, (p: ScanProgress) => {
          // 单调不减：并行/事件乱序时进度条不回退（下一轮扫描开始时重置为 2）
          setScanPct((prev) => Math.max(prev, Math.max(2, Math.min(99, p.pct))));
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

  /** 全量体检：后端并行扫描各模块，结果逐模块入库；扫描中可自由切换页面 */
  const startFullScan = useCallback(async () => {
    if (scanning) return;
    // 新一轮体检：清空旧结果与运行态，逐模块重新填入
    setResults(emptyResults());
    setReady(emptyReady());
    setLastScope(null);
    setRunningScopes([]);
    setFullRunning(true);
    setSingleScope(null);
    setScanning(true);
    setScanPct(2);
    setScanLabel("开始全盘体检");
    try {
      await ipc.scan(
        "full",
        (p: ScanProgress) => {
          setScanPct((prev) => Math.max(prev, Math.max(2, Math.min(100, p.pct))));
          setScanLabel(p.label || p.stage);
        },
        (m) => {
          if (m.status === "start") {
            // 模块进入扫描中（并行时可能多个同时进行）
            setRunningScopes((prev) =>
              prev.includes(m.scope) ? prev : [...prev, m.scope]
            );
          } else {
            // 模块完成：入库并移出运行中集合（缺省 status 也按完成处理）
            putModule(m.scope, m.items);
            setRunningScopes((prev) => prev.filter((s) => s !== m.scope));
          }
        }
      );
      setScanPct(100);
    } catch (e) {
      toast("warn", "全量体检失败：" + String(e));
    } finally {
      setTimeout(() => {
        setScanning(false);
        setFullRunning(false);
        setRunningScopes([]);
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

  /**
   * 统一删除入口：重置进度/日志 → 调 IPC（订阅 clean-log/clean-progress）→
   * 结束后复位。无论成功失败都要解除遮罩，避免删除按钮“看起来没反应”。
   */
  const executeClean = useCallback(
    async (reqs: CleanItemReq[]) => {
      setCleanLogs([]);
      setCleanProgress(null);
      setCleaning(true);
      try {
        return await ipc.cleanExecute(
          reqs,
          langEn,
          (l) =>
            setCleanLogs((prev) => {
              const next = [...prev, l];
              return next.length > 300 ? next.slice(next.length - 300) : next;
            }),
          (p) => setCleanProgress(p)
        );
      } finally {
        setCleaning(false);
      }
    },
    [langEn]
  );

  const value = useMemo<AppState>(
    () => ({
      page,
      setPage,
      cleanFocus,
      focusClean,
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
      runningScopes,
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
      cleaning,
      cleanProgress,
      cleanLogs,
      executeClean,
    }),
    [
      page,
      cleanFocus,
      focusClean,
      theme,
      langEn,
      setLangEn,
      toasts,
      toast,
      scanning,
      scanPct,
      scanLabel,
      fullRunning,
      runningScopes,
      singleScope,
      results,
      ready,
      scanTick,
      lastScope,
      startFullScan,
      removePaths,
      startScan,
      confirmReq,
      executeClean,
    ]
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useApp(): AppState {
  const v = useContext(Ctx);
  if (!v) throw new Error("useApp must be used within AppProvider");
  return v;
}
