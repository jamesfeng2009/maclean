import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ipc } from "../lib/ipc";
import { fmt, shortPath } from "../lib/format";
import type { CleanItemReq, ImBreakdown, ScanItem } from "../lib/types";
import { useApp } from "../lib/store";
import { Icon } from "../components/Icon";
import { Badge, Empty, PageHeader } from "../components/ui";

type GroupRisk = "safe" | "caution" | "advanced" | "protected";

interface Group {
  name: string;
  items: ScanItem[];
  /** 仅统计可删除项的可回收大小 */
  size: number;
  deletableCount: number;
  protectedCount: number;
  safeCount: number;
  cautionCount: number;
  advancedCount: number;
}

/** 组内最高风险：受保护(整组不可清理) > 高级 > 注意 > 安全 */
function groupRisk(g: Group): GroupRisk {
  // 整组没有任何可删除项（如微信聊天数据、Docker 虚拟机磁盘）：不是“安全可清理”，
  // 而是“受保护不可删”，徽章与竖条用中性品牌色，避免绿色“安全”误导用户去删。
  if (g.deletableCount === 0 && g.protectedCount > 0) return "protected";
  if (g.advancedCount > 0) return "advanced";
  if (g.cautionCount > 0) return "caution";
  return "safe";
}

/** 整组可删除项是否都属于安全/缓存（默认勾选只允许这种纯安全组） */
function isPureSafe(g: Group): boolean {
  return g.deletableCount > 0 && g.advancedCount === 0 && g.cautionCount === 0;
}

/** 把扫描项按分类聚合成组，并统计各风险等级数量 */
function groupItems(src: ScanItem[]): Group[] {
  const m = new Map<string, Group>();
  for (const it of src) {
    let g = m.get(it.category);
    if (!g) {
      g = {
        name: it.category,
        items: [],
        size: 0,
        deletableCount: 0,
        protectedCount: 0,
        safeCount: 0,
        cautionCount: 0,
        advancedCount: 0,
      };
      m.set(it.category, g);
    }
    g.items.push(it);
    if (it.deletable) {
      g.size += it.size_bytes;
      g.deletableCount += 1;
      if (it.recommend === "Advanced") g.advancedCount += 1;
      else if (it.recommend === "Caution") g.cautionCount += 1;
      else g.safeCount += 1;
    } else {
      g.protectedCount += 1;
    }
  }
  return [...m.values()].sort((a, b) => b.size - a.size);
}

const GROUP_ICON: Record<string, string> = {
  浏览器缓存: "folder",
  系统日志: "doc",
  安装包: "file",
  模拟器镜像: "cpu",
  AI模型: "model",
};

function groupIcon(name: string): string {
  if (GROUP_ICON[name]) return GROUP_ICON[name];
  if (name.includes("Docker") || name.includes("K8s")) return "container";
  if (name.includes("模型") || name.includes("缓存")) return "cpu";
  return "folder";
}

/**
 * 受支持 IM 的 bundle id → 中文名。须与 maclean-core `im_data::IM_APPS`
 * 保持同步：仅用于 UI 层从扫描项路径识别"这是 IM"，权威判定仍在 Rust 侧。
 */
const IM_BUNDLES: Record<string, string> = {
  "com.tencent.xinWeChat": "微信",
  "com.tencent.qq": "QQ",
  "com.tencent.WeWorkMac": "企业微信",
  "com.tencent.workbuddy.mac": "企业微信",
};

/** 若扫描项路径是某 IM 的沙盒 Documents 根，返回其 bundle 与中文名 */
function imFromPath(path: string): { bundle: string; name: string } | null {
  const m = path.match(/\/Library\/Containers\/([^/]+)\/Data\/Documents\/?$/);
  if (!m) return null;
  const name = IM_BUNDLES[m[1]];
  return name ? { bundle: m[1], name } : null;
}

/** IM 只读占用构成（消息库/视频/图片/文件/缓存… 占比条） */
function ImBreakdownPanel({ b }: { b: ImBreakdown }) {
  const pctOf = (n: number) => (b.total_bytes > 0 ? Math.round((n / b.total_bytes) * 100) : 0);
  return (
    <div className="im-parts">
      <div className="im-parts-hint">
        共 <b>{fmt(b.total_bytes)}</b> · 请在{b.app_name}内清理（{b.storage_hint}），maclean 不直接删除
      </div>
      {b.parts.map((p) => {
        const pct = pctOf(p.size_bytes);
        return (
          <div className="im-part" key={p.key}>
            <span className="im-part-label">{p.label}</span>
            <span className="im-part-track">
              <span
                className={`im-part-fill fill-${p.key}`}
                style={{ width: `${Math.max(p.size_bytes > 0 ? 2 : 0, pct)}%` }}
              />
            </span>
            <span className="im-part-size">{fmt(p.size_bytes)}</span>
            <span className="im-part-pct">{pct}%</span>
          </div>
        );
      })}
    </div>
  );
}

export function Clean() {
  const {
    startScan,
    scanning,
    fullRunning,
    confirm,
    toast,
    results,
    ready,
    scanTick,
    lastScope,
    startFullScan,
    removePaths,
    executeClean,
    cleanFocus,
  } = useApp();
  // 数据来自全局缓存的 all 模块：全量体检或单模块重新扫描写入后，本页直接复用
  const items = results.all;
  const hasScanned = ready.all;
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  // 概览跳转聚焦：分组 DOM 引用 + 短暂高亮的分类名
  const groupRefs = useRef<Map<string, HTMLDivElement | null>>(new Map());
  const [flashCat, setFlashCat] = useState<string | null>(null);
  const flashTimer = useRef<number | null>(null);

  // IM 受保护项：按需只读分析占用、打开应用
  const [imOpen, setImOpen] = useState<Set<string>>(new Set());
  const [imData, setImData] = useState<Record<string, ImBreakdown>>({});
  const [imBusy, setImBusy] = useState<string | null>(null);
  const [imErr, setImErr] = useState<Record<string, string>>({});

  const toggleIm = useCallback(
    async (it: ScanItem) => {
      if (imOpen.has(it.path)) {
        setImOpen((s) => {
          const n = new Set(s);
          n.delete(it.path);
          return n;
        });
        return;
      }
      setImOpen((s) => new Set(s).add(it.path));
      if (!imData[it.path] && imBusy !== it.path) {
        setImBusy(it.path);
        try {
          const b = await ipc.imBreakdown(it.path);
          setImData((d) => ({ ...d, [it.path]: b }));
        } catch (e) {
          setImErr((x) => ({ ...x, [it.path]: String(e) }));
        } finally {
          setImBusy(null);
        }
      }
    },
    [imOpen, imData, imBusy]
  );

  const openImApp = useCallback(
    async (bundle: string, name: string) => {
      try {
        await ipc.appOpen(bundle);
      } catch (e) {
        toast("warn", `打开${name}失败：` + String(e));
      }
    },
    [toast]
  );

  // 页头「重新扫描」只刷新智能清理所属的 all 模块；首次空态按钮走全量体检
  const refresh = useCallback(() => {
    void startScan("all");
  }, [startScan]);

  // 每次 all 模块结果写入（全量体检到达该模块 / 手动重新扫描）后，默认仅勾选纯安全组。
  // 仅以扫描完成计数 scanTick 为触发：删除/卸载更新缓存不应重置用户勾选。
  useEffect(() => {
    if (lastScope === "all" && results.all.length > 0) {
      setChecked(
        new Set(
          groupItems(results.all)
            .filter(isPureSafe)
            .map((g) => g.name)
        )
      );
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scanTick]);

  const groups = useMemo(() => groupItems(items), [items]);

  // 概览页点某分类卡片 → focusClean：自动展开该组、平滑滚动定位、短暂高亮；
  // 微信/QQ 等 IM 受保护组额外自动展开只读占用构成。以 nonce 为触发，重复点击同项也生效。
  useEffect(() => {
    const f = cleanFocus;
    if (!f) return;
    const g = groups.find((x) => x.name === f.category);
    if (!g) return;
    setOpen((s) => new Set(s).add(f.category));
    const imItem = g.items.find((it) => !it.deletable && imFromPath(it.path));
    // 先让切页 / 展开 / 滚动 / 高亮瞬时完成，再在转场稳定后发起 IM 占用分析；
    // 分析本身在 Rust 阻塞线程池跑（不卡 UI），延迟只为主线程转场动画不掉帧。
    const t = window.setTimeout(() => {
      groupRefs.current.get(f.category)?.scrollIntoView({ behavior: "smooth", block: "start" });
      setFlashCat(f.category);
      if (flashTimer.current) window.clearTimeout(flashTimer.current);
      flashTimer.current = window.setTimeout(() => setFlashCat(null), 1900);
      if (imItem) void toggleIm(imItem);
    }, 120);
    return () => window.clearTimeout(t);
    // 仅以跳转请求(nonce)为触发，groups/toggleIm 取当次渲染闭包即可
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cleanFocus?.nonce]);

  // 可操作的分类（至少有 1 个可删除项）
  const selectableGroups = useMemo(() => groups.filter((g) => g.deletableCount > 0), [groups]);
  const selectAll = useCallback(
    (mode: "all" | "safe" | "none") => {
      if (mode === "none") setChecked(new Set());
      else if (mode === "safe") setChecked(new Set(selectableGroups.filter(isPureSafe).map((g) => g.name)));
      else setChecked(new Set(selectableGroups.map((g) => g.name)));
    },
    [selectableGroups]
  );

  const selectedItems = useMemo(
    () =>
      items.filter(
        (i) => checked.has(i.category) && i.deletable
      ),
    [items, checked]
  );
  const selectedSize = selectedItems.reduce((s, i) => s + i.size_bytes, 0);
  const selCaution = selectedItems.filter((i) => i.recommend === "Caution").length;
  const selAdvanced = selectedItems.filter((i) => i.recommend === "Advanced").length;
  const hasRisk = selAdvanced > 0 || selCaution > 0;
  // 已选中的风险分类名（确认弹窗里点名告知）
  const riskCatNames = useMemo(
    () =>
      [...new Set(selectedItems.filter((i) => i.recommend !== "Safe" && i.recommend !== "CacheOnly").map((i) => i.category))],
    [selectedItems]
  );

  const toggleGroup = (name: string) => {
    setChecked((s) => {
      const n = new Set(s);
      if (n.has(name)) n.delete(name);
      else n.add(name);
      return n;
    });
  };
  const toggleOpen = (name: string) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(name)) n.delete(name);
      else n.add(name);
      return n;
    });

  const runClean = async () => {
    const reqs: CleanItemReq[] = selectedItems.map((i) => ({
      path: i.path,
      category: i.category,
      batch_paths: i.batch_paths,
      size_bytes: i.size_bytes,
      recommend: i.recommend,
    }));

    // Rust 侧 dry-run 复核：safety 闸门 + 当前可删除性
    let preview;
    try {
      preview = await ipc.cleanPreview(reqs);
    } catch (e) {
      toast("warn", "安全预检失败：" + String(e));
      return;
    }
    const blocked = preview.filter((p) => !p.allowed);
    const allowed = preview.filter((p) => p.allowed);
    if (allowed.length === 0) {
      toast("warn", "所选项目均未通过安全检查，未执行删除");
      return;
    }
    const byKey = new Map(allowed.map((p) => [p.path + "|" + p.category, p]));
    const finalReqs = reqs.filter((r) => byKey.has(r.path + "|" + r.category));
    const paths = finalReqs.flatMap((r) => [r.path, ...r.batch_paths]);

    if (blocked.length > 0) {
      toast("info", `${blocked.length} 项未通过安全检查，已自动排除`);
    }

    confirm({
      title: "确认清理选中项目？",
      sub: `将把 ${finalReqs.length} 项（含副本共 ${paths.length} 个路径）移入废纸篓，可恢复。`,
      warn: hasRisk
        ? `包含「注意/高级」风险项（涉及分类：${riskCatNames.join("、")}）：请确认你了解这些文件的用途，删除后可能需要重新下载、重新编译或重新登录。`
        : "所选均为安全/缓存项；操作会先经过 maclean-core 安全闸门，系统关键目录会被拦截。",
      items: paths,
      confirmText: `安全清理 ${fmt(selectedSize)}`,
      onConfirm: async () => {
        setBusy(true);
        try {
          const rep = await executeClean(finalReqs);
          if (rep.cancelled) {
            toast("warn", "清理已取消");
          } else {
            toast(
              "success",
              `已清理 ${rep.deleted} 项${rep.intercepted ? `，拦截 ${rep.intercepted} 项` : ""}${
                rep.need_password ? `，${rep.need_password} 项需要管理员权限（请用桌面版）` : ""
              }`
            );
            // 从全局缓存移除已清理项，不自动重扫；需要最新结果请用户手动重新扫描
            const removed = new Set(finalReqs.map((r) => r.path));
            removePaths("all", removed);
            setChecked(new Set());
          }
        } catch (e) {
          toast("warn", "清理失败：" + String(e));
        } finally {
          setBusy(false);
        }
      },
    });
  };

  return (
    <div>
      <PageHeader
        title="智能清理"
        sub="选择要清理的分类；所有删除都会过安全闸门并优先移入废纸篓"
        action={
          <button
            className="btn-secondary"
            onClick={hasScanned ? refresh : startFullScan}
            style={{ height: 34 }}
          >
            <Icon name="refresh" size={14} /> {hasScanned ? "重新扫描" : "开始扫描"}
          </button>
        }
      />

      {groups.length === 0 ? (
        <Empty
          icon="clean"
          text={
            hasScanned
              ? "暂未发现可清理项"
              : scanning
                ? fullRunning
                  ? "正在全盘体检，缓存与应用数据模块完成后自动呈现…"
                  : "正在重新扫描可清理项…"
                : "尚未扫描，点击下方按钮开始一次全盘体检（只读扫描，不会删除文件）"
          }
          action={
            !scanning && !hasScanned ? (
              <button className="btn-primary" onClick={startFullScan}>
                <Icon name="zap" size={15} /> 开始扫描
              </button>
            ) : undefined
          }
        />
      ) : (
        <>
          <div className="select-bar">
            <span className="select-info">
              共 {selectableGroups.length} 个可清理分类 · 已选 {selectedItems.length} 项 · 可回收{" "}
              {fmt(selectedSize)}
            </span>
            <div className="grow" />
            <button
              className="btn-secondary select-btn"
              onClick={() => selectAll("all")}
              disabled={scanning}
              title="勾选所有可清理分类，包括注意/高级风险项"
            >
              全选
            </button>
            <button
              className="btn-secondary select-btn"
              onClick={() => selectAll("safe")}
              disabled={scanning}
              title="只勾选整组均为安全/缓存的分类"
            >
              仅选安全项
            </button>
            <button
              className="btn-secondary select-btn"
              onClick={() => selectAll("none")}
              disabled={scanning}
            >
              清空
            </button>
          </div>

          <div className="cat-list">
            {groups.map((g) => {
              const isOn = checked.has(g.name);
              const isOpen = open.has(g.name);
              const disabled = g.deletableCount === 0;
              const risk = groupRisk(g);
              const detail = [
                `${g.deletableCount} 项可清理`,
                g.advancedCount > 0 ? `${g.advancedCount} 项高级` : "",
                g.cautionCount > 0 ? `${g.cautionCount} 项注意` : "",
                g.protectedCount > 0 ? `${g.protectedCount} 项受保护` : "",
              ]
                .filter(Boolean)
                .join(" · ");
              return (
                <div
                  key={g.name}
                  ref={(el) => {
                    groupRefs.current.set(g.name, el);
                  }}
                  className={flashCat === g.name ? "cat-focus" : undefined}
                >
                  <div
                    className={`cat-card risk-${risk}${isOn ? " checked" : ""}${disabled ? " disabled" : ""}`}
                    onClick={() => toggleOpen(g.name)}
                  >
                    <span
                      className={`chek${disabled ? " disabled" : ""}`}
                      title={disabled ? "该分类暂无可清理项" : "选择 / 取消该分类"}
                      onClick={(e) => {
                        // 勾选框独立负责选中，阻止冒泡以免同时触发展开
                        e.stopPropagation();
                        if (!disabled) toggleGroup(g.name);
                      }}
                    >
                      <Icon name="check" size={12} />
                    </span>
                    <span
                      className="ico"
                      style={{ background: "var(--brand-50)", color: "var(--brand)" }}
                    >
                      <Icon name={groupIcon(g.name)} size={20} />
                    </span>
                    <div className="meta">
                      <div className="t">
                        {g.name}
                        <span className="cat-tags">
                          {risk === "protected" ? (
                            <Badge r="Protected" />
                          ) : risk === "advanced" ? (
                            <Badge r="Advanced" />
                          ) : risk === "caution" ? (
                            <Badge r="Caution" />
                          ) : (
                            <Badge r="Safe" />
                          )}
                        </span>
                      </div>
                      <div className="d">{detail}</div>
                    </div>
                    <span className="sz">{fmt(g.size)}</span>
                    <span className="chev-rt" title={isOpen ? "收起" : "展开明细"}>
                      <Icon
                        name="chev"
                        size={16}
                        style={{
                          transition: "transform .2s",
                          transform: isOpen ? "rotate(90deg)" : "none",
                        }}
                      />
                    </span>
                  </div>

                  {isOpen && (
                    <div className="card cat-items">
                      {g.items.map((it) => {
                        const im = !it.deletable ? imFromPath(it.path) : null;
                        return (
                        <Fragment key={it.path}>
                        <div
                          className={`dup-file risk-${
                            it.deletable
                              ? it.recommend === "Advanced"
                                ? "advanced"
                                : it.recommend === "Caution"
                                  ? "caution"
                                  : "safe"
                              : "protected"
                          }`}
                          style={{ paddingLeft: 10, opacity: it.deletable ? 1 : 0.55 }}
                        >
                          {it.deletable ? <Badge r={it.recommend} /> : <Badge r="Protected" />}
                          <span className="fp" title={it.path}>
                            {shortPath(it.path)}
                          </span>
                          {it.description && (
                            <span
                              className="item-desc"
                              title={it.description}
                            >
                              {it.description}
                            </span>
                          )}
                          {!it.deletable && !im && (
                            <span className="muted" style={{ fontSize: 11 }}>
                              {it.undeletable_reason || "受保护"}
                            </span>
                          )}
                          <span className="sz">{fmt(it.size_bytes)}</span>
                        </div>

                        {im && (
                          <div className="im-guard">
                            <div className="im-guard-bar">
                              <span className="im-guard-ico">
                                <Icon name="shield" size={14} />
                              </span>
                              <span className="im-guard-tip">{it.undeletable_reason}</span>
                              <span className="grow" />
                              <button
                                type="button"
                                className="btn-secondary im-btn"
                                onClick={() => void toggleIm(it)}
                              >
                                {imOpen.has(it.path) ? "收起占用" : "查看占用构成"}
                              </button>
                              <button
                                type="button"
                                className="btn-primary im-btn"
                                onClick={() => void openImApp(im.bundle, im.name)}
                              >
                                打开{im.name}
                              </button>
                            </div>
                            {imOpen.has(it.path) && (
                              <div className="im-breakdown">
                                {imBusy === it.path ? (
                                  <span className="muted">
                                    正在后台只读统计占用，期间可正常操作；数据较多时请稍候…
                                  </span>
                                ) : imErr[it.path] ? (
                                  <span className="muted">分析失败：{imErr[it.path]}</span>
                                ) : imData[it.path] ? (
                                  <ImBreakdownPanel b={imData[it.path]} />
                                ) : null}
                              </div>
                            )}
                          </div>
                        )}
                        </Fragment>
                        );
                      })}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </>
      )}

      {selectedItems.length > 0 && (
        <div className="summary-bar">
          <span className="sel">
            已选 <b>{selectedItems.length}</b> 项 · 共 <b>{fmt(selectedSize)}</b>
          </span>
          {hasRisk && (
            <span className="sel-risk">
              <Icon name="shield" size={14} />
              {selAdvanced > 0 ? `含 ${selAdvanced} 项高级` : ""}
              {selAdvanced > 0 && selCaution > 0 ? "、" : ""}
              {selCaution > 0 ? `${selCaution} 项注意` : ""}
              ，删除后可能需重新下载 / 编译 / 登录
            </span>
          )}
          <div className="grow" />
          <button className="btn-primary danger" onClick={runClean} disabled={busy || scanning}>
            <Icon name="trash" size={16} />
            {busy ? "正在清理…" : `安全清理 ${fmt(selectedSize)}`}
          </button>
        </div>
      )}
    </div>
  );
}
