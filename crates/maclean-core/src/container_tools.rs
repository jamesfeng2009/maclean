//! 容器 / 虚拟机类工具的数据目录登记表（2026-11-13）
//!
//! 为什么要表驱动：
//! 以前 Docker、OrbStack 是**点名**的专门通道——`safety.rs` 硬编码两组路径
//! 前缀，`dev_cache.rs` 硬编码两段扫描代码。用户装 Lima / Colima / UTM /
//! Parallels / VMware 时，行为取决于"这个路径有没有被哪段 if 记得"，漏掉的
//! 要么被当成普通大文件、要么被路径硬拦成永久受保护。加一个工具要改三处，
//! 改漏一处就是事故。
//!
//! 现在的口径（三层）：
//! - **L1 本表**：已知的 GUI 容器 / 虚拟机工具，逐条登记数据根。
//!   装了 → 红线只读（删文件会破坏镜像 / 虚拟机磁盘），引导官方通道；
//!   卸了 → 孤儿残留可清理（Caution，删了要重新下载，代价写在文案里）。
//! - **L2 沙盒容器**：`~/Library/Containers/<bundle-id>` 归属天然明确，
//!   由 `scanner/app_cache.rs` 用 `installed_bundle_id_set()` 判孤儿，
//!   不认识的沙盒 App 也能覆盖。
//! - **L3 未登记目录**：只读提示、绝不自动判定可删。宁可漏空间，不猜错删掉
//!   别人的数据库。
//!
//! 新增一个工具 = 这里加一条表项 + 一条参数化测试（测试会自动覆盖全表）。

use std::path::{Path, PathBuf};

use crate::scanner::Recommend;

/// 某个工具的一处数据根（home 相对路径）
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ContainerRoot {
    /// home 相对路径；绝对路径不登记（避免把系统目录写进用户数据表）
    pub path: &'static str,
    /// 扫描结果里的分类名（UI 分组用）
    pub category: &'static str,
    /// App 仍安装时的说明
    pub label: &'static str,
    /// App 仍安装时的不可删原因（引导官方通道）
    pub hint: &'static str,
    /// App 已卸载时的说明
    pub orphan_label: &'static str,
    /// App 仍安装时的风险等级
    pub installed_recommend: Recommend,
}

/// 一个容器 / 虚拟机工具
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ContainerTool {
    pub id: &'static str,
    /// `/Applications` 或 `~/Applications` 下的 .app 名（文件系统真相）
    pub app: &'static str,
    pub display: &'static str,
    pub roots: &'static [ContainerRoot],
}

const ADVANCED: Recommend = Recommend::Advanced;
const CAUTION: Recommend = Recommend::Caution;

pub(crate) const TOOLS: &[ContainerTool] = &[
    ContainerTool {
        id: "docker-desktop",
        app: "Docker.app",
        display: "Docker Desktop",
        roots: &[
            ContainerRoot {
                path: "Library/Containers/com.docker.docker/Data/vms",
                category: "Docker虚拟机",
                label: "Docker Desktop 虚拟机数据（只展示，不直接删除；清理请走 Docker Desktop/CLI）",
                hint: "容器数据目录，禁止直接删除；请用 Docker Desktop 或 docker system prune 清理镜像",
                orphan_label: "Docker Desktop 已卸载，残留虚拟机数据（可删除释放空间）",
                installed_recommend: ADVANCED,
            },
            ContainerRoot {
                path: "Library/Containers/com.docker.docker/Data/cache",
                category: "Docker缓存",
                label: "Docker Desktop 缓存数据（只展示，不直接删除）",
                hint: "容器数据目录，禁止直接删除；请用 Docker Desktop 清理",
                orphan_label: "Docker Desktop 已卸载，残留缓存数据（可删除释放空间）",
                installed_recommend: CAUTION,
            },
        ],
    },
    ContainerTool {
        id: "orbstack",
        app: "OrbStack.app",
        display: "OrbStack",
        roots: &[
            ContainerRoot {
                path: ".orbstack",
                category: "OrbStack数据",
                label: "OrbStack 数据（只展示，不直接删除；清理请用 orbctl）",
                hint: "容器数据目录，禁止直接删除；请用 orbctl delete/reset 清理",
                orphan_label: "OrbStack 已卸载，残留数据（可删除释放空间）",
                installed_recommend: ADVANCED,
            },
            ContainerRoot {
                path: "Library/Group Containers/HUAQ24HBR6.dev.orbstack",
                category: "OrbStack容器",
                label: "OrbStack 容器/虚拟机数据（只展示，不直接删除；清理请用 orbctl）",
                hint: "容器数据目录，禁止直接删除；请用 orbctl delete/reset 清理",
                orphan_label: "OrbStack 已卸载，残留容器数据（可删除释放空间）",
                installed_recommend: ADVANCED,
            },
        ],
    },
    ContainerTool {
        id: "parallels-desktop",
        app: "Parallels Desktop.app",
        display: "Parallels Desktop",
        roots: &[ContainerRoot {
            path: "Parallels",
            category: "Parallels虚拟机",
            label: "Parallels 虚拟机数据（只展示，不直接删除；删虚拟机请在 Parallels 控制中心操作）",
            hint: "虚拟机磁盘包，直接删文件会损坏虚拟机；请在 Parallels 中删除虚拟机",
            orphan_label: "Parallels Desktop 已卸载，残留虚拟机数据（可删除释放空间）",
            installed_recommend: ADVANCED,
        }],
    },
    ContainerTool {
        id: "vmware-fusion",
        app: "VMware Fusion.app",
        display: "VMware Fusion",
        roots: &[
            ContainerRoot {
                path: "Virtual Machines",
                category: "VMware虚拟机",
                label: "VMware Fusion 虚拟机数据（只展示，不直接删除；删虚拟机请在 VMware 中操作）",
                hint: "虚拟机磁盘包，直接删文件会损坏虚拟机；请在 VMware Fusion 中删除虚拟机",
                orphan_label: "VMware Fusion 已卸载，残留虚拟机数据（可删除释放空间）",
                installed_recommend: ADVANCED,
            },
            ContainerRoot {
                path: "Library/Application Support/VMware Fusion",
                category: "VMware配置",
                label: "VMware Fusion 配置与支持文件（只展示，不直接删除）",
                hint: "虚拟机管理配置，直接删文件会影响已注册虚拟机；请用 VMware 卸载器",
                orphan_label: "VMware Fusion 已卸载，残留配置数据（可删除释放空间）",
                installed_recommend: CAUTION,
            },
        ],
    },
    ContainerTool {
        id: "virtualbox",
        app: "VirtualBox.app",
        display: "VirtualBox",
        roots: &[ContainerRoot {
            path: "VirtualBox VMs",
            category: "VirtualBox虚拟机",
            label: "VirtualBox 虚拟机数据（只展示，不直接删除；删虚拟机请在 VirtualBox 中操作）",
            hint: "虚拟机磁盘包，直接删文件会损坏虚拟机；请在 VirtualBox 中删除虚拟机",
            orphan_label: "VirtualBox 已卸载，残留虚拟机数据（可删除释放空间）",
            installed_recommend: ADVANCED,
        }],
    },
    ContainerTool {
        id: "utm",
        app: "UTM.app",
        display: "UTM",
        roots: &[ContainerRoot {
            path: "Library/Containers/com.utmapp.UTM/Data/Documents",
            category: "UTM虚拟机",
            label: "UTM 虚拟机数据（只展示，不直接删除；删虚拟机请在 UTM 中操作）",
            hint: "虚拟机磁盘包，直接删文件会损坏虚拟机；请在 UTM 中删除虚拟机",
            orphan_label: "UTM 已卸载，残留虚拟机数据（可删除释放空间）",
            installed_recommend: ADVANCED,
        }],
    },
    ContainerTool {
        id: "rancher-desktop",
        app: "Rancher Desktop.app",
        display: "Rancher Desktop",
        roots: &[ContainerRoot {
            path: "Library/Application Support/rancher-desktop",
            category: "Rancher容器",
            label: "Rancher Desktop 容器数据（只展示，不直接删除；清理请用 Rancher Desktop）",
            hint: "容器数据目录，禁止直接删除；请用 Rancher Desktop 或 nerdctl 清理",
            orphan_label: "Rancher Desktop 已卸载，残留容器数据（可删除释放空间）",
            installed_recommend: ADVANCED,
        }],
    },
];

/// 工具的 App 是否仍安装（只看文件系统真相）
///
/// 判定顺序：`/Applications` → 各 home 候选的 `~/Applications`。
/// **读不到任何 home 候选时按"已安装"返回**：保守方向永远是保护，
/// 一次异常不能把仍在使用的虚拟机磁盘判成孤儿。
pub(crate) fn tool_installed(tool: &ContainerTool, homes: &[PathBuf]) -> bool {
    if Path::new("/Applications").join(tool.app).is_dir() {
        return true;
    }
    if homes.is_empty() {
        return true;
    }
    homes
        .iter()
        .any(|h| h.join("Applications").join(tool.app).is_dir())
}

/// 命中某条登记路径时，返回 `(tool, root)`
///
/// 前缀匹配用 home 候选逐个拼，不采信进程内可改写的 `$HOME`
/// （与 `safety::protected_homes` 同源）。
pub(crate) fn match_root<'a>(
    path: &str,
    homes: &[PathBuf],
) -> Option<(&'a ContainerTool, &'a ContainerRoot)> {
    for h in homes {
        let home = h.to_string_lossy();
        for tool in TOOLS {
            for root in tool.roots {
                let prefix = format!("{}/{}", home, root.path);
                if path == prefix || path.starts_with(&format!("{}/", prefix)) {
                    return Some((tool, root));
                }
            }
        }
    }
    None
}

/// 路径是否落在「App 仍安装的容器 / 虚拟机数据」红线上
///
/// 与删除端同源：判断与扫描期 `precheck_deletability` 一致，
/// 避免出现"扫得出、删不掉"或"删得掉、其实不该删"。
pub(crate) fn redline_hit(path: &str, homes: &[PathBuf]) -> bool {
    redline_hit_with(path, homes, |tool| tool_installed(tool, homes))
}

/// 纯判定（安装状态由调用方注入，便于单测覆盖两种分支而不依赖本机装了什么）
pub(crate) fn redline_hit_with<F>(path: &str, homes: &[PathBuf], installed: F) -> bool
where
    F: Fn(&ContainerTool) -> bool,
{
    match match_root(path, homes) {
        Some((tool, _)) => installed(tool),
        None => false,
    }
}

/// 数据根的出口策略（纯函数）
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RootPolicy {
    pub deletable: bool,
    pub recommend: Recommend,
    pub reason: String,
    pub description: String,
}

pub(crate) fn root_policy(root: &ContainerRoot, app_installed: bool) -> RootPolicy {
    if app_installed {
        RootPolicy {
            deletable: false,
            recommend: root.installed_recommend,
            reason: root.hint.to_string(),
            description: root.label.to_string(),
        }
    } else {
        RootPolicy {
            deletable: true,
            // 孤儿数据里仍可能有镜像 / 虚拟机，删了要重新下载，给 Caution 不给 Safe
            recommend: CAUTION,
            reason: String::new(),
            description: root.orphan_label.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn homes() -> Vec<PathBuf> {
        vec![PathBuf::from("/Users/testuser")]
    }

    /// 全表参数化：装了必拦、卸了必放行，任何新增工具都会被这条覆盖。
    #[test]
    fn every_registered_root_is_redline_only_while_app_installed() {
        let homes = homes();
        for tool in TOOLS {
            for root in tool.roots {
                let abs = format!("/Users/testuser/{}", root.path);
                assert!(
                    redline_hit_with(&abs, &homes, |_| true),
                    "{} 安装时必须拦截: {}",
                    tool.id,
                    abs
                );
                assert!(
                    !redline_hit_with(&abs, &homes, |_| false),
                    "{} 卸载后必须放行: {}",
                    tool.id,
                    abs
                );
                // 子路径同样继承父目录的判定（防止从"文件"入口绕开）
                assert!(redline_hit_with(
                    &format!("{}/sub/file.bin", abs),
                    &homes,
                    |_| true
                ));
            }
        }
    }

    /// 表里每个数据根的出口策略必须是两套文案
    #[test]
    fn every_registered_root_has_two_distinct_policies() {
        for tool in TOOLS {
            for root in tool.roots {
                let installed = root_policy(root, true);
                assert!(!installed.deletable, "{} 安装时应不可删", tool.id);
                assert!(!installed.reason.is_empty(), "{} 安装时应给原因", tool.id);

                let orphan = root_policy(root, false);
                assert!(orphan.deletable, "{} 卸载后应可删", tool.id);
                assert!(orphan.reason.is_empty(), "{} 可删时不该带原因", tool.id);
                assert!(
                    orphan.description.contains("已卸载"),
                    "{} 孤儿文案必须说明已卸载: {}",
                    tool.id,
                    orphan.description
                );
                assert_eq!(
                    orphan.recommend,
                    Recommend::Caution,
                    "{} 孤儿数据不能给 Safe",
                    tool.id
                );
            }
        }
    }

    /// 未登记目录绝不进红线（L3 兜底：不认识就不动手）
    #[test]
    fn unregistered_paths_are_never_redline() {
        let homes = homes();
        for p in [
            "/Users/testuser/.lima/default/disk",
            "/Users/testuser/.colima/_lima/colima/diffdisk",
            "/Users/testuser/Nodes/disk.img",
            "/Users/testuser/Library/Containers/com.example.unknown/Data/x",
        ] {
            assert!(
                !redline_hit_with(p, &homes, |_| true),
                "未登记路径不该被容器红线拦截: {}",
                p
            );
        }
    }

    /// Windows / 空 home 候选：一律按"已安装"保守
    #[test]
    fn empty_home_candidates_are_treated_as_installed() {
        assert!(
            tool_installed(&TOOLS[0], &[]),
            "无 home 候选时不可判定，按已安装"
        );
        // 非登记 home 不命中
        assert!(match_root("/Users/other/Parallels/x", &homes()).is_none());
    }
}
