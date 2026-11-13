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
//! - **L1 本表**：已知工具，逐条登记数据根，**装了 / 卸了两套行为**：
//!   在用 → 红线只读（删文件会破坏镜像库或虚拟机磁盘），引导官方通道；
//!   不在用 → 孤儿残留可清理（Caution，删了要重新下载，代价写进文案）。
//! - **L2 沙盒容器**：`~/Library/Containers/<bundle-id>` 归属天然明确，
//!   由 `scanner/app_cache.rs` 用 `installed_bundle_id_set()` 判孤儿。
//! - **L3 未登记目录**：不猜、不自动判定可删，只给「疑似虚拟机数据」提示，
//!   删不删由用户自己决定。
//!
//! 两类工具两种探测：
//! - GUI（Docker Desktop 等）：`/Applications`、`~/Applications` 下有 .app；
//! - CLI（Colima / Lima / Podman）：没有 App 可查，用三路证据——可执行文件
//!   是否在、是否有进程在跑、数据目录里有没有活跃 socket / 存活 pid。
//!   **任一成立即按"在用"处理**，一条证据都拿不到才算孤儿。
//!
//! 新增一个工具 = 这里加一条表项 + 一条参数化测试（测试自动覆盖全表）。

use std::path::{Path, PathBuf};

use crate::scanner::Recommend;

/// 工具「是否仍在使用」的探测方式
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Probe {
    /// GUI App：登记 .app 名
    App(&'static str),
    /// CLI 工具：登记可执行文件名与进程名
    Cli(CliProbe),
}

/// CLI 工具的三路证据（可执行文件 / 进程 / 运行时文件）
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CliProbe {
    pub binaries: &'static [&'static str],
    pub processes: &'static [&'static str],
}

/// 某个工具的一处数据根（home 相对路径）
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ContainerRoot {
    /// home 相对路径；绝对路径不登记（避免把系统目录写进用户数据表）
    pub path: &'static str,
    /// 扫描结果里的分类名（UI 分组用）
    pub category: &'static str,
    /// 工具仍在用时的说明
    pub label: &'static str,
    /// 仍在用时的不可删原因（引导官方通道）
    pub hint: &'static str,
    /// 工具已不在时的说明
    pub orphan_label: &'static str,
    /// 仍在用时的风险等级
    pub installed_recommend: Recommend,
}

/// 一个容器 / 虚拟机工具
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ContainerTool {
    pub id: &'static str,
    pub probe: Probe,
    pub display: &'static str,
    pub roots: &'static [ContainerRoot],
}

const ADVANCED: Recommend = Recommend::Advanced;
const CAUTION: Recommend = Recommend::Caution;

pub(crate) const TOOLS: &[ContainerTool] = &[
    ContainerTool {
        id: "docker-desktop",
        probe: Probe::App("Docker.app"),
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
        probe: Probe::App("OrbStack.app"),
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
        probe: Probe::App("Parallels Desktop.app"),
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
        probe: Probe::App("VMware Fusion.app"),
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
        probe: Probe::App("VirtualBox.app"),
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
        probe: Probe::App("UTM.app"),
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
        probe: Probe::App("Rancher Desktop.app"),
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
    // ---- CLI 工具：没有 App 可查，走三路证据 ----
    ContainerTool {
        id: "colima",
        probe: Probe::Cli(CliProbe {
            binaries: &["colima"],
            processes: &["colima", "qemu-system-aarch64", "qemu-system-x86_64"],
        }),
        display: "Colima",
        roots: &[ContainerRoot {
            path: ".colima",
            category: "Colima虚拟机",
            label: "Colima 虚拟机数据（只展示，不直接删除；清理请用 colima delete）",
            hint: "虚拟机磁盘文件，直接删会损坏虚拟机；请用 colima delete --all 或 colima prune",
            orphan_label: "未检测到 Colima，残留虚拟机数据（可删除释放空间）",
            installed_recommend: ADVANCED,
        }],
    },
    ContainerTool {
        id: "lima",
        probe: Probe::Cli(CliProbe {
            binaries: &["limactl", "lima"],
            processes: &["limactl", "qemu-system-aarch64", "qemu-system-x86_64"],
        }),
        display: "Lima",
        roots: &[ContainerRoot {
            path: ".lima",
            category: "Lima虚拟机",
            label: "Lima 虚拟机数据（只展示，不直接删除；清理请用 limactl delete）",
            hint: "虚拟机磁盘文件，直接删会损坏虚拟机；请用 limactl delete <实例> 或 lima prune",
            orphan_label: "未检测到 Lima，残留虚拟机数据（可删除释放空间）",
            installed_recommend: ADVANCED,
        }],
    },
    ContainerTool {
        id: "podman",
        probe: Probe::Cli(CliProbe {
            binaries: &["podman"],
            processes: &["podman", "vfkit", "gvproxy"],
        }),
        display: "Podman",
        roots: &[ContainerRoot {
            path: ".local/share/containers",
            category: "Podman容器",
            label: "Podman 容器/镜像数据（只展示，不直接删除；清理请用 podman system prune）",
            hint: "镜像库与容器层，直接删会损坏 Podman 状态；请用 podman machine rm / podman system prune -a",
            orphan_label: "未检测到 Podman，残留容器数据（可删除释放空间）",
            installed_recommend: ADVANCED,
        }],
    },
];

/// 工具是否仍在用（App 是否安装 / CLI 是否有任何在用证据）
///
/// **读不到任何 home 候选时按"在用"返回**：保守方向永远是保护，
/// 一次异常不能把仍在使用的虚拟机磁盘判成孤儿。
pub(crate) fn tool_installed(tool: &ContainerTool, homes: &[PathBuf]) -> bool {
    match &tool.probe {
        Probe::App(app) => app_installed(app, homes),
        Probe::Cli(probe) => {
            // home 拿不到 → 无法定位数据目录，也无法判定，按在用
            let Some(home) = homes.first() else {
                return true;
            };
            cli_in_use(probe, tool, home, homes)
        }
    }
}

fn app_installed(app: &str, homes: &[PathBuf]) -> bool {
    if Path::new("/Applications").join(app).is_dir() {
        return true;
    }
    if homes.is_empty() {
        return true;
    }
    homes
        .iter()
        .any(|h| h.join("Applications").join(app).is_dir())
}

/// CLI 工具三路证据：可执行文件在、进程在跑、数据目录里有活跃 socket /
/// 存活 pid —— 任一成立即视为在用。
fn cli_in_use(probe: &CliProbe, tool: &ContainerTool, home: &Path, homes: &[PathBuf]) -> bool {
    if binaries_exist(probe.binaries, &bin_dirs(homes)) {
        return true;
    }
    if probe.processes.iter().any(|p| process_running(p)) {
        return true;
    }
    tool.roots
        .iter()
        .any(|r| runtime_evidence(&home.join(r.path)))
}

/// 可执行文件候选目录：PATH + 常见安装前缀 + 各 home 的 ~/.local/bin
fn bin_dirs(homes: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for fixed in ["/opt/homebrew/bin", "/usr/local/bin"] {
        dirs.push(PathBuf::from(fixed));
    }
    for h in homes {
        dirs.push(h.join(".local/bin"));
    }
    dirs
}

pub(crate) fn binaries_exist(binaries: &[&str], dirs: &[PathBuf]) -> bool {
    binaries
        .iter()
        .any(|b| dirs.iter().any(|d| d.join(b).is_file()))
}

/// 进程探测：`pgrep -x <name>`，只关心"有没有"
#[cfg(unix)]
fn process_running(name: &str) -> bool {
    std::process::Command::new("pgrep")
        .arg("-x")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn process_running(_name: &str) -> bool {
    false
}

/// 数据目录里的运行时证据：`*.sock` 或**存活的** `*.pid`
///
/// 只下探两层（`~/.lima/<实例>/serial.sock` 这一级够了），不做全树遍历。
/// 陈旧 pid 文件不能算在用——否则一次异常关机就永久锁死清理。
fn runtime_evidence(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".sock") {
            return true;
        }
        if name.ends_with(".pid") && pid_alive_in(&entry.path()) {
            return true;
        }
        if entry.path().is_dir() && runtime_evidence_one_level(&entry.path()) {
            return true;
        }
    }
    false
}

fn runtime_evidence_one_level(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".sock") {
            return true;
        }
        if name.ends_with(".pid") && pid_alive_in(&entry.path()) {
            return true;
        }
    }
    false
}

fn pid_alive_in(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<i32>() else {
        return false;
    };
    pid_alive(pid)
}

#[cfg(unix)]
fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill(pid, 0) 只探测进程是否存在，不投递任何信号。
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(not(unix))]
fn pid_alive(_pid: i32) -> bool {
    false
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

/// 路径是否落在「仍在使用中的容器 / 虚拟机数据」红线上
///
/// 与删除端同源：判断与扫描期 `precheck_deletability` 一致，
/// 避免出现"扫得出、删不掉"或"删得掉、其实不该删"。
pub(crate) fn redline_hit(path: &str, homes: &[PathBuf]) -> bool {
    redline_hit_with(path, homes, |tool| tool_installed(tool, homes))
}

/// 纯判定（在用状态由调用方注入，便于单测覆盖两种分支而不依赖本机装了什么）
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

/// L3：未登记的大文件 / 目录里的「疑似虚拟机数据」提示
///
/// 命中的路径**不进红线、不进容器分类**，仍走大文件出口（删不删用户自己定），
/// 只把风险等级抬到 Caution 并加一句说明 —— 不猜归属，也不替用户判死。
pub(crate) fn vm_like_hint(path: &str) -> Option<&'static str> {
    const MARKERS: &[&str] = &[
        "/Nodes/",
        ".pvm/",
        ".vmwarevm/",
        ".vbox/",
        "/VirtualBox VMs/",
        "disk.img",
        "diffdisk",
        "/cidata",
        ".qcow2",
    ];
    MARKERS
        .iter()
        .find(|m| path.contains(*m))
        .map(|_| "疑似虚拟机/容器数据，若对应软件仍在使用，删除会损坏其磁盘；确认不再使用再删")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn homes() -> Vec<PathBuf> {
        vec![PathBuf::from("/Users/testuser")]
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("maclean_ct_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// 全表参数化：在用时必拦、不在用必放行，任何新增工具都会被这条覆盖。
    #[test]
    fn every_registered_root_is_redline_only_while_in_use() {
        let homes = homes();
        for tool in TOOLS {
            for root in tool.roots {
                let abs = format!("/Users/testuser/{}", root.path);
                assert!(
                    redline_hit_with(&abs, &homes, |_| true),
                    "{} 在用时必须拦截: {}",
                    tool.id,
                    abs
                );
                assert!(
                    !redline_hit_with(&abs, &homes, |_| false),
                    "{} 不在用必须放行: {}",
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
                let in_use = root_policy(root, true);
                assert!(!in_use.deletable, "{} 在用时不可删", tool.id);
                assert!(!in_use.reason.is_empty(), "{} 在用时必须给原因", tool.id);

                let orphan = root_policy(root, false);
                assert!(orphan.deletable, "{} 不在用时应可删", tool.id);
                assert!(orphan.reason.is_empty(), "{} 可删时不该带原因", tool.id);
                assert!(
                    orphan.description.contains("已卸载")
                        || orphan.description.contains("未检测到"),
                    "{} 孤儿文案必须说明工具已不在: {}",
                    tool.id,
                    orphan.description
                );
                assert_eq!(
                    orphan.recommend,
                    Recommend::Caution,
                    "{} 孤儿不能给 Safe",
                    tool.id
                );
            }
        }
    }

    /// CLI 工具：二进制存在即视为在用
    #[test]
    fn cli_binary_presence_means_in_use() {
        let dir = tmpdir("bin");
        let bin = dir.join("colima");
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        assert!(binaries_exist(&["colima"], std::slice::from_ref(&dir)));
        assert!(!binaries_exist(&["limactl"], std::slice::from_ref(&dir)));
        assert!(!binaries_exist(&["colima"], &[tmpdir("empty")]));
    }

    /// CLI 工具：活跃 socket 视为在用；陈旧 pid / 死 pid 不算
    #[test]
    fn cli_runtime_evidence_only_counts_live_processes() {
        let dir = tmpdir("runtime");
        std::fs::create_dir_all(dir.join("instance")).unwrap();
        assert!(!runtime_evidence(&dir), "空目录不应有运行时证据");

        std::fs::write(dir.join("dead.pid"), b"999999").unwrap();
        assert!(!runtime_evidence(&dir), "死 pid 不算在用");

        // 存活 pid 依赖 kill(pid, 0)：只有 unix 有这条路，Windows 上
        // pid_alive 恒 false（保守方向由 socket 兜住）。
        #[cfg(unix)]
        {
            std::fs::write(
                dir.join("instance").join("live.pid"),
                std::process::id().to_string(),
            )
            .unwrap();
            assert!(runtime_evidence(&dir), "存活 pid 算在用（并验证下探一层）");
        }

        std::fs::write(dir.join("serial.sock"), b"").unwrap();
        let _ = std::fs::remove_dir_all(dir.join("instance"));
        assert!(runtime_evidence(&dir), "socket 算在用");
    }

    /// 进程探测只做"有没有"，不存在的名字必须返回 false
    #[test]
    fn process_probe_is_negative_for_unknown_names() {
        assert!(!process_running("maclean_no_such_process_zzz"));
    }

    /// 未登记目录绝不进红线（L3 兜底：不认识就不动手）
    #[test]
    fn unregistered_paths_are_never_redline() {
        let homes = homes();
        for p in [
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

    /// L3 提示：命中常见虚拟机文件名 / 目录名，但不改删除资格
    #[test]
    fn vm_like_hint_marks_unregistered_vm_data() {
        let hint = vm_like_hint("/Users/u/Nodes/disk.img").expect("应命中");
        assert!(hint.contains("疑似虚拟机"));
        assert!(vm_like_hint("/Users/u/Documents/Windows.pvm/disk.img").is_some());
        assert!(vm_like_hint("/Users/u/Movies/a.mp4").is_none());
    }

    /// 空 home 候选：一律按"在用"保守
    #[test]
    fn empty_home_candidates_are_treated_as_in_use() {
        assert!(
            tool_installed(&TOOLS[0], &[]),
            "无 home 候选时不可判定，按在用"
        );
        assert!(match_root("/Users/other/Parallels/x", &homes()).is_none());
    }

    /// 登记表必须覆盖本轮约定的 CLI 工具
    #[test]
    fn cli_tools_are_registered() {
        for id in ["colima", "lima", "podman"] {
            let tool = TOOLS.iter().find(|t| t.id == id).expect("CLI 工具必须登记");
            assert!(
                matches!(tool.probe, Probe::Cli(_)),
                "{} 必须走 CLI 三路证据",
                id
            );
            assert!(!tool.roots.is_empty(), "{} 必须登记数据根", id);
        }
    }
}
