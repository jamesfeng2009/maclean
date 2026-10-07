//! 语义化清理项目录（named catalog）
//!
//! 对标 MangoDisk 的 "WeChat application cache / Lark rendering cache /
//! Go build cache" 粒度：每个应用 / 工具链一个**语义命名**清理项，
//! 带风险等级、说明文案与「清理前需关闭应用」提示，而不是只按路径类别
//! 罗列目录名。
//!
//! 两条驱动路径：
//! 1. [`scan_toolchain_rules`]：语言 / 工具链注册表。只登记 dev_cache 未覆盖
//!    的缺口（Bun / Deno / Playwright / CocoaPods / Swift SPM / Composer /
//!    Ruby gem / Bundler / Go build / uv / Yarn），已覆盖的（Cargo、Gradle、
//!    Maven、pip、poetry、pnpm、npm、Homebrew 等）不加，避免跨扫描器重复。
//! 2. [`scan_installed_app_caches`]：**已安装应用通用驱动** —— 枚举
//!    `/Applications` + `~/Applications`，对每个应用定点探测 Caches / Logs /
//!    容器缓存，每个应用一条语义名分类（前端按 category 自动分组）。
//!
//! 安全边界（与 app_cache 其余段一致）：
//! - 容器路径**不深挖**（TCC 撞墙会卡内核）：只对已知应用的 bundle id 定点
//!   探测 `Data/Library/Caches` 与 `Data/Library/Application Support` 一层，
//!   无授权时 read_dir 失败即整体跳过；
//! - 所有路径删除走既有安全闸门（safety + 废纸篓 + 白名单）；
//! - 路径存在才展示、体积达标（[`NAMED_MIN`]）才出项。

use std::path::PathBuf;

use super::app_cache::push_named_cache_item;
use super::{home_dir, Recommend, ScanItem};
use crate::scanner::uninstall::{collect_app_paths, get_bundle_id};

/// 工具链 / 语言注册表条目
struct ToolchainRule {
    category: &'static str,
    recommend: Recommend,
    description: &'static str,
    apps_to_close: &'static [&'static str],
    /// 候选路径（`~` 前缀按 home 展开；也可为绝对路径）
    paths: &'static [&'static str],
}

/// 语言 / 工具链缺口注册表（dev_cache 已覆盖的不在此列）。
static TOOLCHAIN_RULES: &[ToolchainRule] = &[
    ToolchainRule {
        category: "Go构建缓存",
        recommend: Recommend::CacheOnly,
        description: "Go 编译产物缓存，删除后下次构建会重新编译；源码与已下载的模块缓存不受影响。",
        apps_to_close: &[],
        paths: &["~/Library/Caches/go-build"],
    },
    ToolchainRule {
        category: "uv缓存",
        recommend: Recommend::CacheOnly,
        description: "uv（Python 包管理器）的下载与构建缓存，删除后下次安装依赖时重新下载。",
        apps_to_close: &[],
        paths: &["~/Library/Caches/uv"],
    },
    ToolchainRule {
        category: "Yarn缓存",
        recommend: Recommend::CacheOnly,
        description: "Yarn 下载的依赖包缓存，删除后下次安装时重新下载。",
        apps_to_close: &[],
        paths: &["~/Library/Caches/Yarn", "~/Library/Caches/Yarn v6"],
    },
    ToolchainRule {
        category: "Bun缓存",
        recommend: Recommend::CacheOnly,
        description: "Bun 包管理器下载的依赖缓存，删除后下次安装时重新下载。",
        apps_to_close: &[],
        paths: &["~/.bun/install/cache"],
    },
    ToolchainRule {
        category: "Deno缓存",
        recommend: Recommend::CacheOnly,
        description: "Deno 的模块与编译缓存，删除后下次运行脚本时重新下载/编译。",
        apps_to_close: &[],
        paths: &["~/Library/Caches/deno"],
    },
    ToolchainRule {
        category: "Playwright浏览器",
        recommend: Recommend::Caution,
        description: "Playwright / Puppeteer 下载的浏览器运行时（Chromium/Firefox/WebKit 等），删除后需要重新下载（GB 级）。",
        apps_to_close: &[],
        paths: &["~/Library/Caches/ms-playwright"],
    },
    ToolchainRule {
        category: "CocoaPods缓存",
        recommend: Recommend::CacheOnly,
        description: "CocoaPods 的 Specs 与下载缓存，删除后 pod install 时重新下载。",
        apps_to_close: &[],
        paths: &["~/Library/Caches/CocoaPods"],
    },
    ToolchainRule {
        category: "Swift SPM缓存",
        recommend: Recommend::CacheOnly,
        description: "Swift Package Manager 的依赖下载与编译缓存，删除后下次构建重新解析。",
        apps_to_close: &[],
        paths: &["~/Library/Caches/org.swift.swiftpm"],
    },
    ToolchainRule {
        category: "Composer缓存",
        recommend: Recommend::CacheOnly,
        description: "PHP Composer 的包下载缓存，删除后 composer install 时重新下载。",
        apps_to_close: &[],
        paths: &["~/Library/Caches/composer", "~/.composer/cache"],
    },
    ToolchainRule {
        category: "Ruby gem",
        recommend: Recommend::Caution,
        description: "Ruby 全局 gem 安装目录，删除后已安装的 gem 需重新安装；不影响 Gemfile 锁定的项目依赖（bundle 会重建）。",
        apps_to_close: &[],
        paths: &["~/.gem"],
    },
    ToolchainRule {
        category: "Bundler缓存",
        recommend: Recommend::CacheOnly,
        description: "Ruby Bundler 的 gem 下载缓存，删除后 bundle install 时重新下载。",
        apps_to_close: &[],
        paths: &["~/Library/Caches/Bundler"],
    },
];

/// 扫描工具链注册表：每个规则命中（路径存在且体积达标）即产出语义化清理项。
pub fn scan_toolchain_rules() -> Vec<ScanItem> {
    let home = home_dir();
    let mut items = Vec::new();
    for rule in TOOLCHAIN_RULES {
        let mut close_hint = String::new();
        if !rule.apps_to_close.is_empty() {
            close_hint = format!(" 清理前请关闭：{}。", rule.apps_to_close.join("、"));
        }
        let description = format!("{}{}", rule.description, close_hint);
        for p in rule.paths {
            let full = if let Some(rest) = p.strip_prefix("~/") {
                home.join(rest)
            } else {
                PathBuf::from(p)
            };
            push_named_cache_item(
                &mut items,
                &full,
                rule.category,
                &description,
                rule.recommend,
            );
        }
    }
    items
}

/// 已安装应用通用驱动：枚举全部 `.app`，对每个应用定点探测常见缓存落点，
/// 每个应用一个语义名分类（Caches / Logs / 容器缓存），路径存在且达标才出项。
///
/// 探测顺序（每步都定点、数量有界）：
/// 1. `~/Library/Caches/<显示名>`（最常见）；
/// 2. `~/Library/Caches/<bundle_id>`（部分应用按 bundle 名建缓存目录）；
/// 3. `~/Library/Containers/<bundle_id>/Data/Library/Caches` 一层缓存目录；
/// 4. `~/Library/Containers/<bundle_id>/Data/Library/Application Support`
///    一层的缓存目录名（DawnCache/GPUCache/Cache 等）；
/// 5. `~/Library/Logs/<显示名>`（日志）。
///
/// 容器路径不深挖：无「完全磁盘访问」时 read_dir 失败即跳过，不卡段。
pub fn scan_installed_app_caches() -> Vec<ScanItem> {
    let home = home_dir();
    let mut items = Vec::new();
    let apps = collect_app_paths();
    if apps.is_empty() {
        return items; // 目录不可读：保守返回空，不误扫
    }

    for app_path in apps {
        let Some(name) = app_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.trim_end_matches(".app").to_string())
        else {
            continue;
        };
        if name.is_empty() || name.eq_ignore_ascii_case("Safari") {
            continue; // 系统级基础应用无独立缓存布局，跳过省 IO
        }
        let bundle = get_bundle_id(&app_path);

        // 1) Caches/<显示名>
        push_app_item(
            &mut items,
            &home.join("Library/Caches").join(&name),
            &format!("{}缓存", name),
            &format!(
                "{} 的缓存数据（Library/Caches），删除后应用会自动重建；配置与用户数据不受影响。",
                name
            ),
            Recommend::CacheOnly,
        );
        // 2) Caches/<bundle_id>
        if let Some(b) = &bundle {
            push_app_item(
                &mut items,
                &home.join("Library/Caches").join(b),
                &format!("{}缓存", name),
                &format!(
                    "{} 的缓存数据（按 bundle id 建目录），删除后应用会自动重建。",
                    name
                ),
                Recommend::CacheOnly,
            );
        }
        // 5) Logs/<显示名>
        push_app_item(
            &mut items,
            &home.join("Library/Logs").join(&name),
            &format!("{}日志", name),
            &format!(
                "{} 的诊断日志与临时数据，删除后自动重建；不影响应用配置。",
                name
            ),
            Recommend::CacheOnly,
        );

        // 3) 4) 容器缓存：只对已解析到 bundle id 的应用定点探测，不深挖
        let Some(b) = bundle else { continue };
        let container = home.join("Library/Containers").join(&b).join("Data");
        scan_container_cache_layer(&mut items, &container, &name);
    }

    items
}

/// 容器内定点缓存探测：`Data/Library/Caches` 整目录 + `Data/Library/Application
/// Support` 一层缓存目录名匹配。失败（TCC 未授权 / 不存在）即静默返回。
fn scan_container_cache_layer(items: &mut Vec<ScanItem>, data_root: &std::path::Path, app_name: &str) {
    // Data/Library/Caches（整目录，含子缓存）
    let caches = data_root.join("Library/Caches");
    push_app_item(
        items,
        &caches,
        &format!("{}容器缓存", app_name),
        &format!(
            "{} 沙盒容器内的缓存数据，删除后应用会自动重建；聊天/文档等用户数据不受影响。清理前请退出{}。",
            app_name, app_name
        ),
        Recommend::CacheOnly,
    );
    // Data/Library/Application Support/<app>/ 下的一层缓存目录
    let as_root = data_root.join("Library/Application Support");
    if let Ok(rd) = std::fs::read_dir(&as_root) {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let dir_name = e.file_name().to_string_lossy().to_string();
            if !super::app_cache::is_cache_dir_name(&dir_name) {
                continue;
            }
            let description = format!(
                "{} 沙盒容器内的缓存目录（{}），删除后应用会自动重建；用户数据不受影响。清理前请退出{}。",
                app_name, dir_name, app_name
            );
            push_named_cache_item(
                items,
                &p,
                &format!("{}容器缓存", app_name),
                &description,
                Recommend::CacheOnly,
            );
        }
    }
}

/// 定点推送一个应用缓存项（目录存在且体积达标才进列表）。
fn push_app_item(
    items: &mut Vec<ScanItem>,
    dir: &std::path::Path,
    category: &str,
    description: &str,
    recommend: Recommend,
) {
    push_named_cache_item(items, dir, category, description, recommend);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolchain_rules_have_valid_layout() {
        // 注册表条目必须齐全、路径非空、描述非空
        assert!(!TOOLCHAIN_RULES.is_empty());
        for rule in TOOLCHAIN_RULES {
            assert!(!rule.category.is_empty());
            assert!(!rule.paths.is_empty());
            assert!(!rule.description.is_empty());
            for p in rule.paths {
                assert!(p.starts_with("~/") || p.starts_with('/'));
            }
        }
    }

    #[test]
    fn toolchain_rules_do_not_duplicate_dev_cache_coverage() {
        // 已由 dev_cache 覆盖的路径不应出现在注册表（避免跨扫描器重复显示）：
        // Cargo registry、go/pkg/mod、Gradle、Maven、poetry、pnpm/npm、Homebrew、pip
        let joined: Vec<String> = TOOLCHAIN_RULES
            .iter()
            .flat_map(|r| r.paths.iter().map(|p| p.to_string()))
            .collect();
        for dup in [
            ".cargo/registry",
            "go/pkg/mod",
            ".gradle/caches",
            ".m2/repository",
            "Library/Caches/pypoetry",
            "Library/Caches/Homebrew",
            "Library/Caches/pip",
        ] {
            assert!(
                !joined.iter().any(|p| p.contains(dup)),
                "注册表不应重复 dev_cache 已覆盖路径: {}",
                dup
            );
        }
    }

    #[test]
    fn installed_app_driver_ignores_missing_paths() {
        // 定点探测：不存在的目录不产生项（不触碰文件系统）
        let mut items = Vec::new();
        let missing = std::env::temp_dir().join("maclean_app_driver_missing_xyz");
        push_app_item(
            &mut items,
            &missing,
            "测试缓存",
            "test",
            Recommend::CacheOnly,
        );
        assert!(items.is_empty());
    }
}
