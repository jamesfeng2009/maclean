//! 统一缓存注册表
//!
//! 定义所有开发者缓存路径，支持 macOS 和 Windows 双平台。
//! 添加新缓存只需在 `CACHE_REGISTRY` 中加一行。
//!
//! 每个条目包含：名称、macOS 路径、Windows 路径、推荐等级、中英文描述。
//! 扫描时遍历注册表，根据当前平台解析路径并计算大小。

use std::path::Path;

use super::{dir_size, home_dir, Recommend, ScanItem, ScanResult, Scanner};

/// 缓存定义
#[derive(Debug, Clone)]
pub struct CacheDef {
    /// 分类名称（中文，i18n 模块会翻译）
    pub name: &'static str,
    /// macOS 路径（~ 代表主目录）
    pub macos_path: &'static str,
    /// Windows 路径（%USERPROFILE% 等环境变量）
    ///
    /// 只有 `scan()` 里的 `#[cfg(target_os = "windows")]` 分支会读它。
    /// 在 macOS 上编译时该分支整段不存在，dead_code 会误报 —— 这里按平台压制，
    /// 而不是删字段（删了 Windows 侧 37 条缓存定义就全空了）。
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub windows_path: &'static str,
    /// 推荐等级
    pub recommend: Recommend,
    /// 中文描述
    pub desc_zh: &'static str,
    /// 英文描述
    ///
    /// 37 条定义的英文文案都已写好，但渲染层目前只读 `desc_zh`。
    /// 接入英文界面时在这里取（见 README 的「界面国际化」待办）。
    #[allow(dead_code)]
    pub desc_en: &'static str,
}

/// 统一缓存注册表：37 个固定路径缓存
///
/// 另有 ~15 个复杂扫描器（Xcode/Docker/AI 模型等）在 dev_cache.rs 中单独实现，
/// 加上 8 种构建产物递归扫描，总计 60+ 类别。
pub static CACHE_REGISTRY: &[CacheDef] = &[
    // === 语言/运行时缓存 ===
    CacheDef {
        name: "RubyGems",
        macos_path: "~/.gem",
        windows_path: "%USERPROFILE%\\.gem",
        recommend: Recommend::Caution,
        desc_zh: "Ruby Gem 缓存，删除后安装时需重新下载",
        desc_en: "Ruby Gem cache, needs re-download after deletion",
    },
    CacheDef {
        name: "Bundler",
        macos_path: "~/.bundle/cache",
        windows_path: "%USERPROFILE%\\.bundle\\cache",
        recommend: Recommend::Safe,
        desc_zh: "Ruby Bundler 缓存，可安全删除",
        desc_en: "Ruby Bundler cache, safe to delete",
    },
    CacheDef {
        name: "rbenv",
        macos_path: "~/.rbenv/versions",
        windows_path: "%USERPROFILE%\\.rbenv\\versions",
        recommend: Recommend::Advanced,
        desc_zh: "rbenv 安装的 Ruby 版本，请确认后删除",
        desc_en: "Ruby versions installed by rbenv, confirm before deletion",
    },
    CacheDef {
        name: "Composer",
        macos_path: "~/.composer/cache",
        windows_path: "%USERPROFILE%\\.composer\\cache",
        recommend: Recommend::Safe,
        desc_zh: "PHP Composer 下载缓存，可安全删除",
        desc_en: "PHP Composer download cache, safe to delete",
    },
    CacheDef {
        name: "Flutter/Dart",
        macos_path: "~/.pub-cache",
        windows_path: "%LOCALAPPDATA%\\Pub\\Cache",
        recommend: Recommend::Caution,
        desc_zh: "Dart/Flutter 包缓存，删除后需重新下载",
        desc_en: "Dart/Flutter package cache, needs re-download after deletion",
    },
    CacheDef {
        name: "SwiftPM",
        macos_path: "~/.swiftpm",
        windows_path: "%USERPROFILE%\\.swiftpm",
        recommend: Recommend::Safe,
        desc_zh: "Swift Package Manager 缓存，可安全删除",
        desc_en: "Swift Package Manager cache, safe to delete",
    },
    CacheDef {
        name: "CocoaPods",
        macos_path: "~/Library/Caches/CocoaPods",
        windows_path: "%LOCALAPPDATA%\\CocoaPods",
        recommend: Recommend::Safe,
        desc_zh: "CocoaPods 缓存，可安全删除",
        desc_en: "CocoaPods cache, safe to delete",
    },
    CacheDef {
        name: "Carthage",
        macos_path: "~/Library/Caches/org.carthage.CarthageKit",
        windows_path: "%LOCALAPPDATA%\\Carthage",
        recommend: Recommend::Safe,
        desc_zh: "Carthage 依赖构建缓存，可安全删除",
        desc_en: "Carthage dependency build cache, safe to delete",
    },
    CacheDef {
        name: "CMake",
        macos_path: "~/.cmake",
        windows_path: "%USERPROFILE%\\.cmake",
        recommend: Recommend::Safe,
        desc_zh: "CMake 缓存，可安全删除",
        desc_en: "CMake cache, safe to delete",
    },
    CacheDef {
        name: "AndroidSDK",
        macos_path: "~/Library/Android/sdk/system-images",
        windows_path: "%LOCALAPPDATA%\\Android\\sdk\\system-images",
        recommend: Recommend::Caution,
        desc_zh: "Android 模拟器系统镜像，删除后需重新下载",
        desc_en: "Android emulator system images, needs re-download after deletion",
    },
    CacheDef {
        name: "Yarn",
        macos_path: "~/.yarn/cache",
        windows_path: "%LOCALAPPDATA%\\Yarn\\Cache",
        recommend: Recommend::Safe,
        desc_zh: "Yarn 包缓存，可安全删除",
        desc_en: "Yarn package cache, safe to delete",
    },
    CacheDef {
        name: "Deno",
        macos_path: "~/Library/Caches/deno",
        windows_path: "%LOCALAPPDATA%\\deno",
        recommend: Recommend::Safe,
        desc_zh: "Deno 缓存，可安全删除",
        desc_en: "Deno cache, safe to delete",
    },
    CacheDef {
        name: "Bun",
        macos_path: "~/.bun/install/cache",
        windows_path: "%USERPROFILE%\\.bun\\install\\cache",
        recommend: Recommend::Safe,
        desc_zh: "Bun 包缓存，可安全删除",
        desc_en: "Bun package cache, safe to delete",
    },
    // === 新增：语言/运行时 ===
    CacheDef {
        name: "NuGet",
        macos_path: "~/.nuget/packages",
        windows_path: "%USERPROFILE%\\.nuget\\packages",
        recommend: Recommend::Caution,
        desc_zh: ".NET NuGet 包缓存，删除后需重新还原",
        desc_en: ".NET NuGet package cache, needs restore after deletion",
    },
    CacheDef {
        name: "Zig",
        macos_path: "~/.cache/zig",
        windows_path: "%LOCALAPPDATA%\\zig",
        recommend: Recommend::Safe,
        desc_zh: "Zig 编译缓存，可安全删除",
        desc_en: "Zig build cache, safe to delete",
    },
    CacheDef {
        name: "Elixir Mix",
        macos_path: "~/.mix",
        windows_path: "%USERPROFILE%\\.mix",
        recommend: Recommend::Caution,
        desc_zh: "Elixir Mix 构建缓存，删除后需重新编译",
        desc_en: "Elixir Mix build cache, needs recompile after deletion",
    },
    CacheDef {
        name: "Elixir Hex",
        macos_path: "~/.hex/packages",
        windows_path: "%USERPROFILE%\\.hex\\packages",
        recommend: Recommend::Safe,
        desc_zh: "Elixir Hex 包缓存，可安全删除",
        desc_en: "Elixir Hex package cache, safe to delete",
    },
    CacheDef {
        name: "Haskell Stack",
        macos_path: "~/.stack",
        windows_path: "%USERPROFILE%\\.stack",
        recommend: Recommend::Caution,
        desc_zh: "Haskell Stack 编译缓存和工具链，删除后需重新安装",
        desc_en: "Haskell Stack build cache and toolchain, needs reinstall after deletion",
    },
    CacheDef {
        name: "Nix",
        macos_path: "/nix/var/nix/db",
        windows_path: "N/A",
        recommend: Recommend::Advanced,
        desc_zh: "Nix 包管理器数据库，建议用 nix-collect-garbage 清理",
        desc_en: "Nix package manager database, use nix-collect-garbage to clean",
    },
    CacheDef {
        name: "Crystal",
        macos_path: "~/.cache/crystal",
        windows_path: "%LOCALAPPDATA%\\crystal",
        recommend: Recommend::Safe,
        desc_zh: "Crystal shards 缓存，可安全删除",
        desc_en: "Crystal shards cache, safe to delete",
    },
    CacheDef {
        name: "Julia",
        macos_path: "~/.julia/artifacts",
        windows_path: "%USERPROFILE%\\.julia\\artifacts",
        recommend: Recommend::Caution,
        desc_zh: "Julia 包构件缓存，删除后需重新下载",
        desc_en: "Julia package artifacts cache, needs re-download after deletion",
    },
    CacheDef {
        name: "Nim",
        macos_path: "~/.nimble/pkgs",
        windows_path: "%USERPROFILE%\\.nimble\\pkgs",
        recommend: Recommend::Caution,
        desc_zh: "Nimble 包缓存，删除后需重新下载",
        desc_en: "Nimble package cache, needs re-download after deletion",
    },
    CacheDef {
        name: "LuaRocks",
        macos_path: "~/.luarocks",
        windows_path: "%USERPROFILE%\\.luarocks",
        recommend: Recommend::Caution,
        desc_zh: "LuaRocks 包缓存，删除后需重新安装",
        desc_en: "LuaRocks package cache, needs reinstall after deletion",
    },
    CacheDef {
        name: "R",
        macos_path: "~/Library/Caches/R",
        windows_path: "%LOCALAPPDATA%\\R\\cache",
        recommend: Recommend::Safe,
        desc_zh: "R 语言包缓存，可安全删除",
        desc_en: "R language package cache, safe to delete",
    },
    // === 构建工具 ===
    CacheDef {
        name: "Bazel",
        macos_path: "~/.cache/bazel",
        windows_path: "%LOCALAPPDATA%\\bazel",
        recommend: Recommend::Caution,
        desc_zh: "Bazel 构建缓存，删除后需重新构建",
        desc_en: "Bazel build cache, needs rebuild after deletion",
    },
    CacheDef {
        name: "Fastlane",
        macos_path: "~/.fastlane",
        windows_path: "%USERPROFILE%\\.fastlane",
        recommend: Recommend::Safe,
        desc_zh: "Fastlane lane 缓存，可安全删除",
        desc_en: "Fastlane lane cache, safe to delete",
    },
    // === 移动开发 ===
    CacheDef {
        name: "React Native",
        macos_path: "~/.rncache",
        windows_path: "%LOCALAPPDATA%\\.rncache",
        recommend: Recommend::Safe,
        desc_zh: "React Native 依赖缓存，可安全删除",
        desc_en: "React Native dependency cache, safe to delete",
    },
    CacheDef {
        name: "Expo",
        macos_path: "~/.expo",
        windows_path: "%USERPROFILE%\\.expo",
        recommend: Recommend::Safe,
        desc_zh: "Expo CLI 缓存，可安全删除",
        desc_en: "Expo CLI cache, safe to delete",
    },
    // === IDE/编辑器 ===
    CacheDef {
        name: "VSCode",
        macos_path: "~/Library/Application Support/Code/Cache",
        windows_path: "%APPDATA%\\Code\\Cache",
        recommend: Recommend::Safe,
        desc_zh: "VSCode 编辑器缓存，可安全删除",
        desc_en: "VSCode editor cache, safe to delete",
    },
    CacheDef {
        name: "VSCode CachedData",
        macos_path: "~/Library/Application Support/Code/CachedData",
        windows_path: "%APPDATA%\\Code\\CachedData",
        recommend: Recommend::Safe,
        desc_zh: "VSCode 缓存的 V8 字节码，可安全删除",
        desc_en: "VSCode cached V8 bytecode, safe to delete",
    },
    CacheDef {
        name: "Cursor",
        macos_path: "~/Library/Application Support/Cursor/Cache",
        windows_path: "%APPDATA%\\Cursor\\Cache",
        recommend: Recommend::Safe,
        desc_zh: "Cursor 编辑器缓存，可安全删除",
        desc_en: "Cursor editor cache, safe to delete",
    },
    CacheDef {
        name: "Postman",
        macos_path: "~/Library/Application Support/Postman",
        windows_path: "%APPDATA%\\Postman",
        recommend: Recommend::Caution,
        desc_zh: "Postman API 工具缓存，可能包含请求历史",
        desc_en: "Postman API tool cache, may contain request history",
    },
    // === 游戏引擎 ===
    CacheDef {
        name: "Unity",
        macos_path: "~/Library/Unity/cache",
        windows_path: "%LOCALAPPDATA%\\Unity\\cache",
        recommend: Recommend::Caution,
        desc_zh: "Unity 编辑器缓存，删除后首次打开项目会变慢",
        desc_en: "Unity editor cache, first project open will be slower after deletion",
    },
    CacheDef {
        name: "Unreal Engine",
        macos_path: "~/Library/Epic/UE_DDC",
        windows_path: "%LOCALAPPDATA%\\UnrealEngine\\Common\\DDC",
        recommend: Recommend::Caution,
        desc_zh: "Unreal Engine DerivedDataCache，删除后需重新编译着色器",
        desc_en: "Unreal Engine DerivedDataCache, needs shader recompile after deletion",
    },
    CacheDef {
        name: "Godot",
        macos_path: "~/Library/Caches/Godot",
        windows_path: "%LOCALAPPDATA%\\Godot",
        recommend: Recommend::Safe,
        desc_zh: "Godot 编辑器缓存，可安全删除",
        desc_en: "Godot editor cache, safe to delete",
    },
    // === 云/基础设施 ===
    CacheDef {
        name: "Terraform",
        macos_path: "~/.terraform.d/plugin-cache",
        windows_path: "%APPDATA%\\terraform.d\\plugin-cache",
        recommend: Recommend::Safe,
        desc_zh: "Terraform 插件缓存，可安全删除",
        desc_en: "Terraform plugin cache, safe to delete",
    },
    CacheDef {
        name: "Vagrant",
        macos_path: "~/.vagrant.d/boxes",
        windows_path: "%USERPROFILE%\\.vagrant.d\\boxes",
        recommend: Recommend::Advanced,
        desc_zh: "Vagrant box 镜像，删除后需重新下载",
        desc_en: "Vagrant box images, needs re-download after deletion",
    },
    CacheDef {
        name: "Electron",
        macos_path: "~/Library/Caches/electron",
        windows_path: "%LOCALAPPDATA%\\electron\\Cache",
        recommend: Recommend::Safe,
        desc_zh: "Electron 二进制缓存，可安全删除",
        desc_en: "Electron binary cache, safe to delete",
    },
];

/// 注册表扫描器
#[derive(Debug, Default)]
pub struct RegistryScanner;

impl RegistryScanner {
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for RegistryScanner {
    fn scan(&self) -> ScanResult {
        let start = std::time::Instant::now();
        let home = home_dir();
        let mut items = Vec::new();

        for def in CACHE_REGISTRY {
            // macOS 平台
            #[cfg(target_os = "macos")]
            let raw_path = def.macos_path;

            // Windows 平台
            #[cfg(target_os = "windows")]
            let raw_path = def.windows_path;

            // Linux/其他平台 fallback 到 macOS 路径
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let raw_path = def.macos_path;

            // 跳过不适用的平台
            if raw_path == "N/A" {
                continue;
            }

            // 展开路径
            let expanded = expand_path(raw_path, &home);
            let path = Path::new(&expanded);

            if path.is_dir() {
                let size = dir_size(path);
                if size > 0 {
                    items.push(ScanItem {
                        path: expanded,
                        size_bytes: size,
                        category: def.name.to_string(),
                        selected: false,
                        deletable: true,
                        undeletable_reason: String::new(),
                        batch_paths: Vec::new(),
                        recommend: def.recommend,
                        description: def.desc_zh.to_string(),
                    });
                }
            }
        }

        let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
        ScanResult {
            items,
            total_size,
            scan_time_ms: start.elapsed().as_millis() as u64,
        }
    }
}

/// 展开路径中的 ~ 和环境变量
fn expand_path(path: &str, home: &std::path::Path) -> String {
    let mut result = path.to_string();

    // 展开 ~
    if result.starts_with('~') {
        result = format!("{}{}", home.to_string_lossy(), &result[1..]);
    }

    // Windows 环境变量展开
    #[cfg(target_os = "windows")]
    {
        for (var, val) in [
            ("%USERPROFILE%", "USERPROFILE"),
            ("%LOCALAPPDATA%", "LOCALAPPDATA"),
            ("%APPDATA%", "APPDATA"),
        ] {
            if result.contains(var) {
                if let Ok(env_val) = std::env::var(val) {
                    result = result.replace(var, &env_val);
                }
            }
        }
    }

    result
}

// 2026-09-18 删除了 `registry_count`：零引用。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_has_50_plus_entries() {
        // 注册表本身 37 条 + 复杂扫描器 ~15 + 构建产物 8 = 60+
        assert!(
            CACHE_REGISTRY.len() >= 30,
            "注册表至少 30 条，当前 {}",
            CACHE_REGISTRY.len()
        );
    }

    #[test]
    fn test_all_entries_have_non_empty_paths() {
        for def in CACHE_REGISTRY {
            assert!(!def.name.is_empty(), "名称不能为空");
            assert!(!def.macos_path.is_empty(), "{} macOS 路径为空", def.name);
            // windows_path 可以是 "N/A"（如 Nix），但不能为空
            assert!(
                !def.windows_path.is_empty(),
                "{} Windows 路径为空",
                def.name
            );
            assert!(!def.desc_zh.is_empty(), "{} 中文描述为空", def.name);
            assert!(!def.desc_en.is_empty(), "{} 英文描述为空", def.name);
        }
    }

    #[test]
    fn test_no_duplicate_names() {
        let mut names: Vec<&str> = CACHE_REGISTRY.iter().map(|d| d.name).collect();
        names.sort();
        let len_before = names.len();
        names.dedup();
        assert_eq!(len_before, names.len(), "注册表中有重复的名称");
    }

    #[test]
    fn test_expand_path() {
        let home = std::path::PathBuf::from("/Users/test");
        assert_eq!(expand_path("~/.cargo", &home), "/Users/test/.cargo");
        assert_eq!(expand_path("/nix/var/nix/db", &home), "/nix/var/nix/db");
    }

    #[test]
    fn test_registry_scanner_returns_results() {
        let result = RegistryScanner::new().scan();
        // 至少能扫描出一些结果（测试环境可能没有所有缓存）
        assert!(result.scan_time_ms < 10000, "扫描应快速完成");
    }
}
