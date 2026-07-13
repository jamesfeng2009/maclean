//! 扫描结果多语言翻译模块
//!
//! 在 UI 渲染时将 ScanItem 的 category 和 description 从中文翻译为英文。
//! 扫描器代码无需修改，仍生成中文，翻译在渲染层完成。
//!
//! 对于含动态参数的文本（如 "项目 X 的编译缓存"），使用模式匹配提取
//! 静态部分进行翻译，动态参数保持原样。

use crate::scanner::Recommend;

/// 翻译推荐等级
pub fn translate_recommend(rec: &Recommend, lang_en: bool) -> &'static str {
    if lang_en {
        match rec {
            Recommend::Safe => "Safe",
            Recommend::Caution => "Caution",
            Recommend::Advanced => "Advanced",
        }
    } else {
        match rec {
            Recommend::Safe => "推荐",
            Recommend::Caution => "谨慎",
            Recommend::Advanced => "需确认",
        }
    }
}

/// 翻译分类名
///
/// 对于动态分类（如 "Xcode编译-MyProject"），按 "-" 分割，
/// 翻译前缀部分，保留动态后缀。
pub fn translate_category(cat: &str, lang_en: bool) -> String {
    if !lang_en {
        return cat.to_string();
    }

    // 静态分类直接查表
    if let Some(en) = static_category_en(cat) {
        return en.to_string();
    }

    // 动态分类：按 "-" 分割，翻译前缀
    if let Some(idx) = cat.find('-') {
        let prefix = &cat[..idx];
        let suffix = &cat[idx + 1..];
        if let Some(en_prefix) = static_category_en(prefix) {
            return format!("{}-{}", en_prefix, suffix);
        }
    }

    // App 缓存/数据："{app} 缓存" / "{app} 数据" / "{app} 的 {dir} 目录"
    if let Some(app_name) = cat.strip_suffix(" 缓存") {
        return format!("{} Cache", app_name);
    }
    if let Some(app_name) = cat.strip_suffix(" 数据") {
        return format!("{} Data", app_name);
    }
    if let Some(prefix) = cat.strip_suffix(" 的缓存，删除后自动重建") {
        return format!("{} Cache", prefix);
    }
    if let Some(prefix) = cat.strip_suffix(" 目录，删除后自动重建") {
        // "{app} 的 {dir} 目录，删除后自动重建"
        if let Some(idx) = prefix.rfind(" 的 ") {
            let app_name = &prefix[..idx];
            let dir_name = &prefix[idx + 3..];
            return format!("{}'s {} directory", app_name, dir_name);
        }
    }

    // 浏览器缓存："{browser} ({profile}) 的 {dir} 缓存"
    if let Some(prefix) = cat.strip_suffix(" 缓存，删除后浏览器会自动重建") {
        if let Some(start) = prefix.find(" (") {
            if let Some(end) = prefix.find(") 的 ") {
                let browser = &prefix[..start];
                let dir = &prefix[end + 4..];
                return format!("{} ({}) {} cache", browser, &prefix[start + 2..end], dir);
            }
        }
    }
    // Firefox 缓存："Firefox ({profile}) 的 {dir} 缓存"
    if let Some(prefix) = cat.strip_suffix(" 缓存，可安全清理") {
        if let Some(start) = prefix.find(" (") {
            if let Some(end) = prefix.find(") 的 ") {
                let browser = &prefix[..start];
                let dir = &prefix[end + 4..];
                return format!("{} ({}) {} cache", browser, &prefix[start + 2..end], dir);
            }
        }
    }

    // 未知分类，原样返回
    cat.to_string()
}

/// 静态分类名中英文映射
fn static_category_en(cat: &str) -> Option<&'static str> {
    match cat {
        "Rust编译" => Some("Rust Build"),
        "Xcode编译" => Some("Xcode Build"),
        "iOS设备" => Some("iOS Device"),
        "Xcode归档" => Some("Xcode Archive"),
        "模拟器镜像" => Some("Simulator Image"),
        "模拟器缓存" => Some("Simulator Cache"),
        "模拟器Cryptex" => Some("Simulator Cryptex"),
        "Xcode文档缓存" => Some("Xcode Doc Cache"),
        "Xcode设备日志" => Some("Xcode Device Logs"),
        "Xcode离线文档" => Some("Xcode Offline Docs"),
        "watchOS设备" => Some("watchOS Device"),
        "Xcode连接日志" => Some("Xcode Connect Logs"),
        "Node依赖" => Some("Node Modules"),
        "pnpm缓存" => Some("pnpm Cache"),
        "npm缓存" => Some("npm Cache"),
        "Go模块" => Some("Go Modules"),
        "Homebrew缓存" => Some("Homebrew Cache"),
        "Homebrew下载" => Some("Homebrew Downloads"),
        "Homebrew旧版" => Some("Homebrew Old Versions"),
        "pip缓存" => Some("pip Cache"),
        "IDE旧版" => Some("IDE Old Version"),
        "IDE缓存" => Some("IDE Cache"),
        "Gradle缓存" => Some("Gradle Cache"),
        "Gradle版本" => Some("Gradle Wrapper"),
        "Maven仓库" => Some("Maven Repo"),
        "Java编译" => Some("Java Build"),
        "Conda缓存" => Some("Conda Cache"),
        "Poetry缓存" => Some("Poetry Cache"),
        "Python缓存" => Some("Python Cache"),
        "RubyGems" => Some("RubyGems"),
        "Bundler" => Some("Bundler"),
        "rbenv" => Some("rbenv"),
        "Composer" => Some("Composer"),
        "Flutter/Dart" => Some("Flutter/Dart"),
        "SwiftPM" => Some("SwiftPM"),
        "CocoaPods" => Some("CocoaPods"),
        "CMake" => Some("CMake"),
        "AndroidSDK" => Some("Android SDK"),
        "Yarn" => Some("Yarn"),
        "Deno" => Some("Deno"),
        "Bun" => Some("Bun"),
        "构建产物" => Some("Build Artifacts"),
        "Next.js" => Some("Next.js"),
        "Nuxt.js" => Some("Nuxt.js"),
        "Turborepo" => Some("Turborepo"),
        "SvelteKit" => Some("SvelteKit"),
        "Astro" => Some("Astro"),
        "Remix" => Some("Remix"),
        "Gradle项目" => Some("Gradle Project"),
        "安装包" => Some("Installer"),
        "K8s缓存" => Some("K8s Cache"),
        "K8sHTTP缓存" => Some("K8s HTTP Cache"),
        "Helm缓存" => Some("Helm Cache"),
        "Helm插件缓存" => Some("Helm Plugin Cache"),
        "Docker清理" => Some("Docker Prune"),
        "Docker构建缓存" => Some("Docker Build Cache"),
        "Docker虚拟机" => Some("Docker VM"),
        "Docker缓存" => Some("Docker Cache"),
        "AI模型-HF" => Some("AI Model-HF"),
        "AI缓存-HF" => Some("AI Cache-HF"),
        "Ollama模型" => Some("Ollama Models"),
        "PyTorch缓存" => Some("PyTorch Cache"),
        "llama缓存" => Some("llama Cache"),
        "Monorepo依赖" => Some("Monorepo Deps"),
        "孤儿服务" => Some("Orphaned Service"),
        "DS_Store" => Some("DS_Store"),
        "应用组缓存" => Some("App Group Cache"),
        "系统日志" => Some("System Logs"),
        "浏览器缓存" => Some("Browser Cache"),
        "应用数据" => Some("App Data"),
        "废纸篓残留" => Some("Trash Residual"),
        "下载残留" => Some("Download Residual"),
        "App残留" => Some("App Residual"),
        "App残留缓存" => Some("App Residual Cache"),
        "App残留配置" => Some("App Residual Prefs"),
        "应用名称" => Some("Application"),
        "系统优化" => Some("System Optimize"),
        "APFS快照" => Some("APFS Snapshot"),
        "模拟器运行时" => Some("Simulator Runtime"),
        "目录" => Some("Directory"),
        "文件" => Some("File"),
        "开发者缓存" => Some("Dev Cache"),
        "应用缓存" => Some("App Cache"),
        "应用卸载" => Some("App Uninstall"),
        "磁盘分析" => Some("Disk Analyzer"),
        "系统缓存" => Some("System Cache"),
        // 带 {} 的模板——返回 None，由调用方处理
        _ => None,
    }
}

/// 翻译描述文本
///
/// 对于含动态参数的描述，使用模式匹配提取静态部分进行翻译。
/// 未知描述原样返回。
pub fn translate_description(desc: &str, lang_en: bool) -> String {
    if !lang_en {
        return desc.to_string();
    }

    // 尝试精确匹配
    if let Some(en) = exact_description_en(desc) {
        return en.to_string();
    }

    // 尝试模式匹配（含动态参数的描述）
    if let Some(en) = pattern_description_en(desc) {
        return en;
    }

    // 未知描述，原样返回
    desc.to_string()
}

/// 精确匹配的描述翻译
fn exact_description_en(desc: &str) -> Option<&'static str> {
    match desc {
        "Rust 编译产物，cargo build 会自动重新生成" => Some("Rust build artifacts, cargo build will regenerate"),
        "Cargo 包下载缓存，删除后编译时需重新下载" => Some("Cargo package download cache, needs re-download after deletion"),
        "Xcode 在线文档缓存，可安全删除" => Some("Xcode online documentation cache, safe to delete"),
        "设备日志和崩溃报告，可安全删除" => Some("Device logs and crash reports, safe to delete"),
        "Xcode 旧版离线文档，可能不再需要" => Some("Xcode legacy offline docs, may no longer be needed"),
        "watchOS 设备调试符号，连接手表时会重新生成" => Some("watchOS device debug symbols, regenerated when watch is connected"),
        "Apple Connect 日志，可安全删除" => Some("Apple Connect logs, safe to delete"),
        "Node.js 依赖包，npm install 可恢复" => Some("Node.js dependencies, npm install can restore"),
        "pnpm 全局存储，删除后需重新安装依赖" => Some("pnpm global store, needs reinstall after deletion"),
        "npm 下载缓存，可安全删除" => Some("npm download cache, safe to delete"),
        "Go 模块缓存，编译时需重新下载" => Some("Go module cache, needs re-download when compiling"),
        "Homebrew 下载缓存，可安全删除" => Some("Homebrew download cache, safe to delete"),
        "Homebrew 已下载的安装包，可安全删除" => Some("Homebrew downloaded packages, safe to delete"),
        "pip 下载缓存，可安全删除" => Some("pip download cache, safe to delete"),
        "JetBrains IDE 旧版本配置，已保留最新版" => Some("JetBrains IDE old version config, latest version retained"),
        "JetBrains IDE 缓存，重启 IDE 会自动重建" => Some("JetBrains IDE cache, rebuilt on IDE restart"),
        "Gradle 构建缓存，删除后编译时需重新下载依赖" => Some("Gradle build cache, needs re-download after deletion"),
        "Gradle Wrapper 下载的版本，可安全删除会自动重新下载" => Some("Gradle Wrapper downloaded versions, safe to delete, auto re-downloads"),
        "Maven 本地依赖仓库，删除后编译时需重新下载" => Some("Maven local dependency repo, needs re-download after deletion"),
        "Java/Gradle 项目编译产物，gradle build 会自动重新生成" => Some("Java/Gradle build artifacts, gradle build will regenerate"),
        "Conda 包缓存，删除后安装时需重新下载" => Some("Conda package cache, needs re-download after deletion"),
        "Poetry 依赖缓存，可安全删除" => Some("Poetry dependency cache, safe to delete"),
        "Python 字节码缓存，运行时自动重建" => Some("Python bytecode cache, auto-rebuilt at runtime"),
        "Ruby Gem 缓存，删除后安装时需重新下载" => Some("Ruby Gem cache, needs re-download after deletion"),
        "Ruby Bundler 缓存，可安全删除" => Some("Ruby Bundler cache, safe to delete"),
        "rbenv 安装的 Ruby 版本，请确认后删除" => Some("rbenv installed Ruby versions, confirm before deleting"),
        "PHP Composer 下载缓存，可安全删除" => Some("PHP Composer download cache, safe to delete"),
        "Dart/Flutter 包缓存，删除后需重新下载" => Some("Dart/Flutter package cache, needs re-download after deletion"),
        "Swift Package Manager 缓存，可安全删除" => Some("Swift Package Manager cache, safe to delete"),
        "CocoaPods 缓存，可安全删除" => Some("CocoaPods cache, safe to delete"),
        "CMake 缓存，可安全删除" => Some("CMake cache, safe to delete"),
        "Android 模拟器系统镜像，删除后需重新下载" => Some("Android emulator system images, needs re-download after deletion"),
        "Yarn 包缓存，可安全删除" => Some("Yarn package cache, safe to delete"),
        "Deno 缓存，可安全删除" => Some("Deno cache, safe to delete"),
        "Bun 包缓存，可安全删除" => Some("Bun package cache, safe to delete"),
        "前端构建产物，npm run build 会重新生成" => Some("Frontend build artifacts, npm run build will regenerate"),
        "Next.js 构建缓存，可安全删除" => Some("Next.js build cache, safe to delete"),
        "Nuxt.js 构建缓存，可安全删除" => Some("Nuxt.js build cache, safe to delete"),
        "Turborepo 缓存，可安全删除" => Some("Turborepo cache, safe to delete"),
        "SvelteKit 构建缓存，可安全删除" => Some("SvelteKit build cache, safe to delete"),
        "Astro 构建缓存，可安全删除" => Some("Astro build cache, safe to delete"),
        "Remix 构建缓存，可安全删除" => Some("Remix build cache, safe to delete"),
        "Gradle 项目本地缓存，可安全删除" => Some("Gradle project local cache, safe to delete"),
        "kubectl 缓存（discovery、mapping 等），删除后下次 kubectl 命令自动重建" => Some("kubectl cache (discovery, mapping, etc.), auto-rebuilt on next kubectl command"),
        "kubectl HTTP 缓存，删除后自动重建" => Some("kubectl HTTP cache, auto-rebuilt after deletion"),
        "Helm 仓库索引缓存，删除后执行 helm repo update 恢复" => Some("Helm repo index cache, run helm repo update to restore"),
        "Helm 插件缓存，可安全删除" => Some("Helm plugin cache, safe to delete"),
        "Docker BuildKit 构建缓存，删除后自动重建" => Some("Docker BuildKit build cache, auto-rebuilt after deletion"),
        "Docker Desktop 虚拟机数据，删除前请先退出 Docker Desktop" => Some("Docker Desktop VM data, quit Docker Desktop before deleting"),
        "Docker Desktop 缓存数据" => Some("Docker Desktop cache data"),
        "HuggingFace datasets 临时缓存，可安全删除" => Some("HuggingFace datasets temp cache, safe to delete"),
        "HuggingFace transformers 临时缓存，可安全删除" => Some("HuggingFace transformers temp cache, safe to delete"),
        "PyTorch 缓存（含预训练权重），删除后需重新下载" => Some("PyTorch cache (includes pretrained weights), needs re-download after deletion"),
        "llama.cpp 缓存，可安全删除" => Some("llama.cpp cache, safe to delete"),
        "Time Machine 本地快照，可安全删除" => Some("Time Machine local snapshots, safe to delete"),
        "iOS 模拟器运行时，删除后需重新下载" => Some("iOS simulator runtime, needs re-download after deletion"),
        "应用组缓存，删除后可能需重新配置" => Some("App group cache, may need reconfiguration after deletion"),
        "系统日志文件，可安全删除" => Some("System log files, safe to delete"),
        "Safari 缓存文件，可安全清理" => Some("Safari cache files, safe to clean"),
        "WebKit 网络缓存（被 Safari 等 App 共享），可安全清理" => Some("WebKit network cache (shared by Safari etc.), safe to clean"),
        "刷新 DNS 缓存，修复网络解析问题" => Some("Flush DNS cache, fixes network resolution issues"),
        "清理 QuickLook 缩略图缓存，修复预览问题" => Some("Clear QuickLook thumbnail cache, fixes preview issues"),
        "重建 LaunchServices 数据库，修复\"打开方式\"菜单问题" => Some("Rebuild LaunchServices database, fixes \"Open With\" menu issues"),
        "清理超过 30 天的应用保存状态" => Some("Clear app saved states older than 30 days"),
        "清理 Gatekeeper 下载追踪记录" => Some("Clear Gatekeeper download tracking records"),
        "释放非活跃内存，提升系统响应速度" => Some("Free inactive memory, improves system responsiveness"),
        "重建 Spotlight 搜索索引，修复搜索不到文件的问题" => Some("Rebuild Spotlight search index, fixes files not found in search"),
        // === 注册表新增缓存描述 ===
        "Carthage 依赖构建缓存，可安全删除" => Some("Carthage dependency build cache, safe to delete"),
        ".NET NuGet 包缓存，删除后需重新还原" => Some(".NET NuGet package cache, needs restore after deletion"),
        "Zig 编译缓存，可安全删除" => Some("Zig build cache, safe to delete"),
        "Elixir Mix 构建缓存，删除后需重新编译" => Some("Elixir Mix build cache, needs recompile after deletion"),
        "Elixir Hex 包缓存，可安全删除" => Some("Elixir Hex package cache, safe to delete"),
        "Haskell Stack 编译缓存和工具链，删除后需重新安装" => Some("Haskell Stack build cache and toolchain, needs reinstall after deletion"),
        "Nix 包管理器数据库，建议用 nix-collect-garbage 清理" => Some("Nix package manager database, use nix-collect-garbage to clean"),
        "Crystal shards 缓存，可安全删除" => Some("Crystal shards cache, safe to delete"),
        "Julia 包构件缓存，删除后需重新下载" => Some("Julia package artifacts cache, needs re-download after deletion"),
        "Nimble 包缓存，删除后需重新下载" => Some("Nimble package cache, needs re-download after deletion"),
        "LuaRocks 包缓存，删除后需重新安装" => Some("LuaRocks package cache, needs reinstall after deletion"),
        "R 语言包缓存，可安全删除" => Some("R language package cache, safe to delete"),
        "Bazel 构建缓存，删除后需重新构建" => Some("Bazel build cache, needs rebuild after deletion"),
        "Fastlane lane 缓存，可安全删除" => Some("Fastlane lane cache, safe to delete"),
        "React Native 依赖缓存，可安全删除" => Some("React Native dependency cache, safe to delete"),
        "Expo CLI 缓存，可安全删除" => Some("Expo CLI cache, safe to delete"),
        "VSCode 编辑器缓存，可安全删除" => Some("VSCode editor cache, safe to delete"),
        "VSCode 缓存的 V8 字节码，可安全删除" => Some("VSCode cached V8 bytecode, safe to delete"),
        "Cursor 编辑器缓存，可安全删除" => Some("Cursor editor cache, safe to delete"),
        "Postman API 工具缓存，可能包含请求历史" => Some("Postman API tool cache, may contain request history"),
        "Unity 编辑器缓存，删除后首次打开项目会变慢" => Some("Unity editor cache, first project open will be slower after deletion"),
        "Unreal Engine DerivedDataCache，删除后需重新编译着色器" => Some("Unreal Engine DerivedDataCache, needs shader recompile after deletion"),
        "Godot 编辑器缓存，可安全删除" => Some("Godot editor cache, safe to delete"),
        "Terraform 插件缓存，可安全删除" => Some("Terraform plugin cache, safe to delete"),
        "Vagrant box 镜像，删除后需重新下载" => Some("Vagrant box images, needs re-download after deletion"),
        "Electron 二进制缓存，可安全删除" => Some("Electron binary cache, safe to delete"),
        _ => None,
    }
}

/// 模式匹配的描述翻译（含动态参数）
fn pattern_description_en(desc: &str) -> Option<String> {
    // "项目 {} 的编译缓存，重新构建会自动恢复"
    if let Some(rest) = desc.strip_prefix("项目 ") {
        if let Some(name) = rest.strip_suffix(" 的编译缓存，重新构建会自动恢复") {
            return Some(format!("Build cache for project {}, auto-restored on rebuild", name));
        }
    }

    // "{} 旧版调试符号，可安全删除" or "iOS {} 旧版调试符号，可安全删除"
    if desc.ends_with(" 旧版调试符号，可安全删除") {
        let version = &desc[..desc.len() - " 旧版调试符号，可安全删除".len()];
        return Some(format!("{} legacy debug symbols, safe to delete", version));
    }
    if desc.ends_with(" 设备调试符号（最新版，建议保留）") {
        let version = &desc[..desc.len() - " 设备调试符号（最新版，建议保留）".len()];
        return Some(format!("{} device debug symbols (latest, recommended to keep)", version));
    }

    // "归档 {} ({})，包含构建和调试信息"
    if let Some(rest) = desc.strip_prefix("归档 ") {
        if rest.contains("），包含构建和调试信息") {
            return Some(format!("Archive {} — includes build and debug info", rest.trim_end_matches("），包含构建和调试信息").trim_end_matches('(')));
        }
    }

    // "iOS 模拟器运行时镜像，将通过 xcrun simctl runtime delete 安全删除"
    if desc.contains("将通过 xcrun simctl runtime delete 安全删除") {
        if desc.starts_with("iOS 模拟器运行时镜像") {
            return Some("iOS simulator runtime images, safely deleted via xcrun simctl runtime delete".to_string());
        }
        if desc.starts_with("模拟器运行时 Cryptex 扩展") {
            return Some("Simulator runtime Cryptex extension, safely deleted via xcrun simctl runtime delete".to_string());
        }
    }

    // "模拟器系统缓存，删除后自动重建，需管理员权限"
    if desc == "模拟器系统缓存，删除后自动重建，需管理员权限" {
        return Some("Simulator system cache, auto-rebuilt after deletion, requires admin privileges".to_string());
    }

    // "{} 的旧版本，最新版已保留"
    if let Some(rest) = desc.strip_suffix(" 的旧版本，最新版已保留") {
        return Some(format!("Old versions of {}, latest version retained", rest));
    }

    // "Downloads 下的安装包: {}"
    if let Some(filename) = desc.strip_prefix("Downloads 下的安装包: ") {
        return Some(format!("Installer in Downloads: {}", filename));
    }

    // "HuggingFace 模型 {}，删除后需重新下载"
    if let Some(rest) = desc.strip_prefix("HuggingFace 模型 ") {
        if let Some(model) = rest.strip_suffix("，删除后需重新下载") {
            return Some(format!("HuggingFace model {}, needs re-download after deletion", model));
        }
    }

    // "{} 本地模型（{} 个），删除后需 ollama pull 重新下载"
    if desc.contains("本地模型（") && desc.ends_with("），删除后需 ollama pull 重新下载") {
        let prefix_end = desc.find(" 本地模型（").unwrap_or(0);
        let name = &desc[..prefix_end];
        return Some(format!("{} local models, needs ollama pull to re-download", name));
    }

    // "{} monorepo，包含 {} 个子包的 node_modules\n 删除后需在 root 目录执行 {} install 恢复"
    if desc.contains("monorepo") && desc.contains("子包的 node_modules") {
        return Some(desc
            .replace("monorepo，包含", "monorepo, contains")
            .replace("个子包的 node_modules", " sub-packages' node_modules")
            .replace("删除后需在 root 目录执行", "Run")
            .replace("install 恢复", "install in root directory to restore")
            .replace("，", ",")
            .replace("\n ", "\n"));
    }

    // "指向 {} 的服务已失效（程序已被卸载），{}"
    if let Some(rest) = desc.strip_prefix("指向 ") {
        if rest.contains("的服务已失效（程序已被卸载）") {
            let parts: Vec<&str> = rest.splitn(2, "的服务已失效（程序已被卸载），").collect();
            if parts.len() == 2 {
                return Some(format!("Service pointing to {} is orphaned (program uninstalled), {}", parts[0], parts[1]));
            }
        }
    }

    // DS_Store description with count
    if desc.starts_with("Finder 自动生成的目录元数据文件，共 ") && desc.contains(" 个。") {
        let count_part: String = desc.chars()
            .skip("Finder 自动生成的目录元数据文件，共 ".len())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        return Some(format!(
            "Finder auto-generated directory metadata files, {} total.\nDeleted files are auto-rebuilt by Finder on directory access, no risk.",
            count_part
        ));
    }

    // "{} 缓存" or "{} 数据" patterns from app_cache
    if let Some(app_name) = desc.strip_suffix(" 的应用缓存，删除后自动重建") {
        return Some(format!("{} app cache, auto-rebuilt after deletion", app_name));
    }
    if let Some(app_name) = desc.strip_suffix(" 的应用数据（含文档、聊天记录等），删除可能导致数据丢失") {
        return Some(format!("{} app data (includes documents, chat history, etc.), deletion may cause data loss", app_name));
    }

    // App uninstall residual descriptions
    // "Downloads 中的 {}，同名应用已安装，可安全清理"
    if let Some(rest) = desc.strip_prefix("Downloads 中的 ") {
        if let Some(name) = rest.strip_suffix("，同名应用已安装，可安全清理") {
            return Some(format!("{} in Downloads, same-name app installed, safe to clean", name));
        }
    }
    // "Downloads 中的 {}，未检测到同名已安装应用，请确认后再删除"
    if let Some(rest) = desc.strip_prefix("Downloads 中的 ") {
        if let Some(name) = rest.strip_suffix("，未检测到同名已安装应用，请确认后再删除") {
            return Some(format!("{} in Downloads, no same-name installed app detected, confirm before deleting", name));
        }
    }
    // "{} 中 {} 的残留数据（App 可能已卸载）"
    if let Some(rest) = desc.strip_suffix(" 的残留数据（App 可能已卸载）") {
        if let Some(idx) = rest.rfind(" 中 ") {
            let vendor = &rest[..idx];
            let sub = &rest[idx + " 中 ".len()..];
            return Some(format!("Residual data of {} in {} (app may be uninstalled)", sub, vendor));
        }
    }
    // "{} 的残留数据（App 可能已卸载）"
    if let Some(name) = desc.strip_suffix(" 的残留数据（App 可能已卸载）") {
        return Some(format!("{} residual data (app may be uninstalled)", name));
    }
    // "{} 的残留缓存（App 可能已卸载）"
    if let Some(name) = desc.strip_suffix(" 的残留缓存（App 可能已卸载）") {
        return Some(format!("{} residual cache (app may be uninstalled)", name));
    }
    // "已卸载 App 的偏好设置文件，可安全删除"
    if desc == "已卸载 App 的偏好设置文件，可安全删除" {
        return Some("Preferences files of uninstalled app, safe to delete".to_string());
    }

    // "{} 的 {} 目录，删除后自动重建"
    if desc.ends_with("目录，删除后自动重建") {
        return Some(desc
            .replace("目录，删除后自动重建", "directory, auto-rebuilt after deletion")
            .replace("的", "'s"));
    }

    // "系统缓存目录，可安全删除"
    if desc == "系统缓存目录，可安全删除" {
        return Some("System cache directory, safe to delete".to_string());
    }

    // Browser cache descriptions
    // "Firefox ({}) 的 {} 缓存，可安全清理"
    if desc.starts_with("Firefox (") && desc.ends_with("缓存，可安全清理") {
        return Some(desc
            .replace("缓存，可安全清理", "cache, safe to clean")
            .replace("的", "'s"));
    }
    // "{} ({}) 的 {} 缓存，删除后浏览器会自动重建"
    if desc.ends_with("缓存，删除后浏览器会自动重建") {
        return Some(desc
            .replace("缓存，删除后浏览器会自动重建", "cache, auto-rebuilt by browser after deletion")
            .replace("的", "'s"));
    }

    // "⚠️ 高风险：应用数据目录，删除后可能丢失配置、登录状态或本地数据，App 可能无法启动"
    if desc.starts_with("⚠️ 高风险：应用数据目录") {
        return Some("⚠️ High risk: App data directory, deletion may lose config, login state or local data, app may not launch".to_string());
    }

    // "废纸篓中的 {} 残留，可安全清理释放空间"
    if let Some(rest) = desc.strip_prefix("废纸篓中的 ") {
        if let Some(name) = rest.strip_suffix(" 残留，可安全清理释放空间") {
            return Some(format!("{} residual in Trash, safe to clean to free space", name));
        }
    }

    // "{} 残留，可安全清理释放空间"
    if let Some(name) = desc.strip_suffix(" 残留，可安全清理释放空间") {
        return Some(format!("{} residual, safe to clean to free space", name));
    }

    // "应用大小 {}，关联文件 {} 项"
    if desc.starts_with("应用大小 ") && desc.contains("，关联文件 ") && desc.ends_with(" 项") {
        return Some(desc
            .replace("应用大小 ", "App size: ")
            .replace("，关联文件 ", ", associated files: ")
            .replace(" 项", " items"));
    }

    // Docker prune description with size
    if desc.starts_with("执行 docker system prune") {
        return Some(desc
            .replace("清理未使用镜像/容器/卷/网络", "clean unused images/containers/volumes/networks")
            .replace("可回收约", "can reclaim approx.")
            .replace("空间，清理后需重新拉取镜像", "space, needs re-pull after cleanup")
            .replace("，", ","));
    }

    // Disk analyzer descriptions
    if desc.starts_with("📁 ") && desc.contains("（可进入查看详情）") {
        return Some(desc.replace("（可进入查看详情）", " (click to enter)"));
    }
    if desc.starts_with("📄 ") {
        return Some(desc.to_string());
    }

    // "模拟器系统缓存..." variants
    if desc == "模拟器运行时 Cryptex 扩展，通过 xcrun simctl runtime delete 安全删除" {
        return Some("Simulator runtime Cryptex extension, safely deleted via xcrun simctl runtime delete".to_string());
    }

    // ===== 应用保护列表相关描述 =====

    // "系统关键应用 | 应用大小 {size}"
    if desc.starts_with("系统关键应用 | 应用大小 ") {
        let size = &desc["系统关键应用 | 应用大小 ".len()..];
        return Some(format!("System critical app | App size: {}", size));
    }

    // "{vendor} 安全代理 | 应用大小 {size}"
    if desc.contains(" 安全代理 | 应用大小 ") {
        return Some(desc
            .replace(" 安全代理 | 应用大小 ", " security agent | App size: "));
    }

    // "含敏感数据，卸载前请备份 | 应用大小 {size}，关联文件 {n} 项"
    if desc.starts_with("含敏感数据，卸载前请备份 | ") {
        let rest = &desc["含敏感数据，卸载前请备份 | ".len()..];
        return Some(format!("Contains sensitive data, back up before uninstalling | {}", rest
            .replace("应用大小 ", "App size: ")
            .replace("，关联文件 ", ", associated files: ")
            .replace(" 项", " items")));
    }

    // 登录项审计描述（含动态数量）
    // "审计登录项与启动服务（当前 X 项：登录项 X / 用户服务 X / 系统服务 X / 系统守护进程 X），点击打开系统设置管理"
    if desc.starts_with("审计登录项与启动服务（当前 ") && desc.contains("），点击打开系统设置管理") {
        return Some(desc
            .replace("审计登录项与启动服务（当前 ", "Audit login items and startup services (currently ")
            .replace(" 项：登录项 ", " items: login items ")
            .replace(" / 用户服务 ", " / user agents ")
            .replace(" / 系统服务 ", " / system agents ")
            .replace(" / 系统守护进程 ", " / system daemons ")
            .replace("），点击打开系统设置管理", ", click to open System Settings to manage"));
    }

    None
}

/// 翻译不可删除原因
///
/// 处理应用保护列表生成的原因 key：
/// - "protection_critical" -> "系统关键组件，不可卸载" / "System critical, cannot be uninstalled"
/// - "protection_official_uninstaller:CrowdStrike" -> "请使用 CrowdStrike 官方卸载工具" / "Use CrowdStrike's official uninstaller"
pub fn translate_undeletable_reason(reason: &str, lang_en: bool) -> String {
    if !lang_en {
        if reason == "protection_critical" {
            return "系统关键组件，不可卸载".to_string();
        }
        if let Some(vendor) = reason.strip_prefix("protection_official_uninstaller:") {
            return format!("请使用 {} 官方卸载工具", vendor);
        }
        return reason.to_string();
    }

    // 英文模式
    if reason == "protection_critical" {
        return "System critical, cannot be uninstalled".to_string();
    }
    if let Some(vendor) = reason.strip_prefix("protection_official_uninstaller:") {
        return format!("Use {}'s official uninstaller", vendor);
    }
    reason.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translate_recommend() {
        assert_eq!(translate_recommend(&Recommend::Safe, true), "Safe");
        assert_eq!(translate_recommend(&Recommend::Caution, true), "Caution");
        assert_eq!(translate_recommend(&Recommend::Advanced, true), "Advanced");
        assert_eq!(translate_recommend(&Recommend::Safe, false), "推荐");
        assert_eq!(translate_recommend(&Recommend::Caution, false), "谨慎");
        assert_eq!(translate_recommend(&Recommend::Advanced, false), "需确认");
    }

    #[test]
    fn test_translate_category_static() {
        assert_eq!(translate_category("Rust编译", true), "Rust Build");
        assert_eq!(translate_category("模拟器镜像", true), "Simulator Image");
        assert_eq!(translate_category("Rust编译", false), "Rust编译");
    }

    #[test]
    fn test_translate_category_dynamic() {
        assert_eq!(translate_category("Xcode编译-MyProject", true), "Xcode Build-MyProject");
        assert_eq!(translate_category("iOS设备-17.0", true), "iOS Device-17.0");
        assert_eq!(translate_category("孤儿服务-user", true), "Orphaned Service-user");
    }

    #[test]
    fn test_translate_description_exact() {
        assert_eq!(
            translate_description("Rust 编译产物，cargo build 会自动重新生成", true),
            "Rust build artifacts, cargo build will regenerate"
        );
        assert_eq!(
            translate_description("Rust 编译产物，cargo build 会自动重新生成", false),
            "Rust 编译产物，cargo build 会自动重新生成"
        );
    }

    #[test]
    fn test_translate_description_pattern() {
        assert_eq!(
            translate_description("项目 MyProject 的编译缓存，重新构建会自动恢复", true),
            "Build cache for project MyProject, auto-restored on rebuild"
        );
        assert_eq!(
            translate_description("Downloads 下的安装包: test.dmg", true),
            "Installer in Downloads: test.dmg"
        );
        assert_eq!(
            translate_description("HuggingFace 模型 bert-base，删除后需重新下载", true),
            "HuggingFace model bert-base, needs re-download after deletion"
        );
    }

    #[test]
    fn test_translate_app_uninstall_categories() {
        assert_eq!(translate_category("App残留", true), "App Residual");
        assert_eq!(translate_category("App残留缓存", true), "App Residual Cache");
        assert_eq!(translate_category("App残留配置", true), "App Residual Prefs");
    }

    #[test]
    fn test_translate_app_uninstall_descriptions() {
        assert_eq!(
            translate_description("Downloads 中的 Test.app，同名应用已安装，可安全清理", true),
            "Test.app in Downloads, same-name app installed, safe to clean"
        );
        assert_eq!(
            translate_description("Downloads 中的 Test.app，未检测到同名已安装应用，请确认后再删除", true),
            "Test.app in Downloads, no same-name installed app detected, confirm before deleting"
        );
        assert_eq!(
            translate_description("Google 中 Chrome 的残留数据（App 可能已卸载）", true),
            "Residual data of Chrome in Google (app may be uninstalled)"
        );
        assert_eq!(
            translate_description("FooApp 的残留数据（App 可能已卸载）", true),
            "FooApp residual data (app may be uninstalled)"
        );
        assert_eq!(
            translate_description("FooApp 的残留缓存（App 可能已卸载）", true),
            "FooApp residual cache (app may be uninstalled)"
        );
        assert_eq!(
            translate_description("已卸载 App 的偏好设置文件，可安全删除", true),
            "Preferences files of uninstalled app, safe to delete"
        );
    }

    #[test]
    fn test_translate_description_unknown() {
        // 未知描述原样返回
        assert_eq!(
            translate_description("some unknown description", true),
            "some unknown description"
        );
    }

    #[test]
    fn test_translate_protection_descriptions() {
        // 系统关键应用
        assert_eq!(
            translate_description("系统关键应用 | 应用大小 1.2G", true),
            "System critical app | App size: 1.2G"
        );
        // 安全代理
        assert_eq!(
            translate_description("CrowdStrike 安全代理 | 应用大小 500M", true),
            "CrowdStrike security agent | App size: 500M"
        );
        // 数据保护
        assert_eq!(
            translate_description("含敏感数据，卸载前请备份 | 应用大小 200M，关联文件 5 项", true),
            "Contains sensitive data, back up before uninstalling | App size: 200M, associated files: 5 items"
        );
    }

    #[test]
    fn test_translate_undeletable_reason() {
        // 中文
        assert_eq!(
            translate_undeletable_reason("protection_critical", false),
            "系统关键组件，不可卸载"
        );
        assert_eq!(
            translate_undeletable_reason("protection_official_uninstaller:CrowdStrike", false),
            "请使用 CrowdStrike 官方卸载工具"
        );
        // 英文
        assert_eq!(
            translate_undeletable_reason("protection_critical", true),
            "System critical, cannot be uninstalled"
        );
        assert_eq!(
            translate_undeletable_reason("protection_official_uninstaller:Jamf", true),
            "Use Jamf's official uninstaller"
        );
        // 未知原因原样返回
        assert_eq!(
            translate_undeletable_reason("some other reason", true),
            "some other reason"
        );
    }
}
