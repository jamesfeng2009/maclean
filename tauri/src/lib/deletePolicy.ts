import { useEffect, useState } from "react";
import { ipc } from "./ipc";

/** 删除方式策略 */
export type DelStrategy = "smart" | "trash";

/** 安全/缓存等级（可永久删的那一档）；其余为风险项，永远进废纸篓 */
export const isSafeRecommend = (recommend: string): boolean =>
  recommend === "Safe" || recommend === "CacheOnly";

export interface DeletePlan {
  /** 安全/缓存项数 */
  safeN: number;
  /** 注意/高级项数 */
  riskN: number;
  /** 默认确认下将永久删除的项数 */
  permanentN: number;
  /** 默认确认下将移入废纸篓的项数 */
  trashN: number;
}

/** 依据策略与所选请求，算出默认确认下各删除出口的项数（仅用于展示口径） */
export function planDelete(
  reqs: { recommend: string }[],
  strategy: DelStrategy
): DeletePlan {
  const safeN = reqs.filter((r) => isSafeRecommend(r.recommend)).length;
  const riskN = reqs.length - safeN;
  const permanentN = strategy === "smart" ? safeN : 0;
  const trashN = riskN + (strategy === "smart" ? 0 : safeN);
  return { safeN, riskN, permanentN, trashN };
}

/** 生成确认弹窗的删除去向说明（不含路径数量） */
export function deleteSubText(p: DeletePlan): string {
  const parts: string[] = [];
  if (p.permanentN > 0)
    parts.push(`永久删除 ${p.permanentN} 项安全垃圾（不可恢复、立即释放空间）`);
  if (p.trashN > 0)
    parts.push(`把 ${p.trashN} 项移入废纸篓（可恢复，清空废纸篓后才释放空间）`);
  return parts.length ? parts.join("；") : "没有可清理的项目";
}

/** 读取当前删除方式策略（进页面拉一次；设置页改后切回会重挂刷新） */
export function useDeleteStrategy(): DelStrategy {
  const [strategy, setStrategy] = useState<DelStrategy>("smart");
  useEffect(() => {
    ipc
      .settingsGet()
      .then((c) => setStrategy(c.settings_delete_strategy === "trash" ? "trash" : "smart"))
      .catch(() => undefined);
  }, []);
  return strategy;
}
