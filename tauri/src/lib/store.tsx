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
import { listen } from "@tauri-apps/api/event";
import { ipc } from "./ipc";
import type {
  CleanItemReq,
  CleanLogEntry,
  CleanProgress,
  CleanReport,
  ResultScope,
  ScanItem,
  ScanPartialEvent,
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
  /** 勾选项状态通过参数回传；无勾选项的既有调用可忽略该参数 */
  onConfirm: (toggleOn?: boolean) => void | Promise<void>;
  /** 可选的单一勾选项（如"本次也把安全项移入废纸篓"），仅允许更保守的选择 */
  confirmToggle?: { label: string; defaultOn?: boolean };
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
  /**
   * 本轮扫描中「只拿到部分结果」的模块中文名（磁盘繁忙 / 不可达目录导致未完整统计）。
   * 非空时概览 / 智能清理页显示提示横幅；发起新一轮扫描时清空。
   */
  partialScopes: string[];
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
  /** 重复文件页行级删除单个副本后的组内移除 */
  removeDupCopy: (groupPath: string, copyPath: string) => void;
  startScan: (
    scope: ScanScope,
    onDone?: (items: ScanItem[]) => void
  ) => Promise<void>;
  confirm: (req: ConfirmRequest) => void;
  confirmReq: ConfirmRequest | null;
  closeConfirm: () => void;
  /** 是否正在执行删除/清理（全局，供进度遮罩拦截重复操作） */
  cleaning: boolean;
  /** 每次成功删除（移入废纸篓）后递增，供概览等页重新拉取磁盘/废纸篓占用 */
  cleanNonce: number;
  /**
   * 本次运行会话内经 maclean 成功移入废纸篓的字节数（跨页累计）。
   * macOS 未授予「完全磁盘访问」时无法读取废纸篓真实大小（TCC 拦截），
   * 故只统计我们自己本次删除的量——始终真实；重启应用后归零。
   */
  sessionTrashBytes: number;
  /** 删除进度（已处理/总数/拦截等） */
  cleanProgress: CleanProgress | null;
  /** 本次删除实时日志（最近若干条，遮罩内滚动展示） */
  cleanLogs: CleanLogEntry[];
  /**
   * 统一删除入口：所有页面（智能清理/重复文件/磁盘分析）都走这里，
   * 由 store 维护全局进度与日志；调用方仍负责预览、确认弹窗与结果后处理。
   */
  executeClean: (
    reqs: CleanItemReq[],
    forceTrashSafe?: boolean
  ) => Promise<CleanReport>;
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
  // 部分结果模块（仅本轮扫描内存态）：收到 scan-partial 累积去重，新一轮扫描开始时清空。
  const [partialScopes, setPartialScopes] = useState<string[]>([]);
  const [results, setResults] = useState<Record<ResultScope, ScanItem[]>>(emptyResults);
  const [ready, setReady] = useState<Record<ResultScope, boolean>>(emptyReady);
  const [scanTick, setScanTick] = useState(0);
  const [lastScope, setLastScope] = useState<ResultScope | null>(null);
  const [confirmReq, setConfirmReq] = useState<ConfirmRequest | null>(null);
  const [cleaning, setCleaning] = useState(false);
  const [cleanNonce, setCleanNonce] = useState(0);
  const [sessionTrashBytes, setSessionTrashBytes] = useState(0);
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

  // 全局监听「部分结果」事件：扫描在磁盘繁忙下只完成部分模块 / 分段时，后端用本事件
  // 告知，UI 显示横幅提示「数值可能偏小、空闲后重扫」。监听器与扫描调用解耦，App 生命周期内常驻。
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<ScanPartialEvent>("scan-partial", (e) => {
      const add = Array.isArray(e.payload?.scopes) ? e.payload.scopes : [];
      setPartialScopes((prev) => {
        const next = [...prev];
        for (const name of add) {
          if (name && !next.includes(name)) next.push(name);
        }
        return next;
      });
    })
      .then((u) => {
        unlisten = u;
      })
      .catch(() => undefined);
    return () => unlisten?.();
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
      // 新一轮单模块扫描：清空上一轮的部分结果提示（结束后若仍不完整会重新收到事件）。
      setPartialScopes([]);
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
    // 新一轮体检：清空上一轮的部分结果提示（结束后若仍有模块不完整会重新收到事件）。
    setPartialScopes([]);
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

  /** 重复文件页：行级删除单个副本后，从组内移除该副本并回减大小（不整组移除） */
  const removeDupCopy = useCallback((groupPath: string, copyPath: string) => {
    setResults((prev) => ({
      ...prev,
      dup: prev.dup.map((i) => {
        if (i.path !== groupPath) return i;
        const idx = i.batch_paths.indexOf(copyPath);
        if (idx === -1) return i;
        const batch_paths = i.batch_paths.filter((_, k) => k !== idx);
        const batch_mtimes = i.batch_mtimes.filter((_, k) => k !== idx);
        if (batch_paths.length === 0) {
          return {
            ...i,
            batch_paths,
            batch_mtimes,
            deletable: false,
            undeletable_reason: "该组副本已全部删除",
          };
        }
        // 同组文件等大，按份数估算单份大小并回减（近似展示，重扫后精确）
        const per = i.size_bytes / (i.batch_paths.length + 1);
        return {
          ...i,
          batch_paths,
          batch_mtimes,
          size_bytes: Math.max(0, Math.round(i.size_bytes - per)),
        };
      }),
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
    async (reqs: CleanItemReq[], forceTrashSafe = false) => {
      setCleanLogs([]);
      setCleanProgress(null);
      setCleaning(true);
      try {
        // 删除出口在所有页面统一决策（重复文件 / 磁盘分析 / 卸载 / 智能清理共用）：
        // 注意/高级项强制进废纸篓；安全/缓存项按设置（smart 永久删 / trash 进
        // 废纸篓）并支持确认弹窗的本次覆盖。后端对风险项亦会再次强制，双保险。
        let strategy: "smart" | "trash" = "smart";
        try {
          const cfg = await ipc.settingsGet();
          if (cfg.settings_delete_strategy === "trash") strategy = "trash";
        } catch {
          /* 读取设置失败时用默认 smart */
        }
        const decided = reqs.map((r) => {
          const risky = r.recommend !== "Safe" && r.recommend !== "CacheOnly";
          return { ...r, use_trash: risky || strategy !== "smart" || forceTrashSafe };
        });
        const report = await ipc.cleanExecute(
          decided,
          langEn,
          (l) =>
            setCleanLogs((prev) => {
              const next = [...prev, l];
              return next.length > 300 ? next.slice(next.length - 300) : next;
            }),
          (p) => setCleanProgress(p)
        );
        // 有项真正被删除：通知概览等页刷新磁盘用量。取消 / 全部被拦截时不计。
        if (!report.cancelled && report.deleted > 0) {
          setCleanNonce((n) => n + 1);
          // 仅「移入废纸篓」的体量才会计入提示（永久删除已立即释放、不经废纸篓）。
          const trashed = decided
            .filter((r) => r.use_trash)
            .reduce((s, r) => s + (r.size_bytes || 0), 0);
          if (trashed > 0) setSessionTrashBytes((b) => b + trashed);
        }
        return report;
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
      partialScopes,
      results,
      ready,
      scanTick,
      lastScope,
      startFullScan,
      removePaths,
      removeDupCopy,
      startScan,
      confirm: (req) => setConfirmReq(req),
      confirmReq,
      closeConfirm: () => setConfirmReq(null),
      cleaning,
      cleanNonce,
      sessionTrashBytes,
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
      partialScopes,
      results,
      ready,
      scanTick,
      lastScope,
      startFullScan,
      removePaths,
      removeDupCopy,
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
