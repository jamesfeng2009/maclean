//! 扫描结果多语言翻译模块
//!
//! 在 UI 渲染时将 ScanItem 的 category 和 description 从中文翻译为英文。
//! 扫描器代码无需修改，仍生成中文，翻译在渲染层完成。
//!
//! 对于含动态参数的文本（如 "项目 X 的编译缓存"），使用模式匹配提取
//! 静态部分进行翻译，动态参数保持原样。

use crate::scanner::Recommend;

/// 翻译推荐等级
///
/// 注：UI 上的等级徽标统一走 `theme::recommend_label`（文案按设计稿 02 节：
/// 安全 / 仅缓存 / 谨慎 / 高级），这里保留给日志与其它纯文本场景。
#[allow(dead_code)]
pub fn translate_recommend(rec: &Recommend, lang_en: bool) -> &'static str {
    if lang_en {
        match rec {
            Recommend::Safe => "Safe",
            Recommend::CacheOnly => "Cache Only",
            Recommend::Caution => "Caution",
            Recommend::Advanced => "Advanced",
        }
    } else {
        match rec {
            Recommend::Safe => "推荐",
            Recommend::CacheOnly => "仅缓存",
            Recommend::Caution => "谨慎",
            Recommend::Advanced => "需确认",
        }
    }
}

/// UI 等级徽标文案（与 translate_recommend 是两套口径，勿合并）：
/// translate_recommend 用于普通说明文字，本函数是徽标短标签。
/// 迁移自 theme.rs（theme 模块是 GUI 专属，CLI 构建时被裁剪，徽标文案移到 i18n）。
pub fn recommend_label(rec: &Recommend, lang_en: bool) -> &'static str {
    match rec {
        Recommend::Safe => {
            if lang_en {
                "Safe"
            } else {
                "安全"
            }
        }
        Recommend::CacheOnly => {
            if lang_en {
                "Cache only"
            } else {
                "仅缓存"
            }
        }
        Recommend::Caution => {
            if lang_en {
                "Caution"
            } else {
                "谨慎"
            }
        }
        Recommend::Advanced => {
            if lang_en {
                "Advanced"
            } else {
                "高级"
            }
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
        "移动开发" => Some("Mobile Dev"),
        "开发者工具" => Some("Dev Tools"),
        "AI模型缓存" => Some("AI Model Cache"),
        "AI模型" => Some("AI Model"),
        "K8s缓存" => Some("K8s Cache"),
        "K8sHTTP缓存" => Some("K8s HTTP Cache"),
        "Helm缓存" => Some("Helm Cache"),
        "Helm插件缓存" => Some("Helm Plugin Cache"),
        "Docker清理" => Some("Docker Prune"),
        "Docker构建缓存" => Some("Docker Build Cache"),
        "Docker虚拟机" => Some("Docker VM"),
        // === Windows 专属类别 ===
        "WSL2虚拟磁盘" => Some("WSL2 Virtual Disk"),
        "Windows临时文件" => Some("Windows Temp Files"),
        "Windows更新缓存" => Some("Windows Update Cache"),
        "Windows缩略图" => Some("Windows Thumbnails"),
        // Windows App 缓存/数据（动态生成，"{app} 缓存"/"{app} 数据" 已有通用翻译）
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
/// 查找顺序：**缓存注册表 → 精确匹配表 → 模式匹配**。
/// 注册表优先是因为它才是每条缓存定义的"唯一真相" —— 中文 `desc_zh`
/// 与英文 `desc_en` 写在同一个 `CacheDef` 里，加新缓存时必然成对出现。
///
/// 2026-09-19 之前的实际状态：注册表里 38 条 `desc_en` 写好了却没人读，
/// 渲染层只走 i18n 这张手写表，而手写表把那 38 条**又抄了一份**（共 92 条
/// 里占 38 条）。两份副本当时已经出现 1 处措辞不一致（rbenv：注册表写
/// "Ruby versions installed by rbenv, confirm before deletion"，手写表写
/// "rbenv installed Ruby versions, confirm before deleting"）—— 这种漂移
/// 没有任何编译期保护，改一处忘一处就是线上文案错乱。
/// 现在注册表优先，手写表里那 38 条重复项已全部删除（92 → 68）。
///
/// 未知描述原样返回。
pub fn translate_description(desc: &str, lang_en: bool) -> String {
    if !lang_en {
        return desc.to_string();
    }

    // 1. 缓存注册表（唯一真相，覆盖 38 条固定缓存定义）
    if let Some(en) = registry_description_en(desc) {
        return en.to_string();
    }

    // 2. 精确匹配（注册表之外的固定文案：dev_cache / apfs / 系统项等）
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

/// 从缓存注册表查英文描述
///
/// 扫描项的描述文本就是 `CacheDef::desc_zh`（见 cache_registry 的扫描循环），
/// 所以按中文原文反查定义，取同一条定义里写好的 `desc_en`。
fn registry_description_en(desc: &str) -> Option<&'static str> {
    crate::scanner::cache_registry::CACHE_REGISTRY
        .iter()
        .find(|def| def.desc_zh == desc)
        .map(|def| def.desc_en)
}

/// 精确匹配的描述翻译
///
/// 这里只放**注册表之外**的文案（dev_cache / apfs / 系统项等固定描述）。
/// 属于 CACHE_REGISTRY 的描述不要往这里加 —— 2026-09-19 已清掉全部 38 条
/// 重复项（92 → 68），测试 `registry_descriptions_are_not_duplicated_here`
/// 会拦住再次重复。
fn exact_description_en(desc: &str) -> Option<&'static str> {
    match desc {
        "Rust 编译产物，cargo build 会自动重新生成" => {
            Some("Rust build artifacts, cargo build will regenerate")
        }
        "Cargo 包下载缓存，删除后编译时需重新下载" => {
            Some("Cargo package download cache, needs re-download after deletion")
        }
        "Xcode 在线文档缓存，可安全删除" => {
            Some("Xcode online documentation cache, safe to delete")
        }
        "设备日志和崩溃报告，可安全删除" => {
            Some("Device logs and crash reports, safe to delete")
        }
        "Xcode 旧版离线文档，可能不再需要" => {
            Some("Xcode legacy offline docs, may no longer be needed")
        }
        "watchOS 设备调试符号，连接手表时会重新生成" => {
            Some("watchOS device debug symbols, regenerated when watch is connected")
        }
        "Apple Connect 日志，可安全删除" => Some("Apple Connect logs, safe to delete"),
        "Node.js 依赖包，npm install 可恢复" => {
            Some("Node.js dependencies, npm install can restore")
        }
        "pnpm 全局存储，删除后需重新安装依赖" => {
            Some("pnpm global store, needs reinstall after deletion")
        }
        "npm 下载缓存，可安全删除" => Some("npm download cache, safe to delete"),
        "Go 模块缓存，编译时需重新下载" => {
            Some("Go module cache, needs re-download when compiling")
        }
        "Homebrew 下载缓存，可安全删除" => {
            Some("Homebrew download cache, safe to delete")
        }
        "Homebrew 已下载的安装包，可安全删除" => {
            Some("Homebrew downloaded packages, safe to delete")
        }
        "pip 下载缓存，可安全删除" => Some("pip download cache, safe to delete"),
        "JetBrains IDE 旧版本配置，已保留最新版" => {
            Some("JetBrains IDE old version config, latest version retained")
        }
        "JetBrains IDE 缓存，重启 IDE 会自动重建" => {
            Some("JetBrains IDE cache, rebuilt on IDE restart")
        }
        "Gradle 构建缓存，删除后编译时需重新下载依赖" => {
            Some("Gradle build cache, needs re-download after deletion")
        }
        "Gradle Wrapper 下载的版本，可安全删除会自动重新下载" => {
            Some("Gradle Wrapper downloaded versions, safe to delete, auto re-downloads")
        }
        "Maven 本地依赖仓库，删除后编译时需重新下载" => {
            Some("Maven local dependency repo, needs re-download after deletion")
        }
        "Java/Gradle 项目编译产物，gradle build 会自动重新生成" => {
            Some("Java/Gradle build artifacts, gradle build will regenerate")
        }
        "Conda 包缓存，删除后安装时需重新下载" => {
            Some("Conda package cache, needs re-download after deletion")
        }
        "Poetry 依赖缓存，可安全删除" => Some("Poetry dependency cache, safe to delete"),
        "Python 字节码缓存，运行时自动重建" => {
            Some("Python bytecode cache, auto-rebuilt at runtime")
        }
        "前端构建产物，npm run build 会重新生成" => {
            Some("Frontend build artifacts, npm run build will regenerate")
        }
        "Next.js 构建缓存，可安全删除" => Some("Next.js build cache, safe to delete"),
        "Nuxt.js 构建缓存，可安全删除" => Some("Nuxt.js build cache, safe to delete"),
        "Turborepo 缓存，可安全删除" => Some("Turborepo cache, safe to delete"),
        "SvelteKit 构建缓存，可安全删除" => Some("SvelteKit build cache, safe to delete"),
        "Astro 构建缓存，可安全删除" => Some("Astro build cache, safe to delete"),
        "Remix 构建缓存，可安全删除" => Some("Remix build cache, safe to delete"),
        "Gradle 项目本地缓存，可安全删除" => {
            Some("Gradle project local cache, safe to delete")
        }
        "kubectl 缓存（discovery、mapping 等），删除后下次 kubectl 命令自动重建" => {
            Some("kubectl cache (discovery, mapping, etc.), auto-rebuilt on next kubectl command")
        }
        "kubectl HTTP 缓存，删除后自动重建" => {
            Some("kubectl HTTP cache, auto-rebuilt after deletion")
        }
        "Helm 仓库索引缓存，删除后执行 helm repo update 恢复" => {
            Some("Helm repo index cache, run helm repo update to restore")
        }
        "Helm 插件缓存，可安全删除" => Some("Helm plugin cache, safe to delete"),
        "Docker BuildKit 构建缓存，删除后自动重建" => {
            Some("Docker BuildKit build cache, auto-rebuilt after deletion")
        }
        "Docker Desktop 虚拟机数据，删除前请先退出 Docker Desktop" => {
            Some("Docker Desktop VM data, quit Docker Desktop before deleting")
        }
        "Docker Desktop 缓存数据" => Some("Docker Desktop cache data"),
        "HuggingFace datasets 临时缓存，可安全删除" => {
            Some("HuggingFace datasets temp cache, safe to delete")
        }
        "HuggingFace transformers 临时缓存，可安全删除" => {
            Some("HuggingFace transformers temp cache, safe to delete")
        }
        "PyTorch 缓存（含预训练权重），删除后需重新下载" => {
            Some("PyTorch cache (includes pretrained weights), needs re-download after deletion")
        }
        "llama.cpp 缓存，可安全删除" => Some("llama.cpp cache, safe to delete"),
        // === Windows 专属描述 ===
        "Windows 临时文件目录，可安全删除" => {
            Some("Windows temp files directory, safe to delete")
        }
        "Windows Update 下载缓存，需停止 wuauserv 服务后清理" => {
            Some("Windows Update download cache, stop wuauserv service before cleaning")
        }
        "Windows 资源管理器缩略图缓存，删除后自动重建" => {
            Some("Windows Explorer thumbnail cache, auto-rebuilt after deletion")
        }
        "应用数据删除可能导致应用配置丢失，请确认后手动删除" => {
            Some("App data deletion may cause configuration loss, confirm before manual deletion")
        }
        "系统关键应用，禁止卸载" => Some("System critical app, uninstall blocked"),
        "安全软件，请使用官方卸载工具" => {
            Some("Security software, use official uninstaller")
        }
        "未知的卸载路径格式" => Some("Unknown uninstall path format"),
        "Time Machine 本地快照，可安全删除" => {
            Some("Time Machine local snapshots, safe to delete")
        }
        "iOS 模拟器运行时，删除后需重新下载" => {
            Some("iOS simulator runtime, needs re-download after deletion")
        }
        "应用组缓存，删除后可能需重新配置" => {
            Some("App group cache, may need reconfiguration after deletion")
        }
        "系统日志文件，可安全删除" => Some("System log files, safe to delete"),
        "Safari 缓存文件，可安全清理" => Some("Safari cache files, safe to clean"),
        "WebKit 网络缓存（被 Safari 等 App 共享），可安全清理" => {
            Some("WebKit network cache (shared by Safari etc.), safe to clean")
        }
        "刷新 DNS 缓存，修复网络解析问题" => {
            Some("Flush DNS cache, fixes network resolution issues")
        }
        "清理 QuickLook 缩略图缓存，修复预览问题" => {
            Some("Clear QuickLook thumbnail cache, fixes preview issues")
        }
        "重建 LaunchServices 数据库，修复\"打开方式\"菜单问题" => {
            Some("Rebuild LaunchServices database, fixes \"Open With\" menu issues")
        }
        "清理超过 30 天的应用保存状态" => {
            Some("Clear app saved states older than 30 days")
        }
        "清理 Gatekeeper 下载追踪记录" => {
            Some("Clear Gatekeeper download tracking records")
        }
        "释放非活跃内存，提升系统响应速度" => {
            Some("Free inactive memory, improves system responsiveness")
        }
        "重建 Spotlight 搜索索引，修复搜索不到文件的问题" => {
            Some("Rebuild Spotlight search index, fixes files not found in search")
        }
        // === Windows 优化任务描述 ===
        // （"刷新 DNS 缓存，修复网络解析问题" 与 macOS 任务共用，已在上方定义，此处去重）
        "清理 Windows 临时文件、缩略图缓存、交付优化缓存" => {
            Some("Clean Windows temp files, thumbnail cache, delivery optimization cache")
        }
        "关闭 Windows 遥测与诊断数据收集，减少隐私泄露" => Some(
            "Disable Windows telemetry and diagnostic data collection, reduces privacy exposure",
        ),
        "禁用 Windows Copilot 与 AI 功能，释放内存与后台资源" => {
            Some("Disable Windows Copilot and AI features, frees memory and background resources")
        }
        "关闭开始菜单、设置、锁屏的推荐与广告内容" => Some(
            "Disable recommended and promotional content in Start menu, Settings, and lock screen",
        ),
        "审计开机启动项与计划任务，加快开机速度" => {
            Some("Audit startup items and scheduled tasks, speeds up boot time")
        }
        "关闭快速启动，减少休眠文件占用并避免驱动异常" => {
            Some("Disable fast startup, reduces hibernation file usage and avoids driver issues")
        }
        "创建系统还原点，优化前自动备份当前状态" => Some(
            "Create system restore point, automatically backs up current state before optimization",
        ),
        "重启资源管理器，刷新任务栏/开始菜单/桌面" => {
            Some("Restart Explorer, refreshes taskbar/Start menu/desktop")
        }
        "对 SSD 执行 TRIM 优化，对 HDD 执行碎片整理" => {
            Some("Run TRIM optimization for SSDs, defragmentation for HDDs")
        }
        // === builtin_rules.json 扩展规则描述（P0） ===
        "pnpm 全局存储目录，删除后按需重新下载" => {
            Some("pnpm global store, re-downloaded on demand")
        }
        "Yarn 依赖缓存（v6），删除后重新安装时自动重建" => {
            Some("Yarn dependency cache (v6), rebuilt on next install")
        }
        "Bun 安装缓存，删除后按需重新下载" => {
            Some("Bun install cache, re-downloaded on demand")
        }
        "Deno 模块缓存（npm/remote），删除后自动重新拉取" => {
            Some("Deno module cache (npm/remote), re-fetched automatically")
        }
        "Go 编译缓存，go build 会自动重建" => Some("Go build cache, rebuilt by go build"),
        "uv 包缓存，删除后按需重新下载" => {
            Some("uv package cache, re-downloaded on demand")
        }
        "Poetry 包缓存，删除后按需重新下载" => {
            Some("Poetry package cache, re-downloaded on demand")
        }
        "Conda 包缓存（pkgs），体积大，删除后环境可能需重新下载依赖" => {
            Some("Conda package cache (pkgs), large; environments may need re-download")
        }
        "sccache 编译缓存，自动重新生成" => {
            Some("sccache compile cache, regenerated automatically")
        }
        "NuGet 包缓存，删除后重新还原" => {
            Some("NuGet package cache, restored on next build")
        }
        "Maven 本地仓库，删除后重新下载依赖" => {
            Some("Maven local repository, dependencies re-downloaded")
        }
        "CMake 包注册缓存，自动重建" => {
            Some("CMake package registry cache, rebuilt automatically")
        }
        "CocoaPods 仓库缓存，pod install 自动重建" => {
            Some("CocoaPods repo cache, rebuilt by pod install")
        }
        "Swift Package Manager 缓存，Xcode 自动重建" => {
            Some("Swift Package Manager cache, rebuilt by Xcode")
        }
        "Android SDK 临时下载文件" => Some("Android SDK temporary download files"),
        "Flutter pub 包缓存与引擎缓存，自动重建" => {
            Some("Flutter pub/engine cache, rebuilt automatically")
        }
        "VS Code 渲染/GPU/代码缓存，重启后自动重建" => {
            Some("VS Code render/GPU/code cache, rebuilt on restart")
        }
        "VS Code 已安装扩展的安装包缓存" => {
            Some("VS Code cached extension VSIX installers")
        }
        "JetBrains IDE 索引与缓存（IntelliJ/GoLand/PyCharm 等）" => {
            Some("JetBrains IDE indexes and caches (IntelliJ/GoLand/PyCharm etc.)")
        }
        "Xcode 用户级缓存，自动重建" => {
            Some("Xcode user-level cache, rebuilt automatically")
        }
        "Homebrew 下载与 API 缓存，自动重建" => {
            Some("Homebrew downloads and API cache, rebuilt automatically")
        }
        "HuggingFace 模型/数据集缓存，删除后需重新下载" => {
            Some("HuggingFace model/dataset cache, re-download needed")
        }
        "Ollama 本地模型（重新下载成本高）" => {
            Some("Ollama local models (expensive to re-download)")
        }
        "LM Studio 模型缓存（重新下载成本高）" => {
            Some("LM Studio model cache (expensive to re-download)")
        }
        "llama.cpp 模型缓存" => Some("llama.cpp model cache"),
        "MLX 模型缓存" => Some("MLX model cache"),
        "Replicate 模型缓存" => Some("Replicate model cache"),
        "Podman 容器与镜像数据（删除前请先停止 Podman）" => {
            Some("Podman containers and images (stop Podman first)")
        }
        "OrbStack 容器与镜像数据（删除前请先退出 OrbStack）" => {
            Some("OrbStack containers and images (quit OrbStack first)")
        }
        "Colima 虚拟机与容器数据（删除前请先停止 colima）" => {
            Some("Colima VM and container data (stop colima first)")
        }
        "Rancher Desktop 容器缓存（删除前请先退出）" => {
            Some("Rancher Desktop container cache (quit first)")
        }
        "minikube 镜像缓存（删除前请先停止 minikube）" => {
            Some("minikube image cache (stop minikube first)")
        }
        // === 注册表新增缓存描述 ===
        _ => None,
    }
}

/// 模式匹配的描述翻译（含动态参数）
fn pattern_description_en(desc: &str) -> Option<String> {
    // "项目 {} 的编译缓存，重新构建会自动恢复"
    if let Some(rest) = desc.strip_prefix("项目 ") {
        if let Some(name) = rest.strip_suffix(" 的编译缓存，重新构建会自动恢复") {
            return Some(format!(
                "Build cache for project {}, auto-restored on rebuild",
                name
            ));
        }
    }

    // "{} 旧版调试符号，可安全删除" or "iOS {} 旧版调试符号，可安全删除"
    if let Some(version) = desc.strip_suffix(" 旧版调试符号，可安全删除") {
        return Some(format!("{} legacy debug symbols, safe to delete", version));
    }
    if let Some(version) = desc.strip_suffix(" 设备调试符号（最新版，建议保留）") {
        return Some(format!(
            "{} device debug symbols (latest, recommended to keep)",
            version
        ));
    }

    // "归档 {} ({})，包含构建和调试信息"
    if let Some(rest) = desc.strip_prefix("归档 ") {
        if rest.contains("），包含构建和调试信息") {
            return Some(format!(
                "Archive {} — includes build and debug info",
                rest.trim_end_matches("），包含构建和调试信息")
                    .trim_end_matches('(')
            ));
        }
    }

    // "iOS 模拟器运行时镜像，将通过 xcrun simctl runtime delete 安全删除"
    if desc.contains("将通过 xcrun simctl runtime delete 安全删除") {
        if desc.starts_with("iOS 模拟器运行时镜像") {
            return Some(
                "iOS simulator runtime images, safely deleted via xcrun simctl runtime delete"
                    .to_string(),
            );
        }
        if desc.starts_with("模拟器运行时 Cryptex 扩展") {
            return Some("Simulator runtime Cryptex extension, safely deleted via xcrun simctl runtime delete".to_string());
        }
    }

    // "模拟器系统缓存，删除后自动重建，需管理员权限"
    if desc == "模拟器系统缓存，删除后自动重建，需管理员权限" {
        return Some(
            "Simulator system cache, auto-rebuilt after deletion, requires admin privileges"
                .to_string(),
        );
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
            return Some(format!(
                "HuggingFace model {}, needs re-download after deletion",
                model
            ));
        }
    }

    // "{} 本地模型（{} 个），删除后需 ollama pull 重新下载"
    if desc.contains("本地模型（") && desc.ends_with("），删除后需 ollama pull 重新下载")
    {
        let prefix_end = desc.find(" 本地模型（").unwrap_or(0);
        let name = &desc[..prefix_end];
        return Some(format!(
            "{} local models, needs ollama pull to re-download",
            name
        ));
    }

    // "{} monorepo，包含 {} 个子包的 node_modules\n 删除后需在 root 目录执行 {} install 恢复"
    if desc.contains("monorepo") && desc.contains("子包的 node_modules") {
        return Some(
            desc.replace("monorepo，包含", "monorepo, contains")
                .replace("个子包的 node_modules", " sub-packages' node_modules")
                .replace("删除后需在 root 目录执行", "Run")
                .replace("install 恢复", "install in root directory to restore")
                .replace("，", ",")
                .replace("\n ", "\n"),
        );
    }

    // "指向 {} 的服务已失效（程序已被卸载），{}"
    if let Some(rest) = desc.strip_prefix("指向 ") {
        if rest.contains("的服务已失效（程序已被卸载）") {
            let parts: Vec<&str> = rest.splitn(2, "的服务已失效（程序已被卸载），").collect();
            if parts.len() == 2 {
                return Some(format!(
                    "Service pointing to {} is orphaned (program uninstalled), {}",
                    parts[0], parts[1]
                ));
            }
        }
    }

    // DS_Store description with count
    if desc.starts_with("Finder 自动生成的目录元数据文件，共 ") && desc.contains(" 个。")
    {
        let count_part: String = desc
            .chars()
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
        return Some(format!(
            "{} app cache, auto-rebuilt after deletion",
            app_name
        ));
    }
    if let Some(app_name) =
        desc.strip_suffix(" 的应用数据（含文档、聊天记录等），删除可能导致数据丢失")
    {
        return Some(format!(
            "{} app data (includes documents, chat history, etc.), deletion may cause data loss",
            app_name
        ));
    }

    // App uninstall residual descriptions
    // "Downloads 中的 {}，同名应用已安装，可安全清理"
    if let Some(rest) = desc.strip_prefix("Downloads 中的 ") {
        if let Some(name) = rest.strip_suffix("，同名应用已安装，可安全清理") {
            return Some(format!(
                "{} in Downloads, same-name app installed, safe to clean",
                name
            ));
        }
    }
    // "Downloads 中的 {}，未检测到同名已安装应用，请确认后再删除"
    if let Some(rest) = desc.strip_prefix("Downloads 中的 ") {
        if let Some(name) = rest.strip_suffix("，未检测到同名已安装应用，请确认后再删除")
        {
            return Some(format!(
                "{} in Downloads, no same-name installed app detected, confirm before deleting",
                name
            ));
        }
    }
    // "{} 中 {} 的残留数据（App 可能已卸载）"
    if let Some(rest) = desc.strip_suffix(" 的残留数据（App 可能已卸载）") {
        if let Some(idx) = rest.rfind(" 中 ") {
            let vendor = &rest[..idx];
            let sub = &rest[idx + " 中 ".len()..];
            return Some(format!(
                "Residual data of {} in {} (app may be uninstalled)",
                sub, vendor
            ));
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
        return Some(
            desc.replace(
                "目录，删除后自动重建",
                "directory, auto-rebuilt after deletion",
            )
            .replace("的", "'s"),
        );
    }

    // "系统缓存目录，可安全删除"
    if desc == "系统缓存目录，可安全删除" {
        return Some("System cache directory, safe to delete".to_string());
    }

    // Browser cache descriptions
    // "Firefox ({}) 的 {} 缓存，可安全清理"
    if desc.starts_with("Firefox (") && desc.ends_with("缓存，可安全清理") {
        return Some(
            desc.replace("缓存，可安全清理", "cache, safe to clean")
                .replace("的", "'s"),
        );
    }
    // "{} ({}) 的 {} 缓存，删除后浏览器会自动重建"
    if desc.ends_with("缓存，删除后浏览器会自动重建") {
        return Some(
            desc.replace(
                "缓存，删除后浏览器会自动重建",
                "cache, auto-rebuilt by browser after deletion",
            )
            .replace("的", "'s"),
        );
    }

    // "⚠️ 高风险：应用数据目录，删除后可能丢失配置、登录状态或本地数据，App 可能无法启动"
    if desc.starts_with("⚠️ 高风险：应用数据目录") {
        return Some("⚠️ High risk: App data directory, deletion may lose config, login state or local data, app may not launch".to_string());
    }

    // "废纸篓中的 {} 残留，可安全清理释放空间"
    if let Some(rest) = desc.strip_prefix("废纸篓中的 ") {
        if let Some(name) = rest.strip_suffix(" 残留，可安全清理释放空间") {
            return Some(format!(
                "{} residual in Trash, safe to clean to free space",
                name
            ));
        }
    }

    // "{} 残留，可安全清理释放空间"
    if let Some(name) = desc.strip_suffix(" 残留，可安全清理释放空间") {
        return Some(format!("{} residual, safe to clean to free space", name));
    }

    // "应用大小 {}，关联文件 {} 项"
    if desc.starts_with("应用大小 ") && desc.contains("，关联文件 ") && desc.ends_with(" 项")
    {
        return Some(
            desc.replace("应用大小 ", "App size: ")
                .replace("，关联文件 ", ", associated files: ")
                .replace(" 项", " items"),
        );
    }

    // Docker prune description with size
    if desc.starts_with("执行 docker system prune") {
        return Some(
            desc.replace(
                "清理未使用镜像/容器/卷/网络",
                "clean unused images/containers/volumes/networks",
            )
            .replace("可回收约", "can reclaim approx.")
            .replace(
                "空间，清理后需重新拉取镜像",
                "space, needs re-pull after cleanup",
            )
            .replace("，", ","),
        );
    }

    // Disk analyzer descriptions
    if desc.starts_with("📁 ") && desc.contains("（可进入查看详情）") {
        return Some(desc.replace("（可进入查看详情）", " (click to enter)"));
    }
    if desc.starts_with("📄 ") {
        return Some(desc.to_string());
    }

    // "模拟器系统缓存..." variants
    if desc == "模拟器运行时 Cryptex 扩展，通过 xcrun simctl runtime delete 安全删除"
    {
        return Some(
            "Simulator runtime Cryptex extension, safely deleted via xcrun simctl runtime delete"
                .to_string(),
        );
    }

    // ===== 应用保护列表相关描述 =====

    // "系统关键应用 | 应用大小 {size}"
    if let Some(size) = desc.strip_prefix("系统关键应用 | 应用大小 ") {
        return Some(format!("System critical app | App size: {}", size));
    }

    // "{vendor} 安全代理 | 应用大小 {size}"
    if desc.contains(" 安全代理 | 应用大小 ") {
        return Some(desc.replace(" 安全代理 | 应用大小 ", " security agent | App size: "));
    }

    // "含敏感数据，卸载前请备份 | 应用大小 {size}，关联文件 {n} 项"
    if let Some(rest) = desc.strip_prefix("含敏感数据，卸载前请备份 | ") {
        return Some(format!(
            "Contains sensitive data, back up before uninstalling | {}",
            rest.replace("应用大小 ", "App size: ")
                .replace("，关联文件 ", ", associated files: ")
                .replace(" 项", " items")
        ));
    }

    // 登录项审计描述（含动态数量）
    // "审计登录项与启动服务（当前 X 项：登录项 X / 用户服务 X / 系统服务 X / 系统守护进程 X），点击打开系统设置管理"
    if desc.starts_with("审计登录项与启动服务（当前 ") && desc.contains("），点击打开系统设置管理")
    {
        return Some(
            desc.replace(
                "审计登录项与启动服务（当前 ",
                "Audit login items and startup services (currently ",
            )
            .replace(" 项：登录项 ", " items: login items ")
            .replace(" / 用户服务 ", " / user agents ")
            .replace(" / 系统服务 ", " / system agents ")
            .replace(" / 系统守护进程 ", " / system daemons ")
            .replace(
                "），点击打开系统设置管理",
                ", click to open System Settings to manage",
            ),
        );
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
        if let Some(procs) = reason.strip_prefix("required_stopped_processes:") {
            return format!("删除前请先退出: {}", procs);
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
    if let Some(procs) = reason.strip_prefix("required_stopped_processes:") {
        return format!("Quit before deleting: {}", procs);
    }
    reason.to_string()
}

/// 格式化多语言文本（支持 `{0}`/`{1}` 与无索引 `{}` 两种占位符）。
///
/// 2026-10 阶段 0 从 app.rs 的 `App::tf_lang` 下沉至此：纯字符串处理，
/// 不依赖任何 UI 状态，core 删除/扫描线程直接用它拼日志文案。
///
/// - 无索引 `{}`：按出现顺序依次替换为 args（第 1 个 `{}` 用 args[0]）；
/// - 带索引 `{i}`：替换为 args[i]；
///
/// 两种占位符可混用；args 不足时多余占位符保持原样。
pub fn tf_lang(lang_en: bool, key: &str, args: &[&str]) -> String {
    let mut s = t_lang(lang_en, key).to_string();
    for arg in args {
        if let Some(pos) = s.find("{}") {
            s.replace_range(pos..pos + 2, arg);
        } else {
            break;
        }
    }
    for (i, arg) in args.iter().enumerate() {
        s = s.replace(&format!("{{{}}}", i), arg);
    }
    s
}

/// 主翻译表（2026-10 从 app.rs 的 `App::t_lang` 下沉至此，逐字保留）。
///
/// 未命中的 key 返回空串 —— 与原实现一致，ops 日志等调用方依赖这个 fallback。
pub fn t_lang(lang_en: bool, key: &str) -> &'static str {
    if lang_en {
        match key {
                // Tab 标题
                "tab_overview" => "Overview",
                "tab_dev_cache" => "Dev Cache",
                "tab_large_files" => "Large Files",
                "tab_app_cache" => "App Cache",
                "tab_app_data" => "App Data",
                "tab_app_uninstall" => "Uninstall",
                "tab_system_optimize" => "Optimize",
                "tab_apfs" => "APFS Snapshots",
                "tab_custom_rules" => "Custom Rules",
                "tab_dup_files" => "Duplicate Files",
                "tab_startup_items" => "Startup Items",
                "startup_hint" => "LaunchAgents / LaunchDaemons that run at login. Disabling moves the plist to a backup folder (reversible); running services are not force-stopped.",
                "startup_refresh" => "Refresh",
                "startup_refreshed" => "Startup items refreshed",
                "startup_count" => "{0} items, {1} loaded",
                "startup_enabled" => "Loaded",
                "startup_disabled" => "Disabled",
                "startup_disable" => "Disable",
                "startup_enable" => "Enable",
                "startup_disabled_log" => "Disabled {0} -> moved to {1}",
                "startup_enabled_log" => "Enabled {0} <- restored from {1}",
                "startup_disable_fail" => "Disable failed",
                "startup_enable_fail" => "Enable failed",
                "disk_view_list" => "List",
                "disk_view_tree" => "Treemap",
                "tab_settings" => "Settings",
                "settings_appearance" => "Appearance",
                "setting_dark_mode" => "Dark mode",
                "setting_dark_mode_desc" => "Switch the whole UI between light and dark",
                "settings_general" => "General",
                "settings_safety" => "Safety",
                "settings_language" => "Language",
                "setting_menubar_icon" => "Show menu bar icon on launch",
                "setting_menubar_icon_desc" => "Display disk usage indicator in the macOS menu bar",
                "setting_keep_sudo" => "Keep sudo session alive",
                "setting_keep_sudo_desc" => "Avoid repeated administrator password prompts",
                "setting_scan_cache" => "Cache scan results locally",
                "setting_scan_cache_desc" => "Avoid re-scanning within 7 days for faster startup",
                "setting_scan_all_disks" => "Scan all disks",
                "setting_scan_all_disks_desc" => {
                    "C: only by default. Enabling also scans Program Files on D:/E:/etc."
                },
                // M-1
                "setting_official_uninstaller" => "Use the app's own uninstaller when available",
                "setting_official_uninstaller_desc" => {
                    "Deleting a .app bundle leaves launchd jobs and pkgutil receipts behind. The vendor uninstaller cleans those up."
                },
                "confirm_official_uninstaller_hint" => {
                    "Handed to the app's own uninstaller (opens a separate window)"
                },
                "log_official_uninstaller" => "Handed to official uninstaller",
                // C-1 · 付费分级
                "tier_free" => "Free",
                "tier_pro" => "Pro",
                "tier_capability_unlimited" => "Unlimited cleanup quota",
                "tier_capability_scheduled" => "Scheduled cleanup",
                "tier_pro_includes" => "Pro adds: {}",
                // C-3 · 定时清理
                "settings_schedule" => "Scheduled cleanup",
                "setting_schedule_enable" => "Run cleanup automatically",
                "setting_schedule_enable_desc" => {
                    "Registers a system job, so cleanup runs even when the app is closed"
                },
                "setting_schedule_interval" => "Interval",
                "schedule_every_day" => "Daily",
                "schedule_every_week" => "Weekly",
                "schedule_every_month" => "Monthly",
                "schedule_last_run" => "Last run: {}",
                "schedule_never_run" => "Never run yet",
                "schedule_pro_only" => "Scheduled cleanup is a Pro feature",
                "schedule_installed" => "Scheduled job registered",
                "schedule_install_failed" => "Could not register the job: {}",
                "schedule_removed" => "Scheduled job removed",
                "schedule_remove_failed" => "Could not remove the job: {}",
                // C-2 · 自动更新
                "update_download_aborted" => "Update download stopped unexpectedly",
                "setting_confirm_advanced" => "Double-check before deleting Advanced items",
                "setting_confirm_advanced_desc" => "Advanced items require manual confirmation",
                "setting_show_protected_items" => "Show protected / system items",
                "setting_show_protected_items_desc" => "Items protected by macOS (undeletable by any privilege) are hidden by default to avoid failed-delete loops; enable to show them greyed out for troubleshooting",
                "setting_prevent_lid_close" => "Prevent deletion while lid is closed",
                "setting_prevent_lid_close_desc" => "Detect MacBook clamshell state to avoid accidental deletion",
                "setting_auto_restore_point" => "Auto-create restore point before operations",
                "setting_auto_restore_point_desc" => "Create a system restore point before cleanup/optimization (Windows)",
                "setting_restore_last" => "Restore last registry modification",
                "setting_restore_last_desc" => "Re-import the most recent registry backup made before optimization. Restores old values only; newly added keys must be removed manually",
                "restore_last_btn" => "Restore",
                "restore_last_none" => "No registry backup found",
                "restore_last_success" => "✅ Restored: {0}",
                "restore_last_failed" => "⚠️ Restore failed (administrator may be required)",
                "restore_last_backup_info" => "Last backup: {0} ({1}, {2} keys)",
                "restore_point_created" => "✅ System restore point created",
                "restore_point_skipped" => "⏭️ Recent restore point exists, skipped",
                "restore_point_failed" => "⚠️ Failed to create restore point (administrator required)",
                "settings_config_mgmt" => "Config Management",
                "config_dir_label" => "Config directory",
                "config_export_btn" => "Export Config",
                "config_import_btn" => "Import Config",
                "config_open_btn" => "Open Folder",
                "config_export_success" => "✅ Config exported to: {0}",
                "config_export_failed" => "⚠️ Export failed: {0}",
                "config_import_success" => "✅ Config imported ({0}). Restart to fully apply",
                "config_import_failed" => "⚠️ Import failed: {0}",
                "config_nothing_to_export" => "⚠️ Nothing to export yet",
                "config_no_config_found" => "⚠️ No config.json / apps.json found in the folder",
                "config_apps_hint" => "Place a custom apps.json here to override the built-in bloatware list",
                "setting_language" => "Interface language",
                "setting_language_current" => "Current: Simplified Chinese",
                "switch_to_english" => "Switch to English",
                "switch_to_chinese" => "Switch to 中文",
                // 按钮
                "scan" => "Scan",
                "delete" => "Delete",
                "select_all" => "Select All",
                "deselect_all" => "Deselect All",
                "select_safe" => "Clean",
                "expand_all" => "Expand All",
                "confirm_delete" => "Confirm Delete",
                "cancel" => "Cancel",
                // 磁盘信息
                "disk_used" => "Used",
                "disk_free" => "Free",
                "disk_total" => "Total",
                // 磁盘告警
                "disk_alert_notice" => "Disk space low: {1} GB free ({0}%), keep an eye on it",
                "disk_alert_warning" => "Disk space warning: {1} GB free ({0}%), cleanup recommended",
                "disk_alert_critical" => "Disk critically low! {1} GB free ({0}%), cleanup now",
                "disk_alert_clean_now" => "Clean Now",
                // 扫描状态
                "scanning" => "Scanning",
                "scanning_hint" => "Scanning disk for cleanable files, please wait...",
                "press_r_to_scan" => "Click Scan to start",
                "items_found" => "found",
                "items_selected" => "selected",
                "items" => "items",
                "total" => "total",
                "all_items" => "All",
                "window_title" => "Maclean - macOS Disk Cleaner",
                "back" => "Back",
                "home" => "Home",
                "delete_selected" => "Delete Selected",
                "delete_selected_count" => "Delete Selected ({0} items)",
                "items_total_size" => "{0} items, total {1}",
                "selected_count_size" => "{0} items selected, {1}",
                "found_items_total" => "{0} found items, total {1}",
                "logs" => "Logs",
                "no_large_files" => "No files larger than 1MB in this directory",
                "disk_category_overview" => "Category Breakdown",
                "others" => "Others",
                "rescan" => "Rescan",
                "analyzing" => "Analyzing {0}...",
                "home_dir_label" => "Home",
                "safe_clean" => "safe to clean",
                "cache_only_clean" => "cache only",
                "caution_clean" => "Needs Caution",
                "confirm_clean" => "Needs Confirm",
                // 列表
                "category" => "Category",
                "size" => "Size",
                "path" => "Path",
                "unknown" => "unknown",
                "no_items_hint" => "No items yet - click Scan to find cleanable files",
                "click_to_start" => "Click to start",
                // 状态页（设计稿 5.7：未扫描 与 扫描完成为空 是两种不同情绪）
                "empty_never_scanned" => "Not scanned yet",
                "empty_never_scanned_desc" => {
                    "Scanning only reads cache sizes — nothing is modified or deleted"
                }
                "empty_all_clean" => "This machine is clean",
                "empty_all_clean_desc" => "No cleanable items found",
                "empty_scan_timeout" => "Scan timed out",
                "cancel_scan" => "Cancel Scan",
                "cancelling_scan" => "Stopping…",
                "empty_scan_timeout_desc" => "Directory IO stalled and the scan was skipped; restart Mac and rescan",
                "empty_view_log" => "View scan log",
                "app_subitems_detail" => "Sub-items: {0}",
                "select_app_from_list" => "Please select an app from the left",
                // 概览
                "overview_releasable" => "Releasable Space",
                "overview_recommendation" => "Recommended Cleanup",
                // App 卸载
                "app_uninstall_subtitle" => "{0} apps, {1} releasable",
                "app_uninstall_search_placeholder" => "Search app name...",
                "app_list_title" => "App List",
                "subitem_detail_title" => "Sub-item Details",
                "current_selected" => "Current selected: {0}",
                "overview_recommendation_hint" => "Safe items from all categories, sorted by size",
                "overview_empty_title" => "Everything looks clean",
                "overview_empty_hint" => "Click the Scan button at the top right to find cleanable files",
                "scan_all" => "Scan All",
                "one_click_clean" => "Clean",
                "one_click_recommended_clean" => "Select Recommended",
                "app_list_title_with_count" => "App List ({0})",
                "selected_apps_partial" => "Selected items from {0} apps · {1}",
                "delete_with_size" => "🗑 Delete {0}",
                "all" => "All",
                "collapse_all" => "Collapse All",
                "expand_n_groups" => "Expand {0} groups",
                "collapse_n_groups" => "Collapse {0} groups",
                "badge_undeletable" => "🔒 Undeletable",
                // 过滤/搜索
                "filter" => "Filter",
                "filter_placeholder" => "Filter by path, category, or description... (/ to focus, Esc to clear)",
                "filter_results" => "{0} of {1} items matched",
                "no_match" => "No items match the filter",
                // 删除确认
                "about_to_delete" => "About to delete",
                "irreversible" => "This operation is irreversible!",
                "confirm_subtitle" => "Deleted files cannot be recovered. Please confirm.",
                "confirm_irreversible" => "Irreversible: files are deleted immediately and cannot be restored from Trash",
                "confirm_selected_items" => "Selected items",
                "confirm_protection_skipped" => "Protected items skipped (system / security / app data)",
                "confirm_releasable" => "Releasable space",
                "confirm_safe" => "Safe",
                "confirm_caution" => "Caution",
                "confirm_advanced" => "Advanced",
                "confirm_official_cmd" => "将执行官方 CLI 清理（范围由 Docker/OrbStack 决定，非逐项可恢复）:",
                "confirm_admin_required" => "Administrator privileges",
                "confirm_admin_yes" => "Required (contains root/SIP items)",
                "confirm_admin_no" => "Not required",
                "cleaning" => "Cleaning",
                "cleaning_in_progress" => "Cleaning in progress",
                "cleaning_log" => "Latest logs:",
                "progress_background_run" => "Run in background",
                "progress_authorize" => "Authorize and continue",
                "deleting_subtitle" => "Completed {0} / {1} items",
                "deleting_stop" => "Stop",
                "log_delete_cancelled" => "Deletion stopped; unprocessed items were kept",
                // 系统优化
                "optimize_click_to_scan" => "Click Scan to view available optimization tasks",
                "optimize_safe_hint" => "Optimization tasks are safe and will not affect system stability",
                "optimize_dns_cache_flush" => "Flush DNS Cache",
                "optimize_quicklook_rebuild" => "Rebuild QuickLook Thumbnails",
                "optimize_launchservices_rebuild" => "Rebuild LaunchServices",
                "optimize_saved_state_cleanup" => "Clean Saved States",
                "optimize_gatekeeper_cleanup" => "Clean Gatekeeper Records",
                "optimize_memory_pressure_release" => "Release Memory Pressure",
                "optimize_spotlight_reindex" => "Rebuild Spotlight Index",
                "optimize_login_items_audit" => "Audit Login Items",
                // 系统维护（P1，对标 MangoDisk system_maintenance）
                "optimize_icon_cache_rebuild" => "Rebuild Icon Cache",
                "optimize_finder_service_restart" => "Restart Finder Service",
                "optimize_audio_service_restart" => "Restart Audio Service",
                "optimize_legacy_overrides_clean" => "Clean Legacy Overrides",
                "optimize_user_permissions_repair" => "Repair User Permissions",
                "optimize_startup_disk_verify" => "Verify Startup Disk",
                "opt_icon_cache_success" => "Icon cache rebuilt",
                "opt_icon_cache_partial" => "Icon cache cleared (Finder refresh pending)",
                "opt_icon_cache_fail" => "Failed to rebuild icon cache",
                "opt_finder_restart_success" => "Finder restarted",
                "opt_finder_restart_fail" => "Failed to restart Finder",
                "opt_audio_restart_success" => "Audio service restarted",
                "opt_audio_restart_fail" => "Audio service restart requires administrator privileges; please retry after authorization",
                "opt_legacy_overrides_success" => "Legacy overrides cleared (backup kept)",
                "opt_legacy_overrides_no_backup" => "Legacy overrides cleared (backup failed)",
                "opt_legacy_overrides_empty" => "No legacy overrides found",
                "opt_legacy_overrides_fail" => "Failed to clear legacy overrides",
                "opt_permissions_repair_success" => "User permissions repaired",
                "opt_permissions_repair_fail" => "Failed to repair user permissions",
                "opt_startup_disk_success" => "Startup disk verified, no issues found",
                "opt_startup_disk_fail" => "Startup disk verification failed or found issues",
                "optimize_confirm_title" => "High-risk maintenance task",
                "optimize_confirm_body" => "This task changes user-level settings and may take a while. Continue?",
                "optimize_confirm_yes" => "Continue",
                "optimize_confirm_no" => "Cancel",
                "optimize_running" => "Running…",
                "optimize_busy" => "Another task is already running",
                "optimize_aborted" => "⚠ Optimization task exited abnormally",
                // Windows 优化任务
                "optimize_win_dns_flush" => "Flush DNS Cache",
                "optimize_win_temp_cleanup" => "Clean Temp Files",
                "optimize_win_disable_telemetry" => "Disable Telemetry",
                "optimize_win_disable_copilot" => "Disable Copilot",
                "optimize_win_disable_suggestions" => "Disable Ads & Suggestions",
                "optimize_win_startup_audit" => "Audit Startup Items",
                "optimize_win_disable_fast_startup" => "Disable Fast Startup",
                "optimize_win_restore_point" => "Create Restore Point",
                "optimize_win_restart_explorer" => "Restart Explorer",
                "optimize_win_trim_drives" => "Optimize Drives (TRIM/Defrag)",
                "optimize_logs" => "📋 Optimize Logs",
                // 优化任务执行结果
                "opt_dns_success" => "✅ DNS cache flushed",
                "opt_dns_fail" => "⚠️ DNS flush requires admin privileges. Run in Terminal: sudo dscacheutil -flushcache && sudo killall -HUP mDNSResponder",
                "opt_quicklook_success" => "✅ QuickLook thumbnail cache rebuilt",
                "opt_quicklook_fail" => "⚠️ QuickLook cache rebuild failed",
                "opt_launchservices_success" => "✅ LaunchServices database rebuilt",
                "opt_launchservices_fail" => "⚠️ LaunchServices rebuild failed",
                "opt_saved_state_success" => "✅ Cleaned {0} old saved application states",
                "opt_gatekeeper_success" => "✅ Gatekeeper records cleaned",
                "opt_gatekeeper_fail" => "⚠️ Gatekeeper cleanup failed",
                "opt_gatekeeper_empty" => "✅ Gatekeeper records already empty",
                "opt_memory_success" => "✅ Inactive memory released",
                "opt_memory_fail" => "⚠️ Memory release requires admin privileges. Run in Terminal: sudo purge",
                "opt_spotlight_success" => "✅ Spotlight index rebuild started (may take a few minutes)",
                "opt_spotlight_fail" => "⚠️ Spotlight reindex requires admin privileges. Run in Terminal: sudo mdutil -E /",
                "opt_login_items_opened" => "✅ Opened System Settings > Login Items",
                "opt_login_items_fail" => "⚠️ Failed to open Login Items settings",
                // Windows 优化任务结果
                "opt_win_dns_success" => "✅ DNS cache flushed",
                "opt_win_dns_fail" => "⚠️ DNS flush failed. Run in CMD: ipconfig /flushdns",
                "opt_win_temp_success" => "✅ Cleaned {0} of temp files",
                "opt_win_temp_fail" => "⚠️ Temp file cleanup failed",
                "opt_win_telemetry_success" => "✅ Telemetry disabled (registry policy set)",
                "opt_win_telemetry_fail" => "⚠️ Failed to disable telemetry. Run as Administrator",
                "opt_win_copilot_success" => "✅ Copilot disabled (registry policy set)",
                "opt_win_copilot_fail" => "⚠️ Failed to disable Copilot. Run as Administrator",
                "opt_win_suggestions_success" => "✅ Ads & suggestions disabled",
                "opt_win_suggestions_fail" => "⚠️ Failed to disable suggestions. Run as Administrator",
                "opt_win_startup_opened" => "✅ Opened Task Manager > Startup",
                "opt_win_startup_fail" => "⚠️ Failed to open Task Manager",
                "opt_win_faststartup_success" => "✅ Fast startup disabled",
                "opt_win_faststartup_fail" => "⚠️ Failed to disable fast startup. Run as Administrator",
                "opt_win_restore_success" => "✅ System restore point created",
                "opt_win_restore_fail" => "⚠️ Failed to create restore point. Run as Administrator",
                "opt_win_explorer_success" => "✅ Explorer restarted",
                "opt_win_explorer_fail" => "⚠️ Failed to restart Explorer",
                "opt_win_trim_success" => "✅ Drive optimization started",
                "opt_win_trim_fail" => "⚠️ Drive optimization failed. Run as Administrator",
                "opt_unknown" => "⚠️ Unknown optimization task: {0}",
                // 权限引导
                "permission_title" => "Permission Settings",
                "permission_headline" => "Grant Full Disk Access",
                "permission_desc" => "Maclean needs Full Disk Access permission to delete developer cache files.",
                "permission_sub_desc" => "Some cache files are created by root with macOS security attributes; without this permission they cannot be deleted.",
                "permission_steps_title" => "Please follow these steps:",
                "permission_step1" => "Click the \"Open System Settings\" button below",
                "permission_step2" => "Find Maclean in the Full Disk Access list",
                "permission_step3" => "If missing, click the + button to add Maclean.app",
                "permission_step4" => "Make sure the switch next to Maclean is turned on",
                "permission_step5" => "Restart Maclean to use all deletion features",
                "permission_open_settings" => "Open System Settings",
                "permission_install_app" => "Install to Applications",
                "permission_later" => "Later",
                "permission_hint" => "Hint: Restart Maclean after authorization to use all deletion features",
                // 删除确认
                "confirm_fda_warning" => "Full Disk Access not granted",
                "confirm_fda_sub_warning" => "Some files may not be deleted; authorization is recommended",
                "confirm_grant" => "Go to Settings",
                "confirm_preview" => "Preview items to delete",
                "confirm_preview_summary" => "{} items total, cache permanently deleted, large files moved to Trash",
                // sudo 密码弹窗
                "sudo_title_setup" => "Enable Touch ID",
                "sudo_title_delete" => "Administrator Password Required",
                "sudo_desc_setup" => "Enter your administrator password once to create /etc/pam.d/sudo_local.\nAfter enabling, you can use Touch ID to authenticate future deletions.",
                "sudo_desc_delete" => "{} items need administrator privileges to continue deletion.",
                "sudo_password_hint" => "Enter administrator password...",
                "sudo_password_note" => "Password is only used for this sudo authorization and will not be saved to Keychain.",
                "sudo_confirm_setup" => "Enable",
                "sudo_confirm_delete" => "Confirm Delete",
                "sudo_cancelled_log" => "Authorization cancelled by user",
                // Touch ID 启用提示
                "touchid_setup_title" => "Enable Touch ID",
                "touchid_setup_headline" => "Use Touch ID instead of password",
                "touchid_setup_desc" => "{} items need administrator privileges to delete.",
                "touchid_setup_detail" => "Enabling will create the /etc/pam.d/sudo_local config file (Apple recommended),\nso all future admin operations can use Touch ID instead of a password.\nThis is a one-time setup and persists after macOS updates.",
                "touchid_enable" => "Enable Touch ID",
                "touchid_use_password" => "Use Password",
                // Touch ID 等待
                "touchid_wait_title" => "Waiting for Touch ID Setup",
                "touchid_wait_headline" => "Please enter your password in the system dialog",
                "touchid_wait_desc" => "A password dialog will appear. Enter your administrator password\nto create the /etc/pam.d/sudo_local configuration file.\nDeletion will continue automatically after completion.",
                "touchid_wait_time" => "Waited {} seconds (timeout 120s)",
                "touchid_wait_cancel" => "Cancel, use password instead",
                // Touch ID 删除中
                "touchid_verify_title" => "Touch ID Verification",
                "touchid_verify_headline" => "Please authenticate with Touch ID",
                "touchid_verify_desc" => "Deleting {} items that require administrator privileges...",
                "touchid_verify_hint" => "A Touch ID dialog will appear; touch the fingerprint sensor.",
                "touchid_verify_log" => "Recent logs:",
                "touchid_cancelled" => "Touch ID authorization cancelled",
                "touchid_clamshell_error" => "Screen is closed, Touch ID is unavailable, please enter password",
                "touchid_timeout_error" => "Operation timed out: Touch ID setup not detected, please retry or use password",
                // 删除中
                "deleting_sudo_phase" => "Deleting with administrator privileges...",
                // 删除完成汇总
                "summary_title" => "Cleanup Result",
                "summary_success" => "Successfully deleted {} items",
                "summary_fail" => "Failed to delete {} items",
                "summary_fail_hint" => "Some files could not be deleted due to permissions or system protection. See logs above.",
                // M-2：{} = 可还原项数 / 总项数。两个数字都给，是因为只说
                // "已备份"会让人以为全都能还原 —— 永久删除的字节回不来。
                "summary_backup" => "Deletion log saved ({} of {} items can be restored from Trash)",
                "summary_backup_none" => "Deletion log saved — none of these items are recoverable (they were permanently deleted)",
                "summary_solution_title" => "Tips: Failure reasons and solutions",
                "summary_sip_tip" => "SIP/System protection: Paths like /Library/Developer/CoreSimulator are protected by macOS SIP and cannot be deleted with any privilege. Such paths are marked as undeletable during scanning — there is no need to disable SIP.",
                "summary_perm_tip" => "Permission denied: Some files are owned by root or inside protected directories. Click \"Retry deletion (admin authorization)\" below and the app requests authorization automatically — no manual commands needed.",
                "summary_solution_1" => "1. Click \"Retry deletion (admin authorization)\" below: the app authorizes and retries automatically",
                "summary_solution_2" => "2. SIP-protected system paths cannot be deleted — the app never removes macOS system files",
                "summary_solution_3" => "3. Items still failing: uncheck them in the tab and continue, or check whether the file is currently in use",
                "summary_open_settings" => "Open System Settings",
                "summary_fail_list" => "Failed list:",
                "summary_copy_paths" => "Copy Paths",
                "summary_copy_sudo" => "Copy sudo Command",
                "summary_retry_delete" => "Retry deletion (admin authorization)",
                "log_protected_root_rejected" => "Protected system root — deletion refused: {}",
                "summary_retry_hint" => "maclean will retry with administrator privileges automatically — no manual commands needed.",
                "fail_reason_perm" => "Permission denied",
                "fail_reason_sip" => "Protected by SIP/system",
                "fail_reason_inuse" => "In use by another process",
                "fail_reason_immutable" => "File is locked (immutable flag)",
                "fail_reason_other" => "Other reason",
                "summary_free_space" => "Available space: {}",
                "summary_ok" => "OK",
                "finish_summary" => "Cleanup completed: {} succeeded, {} failed",
                // 后台日志
                "log_intercepted" => "Blocked: {} - {}",
                "log_skipped" => "Skipped: {} - {}",
                "log_deleting" => "Deleting: {}",
                "log_deleted" => "Deleted [{}] {} (success {} / fail {})",
                "log_deleted_sudo" => "Deleted [{}] {} (admin privileges)",
                "log_deleted_touchid" => "Deleted [{}] {} (Touch ID)",
                "log_delete_failed" => "Delete failed: {} - {}",
                "log_snapshot_deleted" => "Deleted snapshot: {}",
                "log_runtime_deleted" => "Deleted runtime: {}",
                "log_action_trashed" => "Moved to Trash",
                "log_action_deleted" => "Deleted",
                "log_xcrun_failed" => "xcrun deletion failed, will try sudo: {}",
                "log_skip_running" => "Skipped [{}] Xcode/Simulator is running",
                "log_docker_failed" => "Docker cleanup failed: {}",
                "log_path_not_exist" => "Path does not exist: {}",
                "already_cleaned" => "already cleaned",
                "log_symlink_rejected" => "Refuse to delete symlink: {}",
                "log_need_sudo" => "{} items need administrator privileges",
                "log_sudo_phase" => "Deleting with administrator privileges...",
                "log_password_wrong" => "Administrator password incorrect, please re-enter",
                "log_trash_failed" => "Could not move to Trash (grant Automation permission in System Settings), kept as-is: {}",
                // M-3 一键卸载
                "uninstall_invalid_path" => "This path is not an uninstallable app",
                "uninstall_no_bundle" => "Cannot read the app bundle identifier",
                "uninstall_blocked_critical" => "System-critical app — uninstall blocked",
                "uninstall_blocked_official" => "Protected app — use the {0} official uninstaller",
                "uninstall_delegated" => "Official uninstaller launched: {0}. Finish the remaining steps there.",
                "uninstall_nothing" => "Nothing to delete for this app",
                "uninstall_done" => "Uninstalled {0}: {1} item(s) moved to Trash",
                "uninstall_partial" => "Uninstalled {0}: {1} item(s) blocked or failed",
                "log_sudo_rejected" => "Blocked by safety check before sudo deletion: {} — {}",
                "log_sudo_symlink_rejected" => "Blocked before sudo deletion (path became a symlink): {}",
                "log_sudo_unsafe_path" => "Blocked before sudo deletion (path contains unsafe characters): {}",
                "log_exit_code" => "sudo exit code {}",
                "log_no_sim_runtimes" => "No installed simulator runtimes found",
                "log_mount_in_use" => "Simulator runtime is mounted in use, skipping",
                "log_cannot_get_mount" => "Cannot get mount point info, skipping for safety",
                "log_sim_deleted" => "Deleted {} simulator runtime(s) via xcrun simctl{}",
                "log_sim_failed_suffix" => ", {} failed",
                "log_docker_not_running" => "Docker daemon is not running, please start Docker Desktop",
                "log_xcrun_done" => "xcrun completed: {}",
                "log_touchid_verifying" => "Touch ID verifying, please authenticate...",
                "log_wait_touchid" => "Waiting for Touch ID authorization to execute xcrun...",
                "log_touchid_prepare_xcrun" => "Preparing xcrun deletion: {}",
                "log_touchid_xcrun_deleted" => "Deleted {} simulator runtime images via xcrun simctl",
                "log_touchid_runtime_deleted_path" => "Deleted simulator runtime image via xcrun simctl: {}",
                "log_sudo_execute" => "Preparing sudo deletion for {} residual items...",
                "log_sudo_done2" => "sudo deletion completed, parsing results...",
                "log_no_sudo_needed" => "No sudo needed, all completed via xcrun",
                "log_touchid_cancel" => "Touch ID cancelled",
                "log_sip_protected" => "SIP protection, cannot delete: {}",
                "log_still_exists" => "Still exists after admin deletion",
                "log_sudo_failed" => "Failed to start sudo: {} - {}",
                "log_cannot_start_sudo" => "Cannot start sudo: {} - {}",
                "log_docker_done" => "Docker cleanup completed, reclaimed space: {}",
                "log_menu_event" => "Menu bar event: {}",
                "log_quickclean_start" => "QuickClean: {} safe items selected, start deletion",
                "log_quickclean_none" => "QuickClean: no deletable safe items",
                "log_keepalive_failed" => "sudo keepalive startup failed: {}",
                "log_unknown" => "Unknown",
                "log_cancelled_auth" => "Authorization cancelled: {}",
                "log_cancel_reason" => "Authorization cancelled",
                "log_sip_reason" => "SIP protection or system restriction",
                "log_acl_protected" => "System protection (ACL rule forbids deletion, no privilege can remove it): {}",
                "summary_close" => "Close",
                // 关联文件标签
                "assoc_app" => "App Bundle",
                "assoc_container" => "App Container",
                "assoc_group" => "Shared Container",
                "assoc_cookie" => "Cookies",
                "assoc_webkit" => "WebKit Data",
                "assoc_script" => "App Scripts",
                "assoc_metadata" => "Metadata",
                "assoc_cache" => "Cache",
                "assoc_app_data" => "App Data",
                "assoc_prefs" => "Preferences",
                "assoc_logs" => "Logs",
                "assoc_saved_state" => "Saved State",
                "assoc_http_storage" => "HTTP Storage",
                "assoc_other" => "Other",
                _ => "",
            }
    } else {
        match key {
                // Tab 标题
                "tab_overview" => "概览",
                "tab_dev_cache" => "开发者缓存",
                "tab_large_files" => "大文件",
                "tab_app_cache" => "App缓存",
                "tab_app_data" => "App数据",
                "tab_app_uninstall" => "应用卸载",
                "tab_system_optimize" => "系统优化",
                "tab_apfs" => "APFS快照",
                "tab_custom_rules" => "自定义规则",
                "tab_dup_files" => "重复文件",
                "tab_startup_items" => "启动项",
                "startup_hint" => "登录时自启的 LaunchAgent / LaunchDaemon。禁用会把 plist 移入备份目录（可逆），不会强停已在运行的服务。",
                "startup_refresh" => "刷新",
                "startup_refreshed" => "启动项已刷新",
                "startup_count" => "{0} 项，已加载 {1}",
                "startup_enabled" => "已加载",
                "startup_disabled" => "已禁用",
                "startup_disable" => "禁用",
                "startup_enable" => "启用",
                "startup_disabled_log" => "已禁用 {0} → 移入 {1}",
                "startup_enabled_log" => "已启用 {0} ← 自 {1} 恢复",
                "startup_disable_fail" => "禁用失败",
                "startup_enable_fail" => "启用失败",
                "disk_view_list" => "列表",
                "disk_view_tree" => "树图",
                "tab_settings" => "设置",
                "settings_appearance" => "外观",
                "setting_dark_mode" => "深色模式",
                "setting_dark_mode_desc" => "切换整套界面的浅色 / 深色配色",
                "settings_general" => "通用",
                "settings_safety" => "安全",
                "settings_language" => "语言",
                "setting_menubar_icon" => "启动时显示菜单栏图标",
                "setting_menubar_icon_desc" => "在 macOS 菜单栏常驻磁盘用量指示器",
                "setting_keep_sudo" => "自动保持 sudo 会话",
                "setting_keep_sudo_desc" => "避免重复输入管理员密码",
                "setting_scan_cache" => "扫描结果本地缓存",
                "setting_scan_cache_desc" => "7 天内避免重复扫描，加速启动",
                "setting_scan_all_disks" => "扫描全部磁盘",
                "setting_scan_all_disks_desc" => "默认只扫 C 盘；开启后同时扫描 D/E 等盘符的 Program Files",
                // M-1
                "setting_official_uninstaller" => "优先使用应用自带的官方卸载器",
                "setting_official_uninstaller_desc" => "直接删 .app 目录会留下 launchd 任务与 pkgutil 收据，官方卸载器会一并清掉",
                "confirm_official_uninstaller_hint" => "交由应用自带官方卸载器处理（会弹出独立窗口）",
                "log_official_uninstaller" => "已交由官方卸载器",
                // C-1 · 付费分级
                "tier_free" => "免费版",
                "tier_pro" => "Pro 版",
                "tier_capability_unlimited" => "清理额度无上限",
                "tier_capability_scheduled" => "定时清理",
                "tier_pro_includes" => "Pro 版增加：{}",
                // C-3 · 定时清理
                "settings_schedule" => "定时清理",
                "setting_schedule_enable" => "自动执行清理",
                "setting_schedule_enable_desc" => "注册系统级任务，应用未运行时也会按时清理",
                "setting_schedule_interval" => "执行间隔",
                "schedule_every_day" => "每天",
                "schedule_every_week" => "每周",
                "schedule_every_month" => "每月",
                "schedule_last_run" => "上次执行：{}",
                "schedule_never_run" => "尚未执行过",
                "schedule_pro_only" => "定时清理为 Pro 版功能",
                "schedule_installed" => "已注册定时任务",
                "schedule_install_failed" => "注册定时任务失败：{}",
                "schedule_removed" => "已移除定时任务",
                "schedule_remove_failed" => "移除定时任务失败：{}",
                "setting_confirm_advanced" => "删除前二次确认",
                "setting_confirm_advanced_desc" => "Advanced 项目必须手动确认",
                "setting_show_protected_items" => "显示受保护/系统项",
                "setting_show_protected_items_desc" => "默认隐藏受系统访问控制保护、任何权限都无法删除的项，避免反复授权删除失败；开启后置灰显示，便于排查",
                "setting_prevent_lid_close" => "合盖时禁止删除",
                "setting_prevent_lid_close_desc" => "检测 MacBook 合盖状态，防止误触",
                "setting_auto_restore_point" => "操作前自动创建系统还原点",
                "setting_auto_restore_point_desc" => "清理/优化前自动备份系统状态（Windows）",
                "setting_restore_last" => "还原上次注册表修改",
                "setting_restore_last_desc" => "重新导入优化前自动备份的注册表文件。仅恢复被覆盖的旧值，新增的键值需手动删除",
                "restore_last_btn" => "还原",
                "restore_last_none" => "未找到注册表备份",
                "restore_last_success" => "✅ 已还原: {0}",
                "restore_last_failed" => "⚠️ 还原失败（可能需要管理员权限）",
                "restore_last_backup_info" => "最近备份: {0}（{1}，{2} 个键）",
                "restore_point_created" => "✅ 系统还原点已创建",
                "restore_point_skipped" => "⏭️ 近期已有还原点，跳过创建",
                "restore_point_failed" => "⚠️ 还原点创建失败（需管理员权限）",
                "settings_config_mgmt" => "配置管理",
                "config_dir_label" => "配置目录",
                "config_export_btn" => "导出配置",
                "config_import_btn" => "导入配置",
                "config_open_btn" => "打开文件夹",
                "config_export_success" => "✅ 配置已导出到: {0}",
                "config_export_failed" => "⚠️ 导出失败: {0}",
                "config_import_success" => "✅ 配置已导入（{0}），重启后完全生效",
                "config_import_failed" => "⚠️ 导入失败: {0}",
                "config_nothing_to_export" => "⚠️ 暂无可导出的配置",
                "config_no_config_found" => "⚠️ 该文件夹中未找到 config.json / apps.json",
                "config_apps_hint" => "将自定义 apps.json 放入配置目录可覆盖内置应用列表",
                "setting_language" => "界面语言",
                "setting_language_current" => "当前：简体中文",
                "switch_to_english" => "切换 English",
                "switch_to_chinese" => "切换 中文",
                // 按钮
                "scan" => "扫描",
                "delete" => "删除",
                "select_all" => "全选",
                "deselect_all" => "取消全选",
                "select_safe" => "一键选择清理",
                "expand_all" => "展开全部",
                "confirm_delete" => "确认删除",
                "cancel" => "取消",
                // 磁盘信息
                "disk_used" => "已用",
                "disk_free" => "可用",
                "disk_total" => "总量",
                // 磁盘告警
                "disk_alert_notice" => "磁盘空间注意：剩余 {1} GB ({0}%)，建议关注",
                "disk_alert_warning" => "磁盘空间警告：剩余 {1} GB ({0}%)，建议立即清理",
                "disk_alert_critical" => "磁盘空间严重不足！剩余 {1} GB ({0}%)，请立即清理",
                "disk_alert_clean_now" => "立即清理",
                // 扫描状态
                "scanning" => "扫描中",
                "scanning_hint" => "正在扫描磁盘上的可清理文件，请稍候...",
                "press_r_to_scan" => "点击「扫描」开始",
                "items_found" => "找到",
                "items_selected" => "已选",
                "items" => "个子项",
                "total" => "共计",
                "all_items" => "全部",
                "window_title" => "Maclean - macOS 磁盘清理",
                "back" => "返回",
                "home" => "主目录",
                "delete_selected" => "删除选中",
                "delete_selected_count" => "删除选中 ({0}项)",
                "items_total_size" => "{0} 项, 总计 {1}",
                "disk_category_overview" => "分类占比总览",
                "others" => "其他",
                "selected_count_size" => "已选 {0} 项, {1}",
                "found_items_total" => "{0} 找到项, 共计 {1}",
                "logs" => "日志",
                "safe_clean" => "可安全清理",
                "cache_only_clean" => "仅缓存可清",
                "caution_clean" => "需谨慎处理",
                "confirm_clean" => "需确认删除",
                // 列表
                "category" => "类别",
                "size" => "大小",
                "path" => "路径",
                "unknown" => "未知",
                "no_items_hint" => "暂无数据 - 点击「扫描」查找可清理文件",
                "click_to_start" => "点击开始",
                // 状态页（设计稿 5.7：未扫描 与 扫描完成为空 是两种不同情绪）
                "empty_never_scanned" => "还没有扫描过",
                "empty_never_scanned_desc" => "扫描只会读取缓存目录大小，不会修改或删除任何文件",
                "empty_all_clean" => "这台机器很干净",
                "empty_all_clean_desc" => "未发现可清理的项目",
                "empty_scan_timeout" => "扫描超时，未完成",
                "cancel_scan" => "取消扫描",
                "cancelling_scan" => "正在停止…",
                "empty_scan_timeout_desc" => "目录 IO 异常导致扫描被跳过，可重启 Mac 后重新扫描",
                "empty_view_log" => "查看扫描日志",
                "app_subitems_detail" => "子项详情：{0}",
                "select_app_from_list" => "请从左侧选择一个应用",
                // 概览
                "overview_releasable" => "可释放空间",
                "overview_recommendation" => "推荐清理",
                // App 卸载
                "app_uninstall_subtitle" => "{0} 个应用 · 共 {1} 可释放",
                "app_uninstall_search_placeholder" => "搜索应用名称...",
                "app_list_title" => "应用列表",
                "subitem_detail_title" => "子项详情",
                "current_selected" => "当前选中：{0}",
                "overview_recommendation_hint" => "聚合所有分类中的安全项，按大小排序",
                "overview_empty_title" => "看起来一切整洁",
                "overview_empty_hint" => "点击右上角「扫描」查找可清理文件",
                "scan_all" => "扫描全部",
                "one_click_clean" => "一键清理",
                "one_click_recommended_clean" => "一键选推荐清理",
                "app_list_title_with_count" => "应用列表（{0}）",
                "selected_apps_partial" => "已选中 {0} 个应用的部分项目 · 可释放 {1}",
                "delete_with_size" => "🗑 删除 {0}",
                "all" => "全部",
                "collapse_all" => "收起全部",
                "expand_n_groups" => "展开 {0} 个应用分组",
                "collapse_n_groups" => "收起 {0} 个应用分组",
                "badge_undeletable" => "🔒 不可删除",
                // 过滤/搜索
                "filter" => "过滤",
                "filter_placeholder" => "按路径、类别或描述过滤...（/ 聚焦，Esc 清除）",
                "filter_results" => "匹配 {0} / {1} 项",
                "no_match" => "没有匹配过滤条件的项",
                // 删除确认
                "about_to_delete" => "即将删除",
                "irreversible" => "此操作不可逆！",
                "confirm_subtitle" => "删除后文件将不可恢复，请确认。",
                "confirm_irreversible" => "不可逆操作：确认后立即删除，无法从废纸篓恢复",
                "confirm_selected_items" => "选中项目",
                "confirm_protection_skipped" => "已跳过受保护项（系统 / 安全软件 / 应用数据）",
                "confirm_releasable" => "预计释放",
                "confirm_safe" => "Safe",
                "confirm_caution" => "Caution",
                "confirm_advanced" => "Advanced",
                "confirm_official_cmd" => "将执行官方 CLI 清理（范围由 Docker/OrbStack 决定，非逐项可恢复）:",
                "confirm_admin_required" => "管理员权限",
                "confirm_admin_yes" => "需要（含 root/SIP 项目）",
                "confirm_admin_no" => "不需要",
                "cleaning" => "清理中",
                "cleaning_in_progress" => "正在执行清理",
                "cleaning_log" => "最新日志：",
                "progress_background_run" => "后台运行",
                "progress_authorize" => "授权并继续",
                "deleting_subtitle" => "已完成 {0} / {1} 项",
                "deleting_stop" => "停止",
                "log_delete_cancelled" => "删除已停止，未处理项已保留",
                // 系统优化
                "optimize_click_to_scan" => "点击扫描查看可用的优化任务",
                "optimize_safe_hint" => "优化任务安全可执行，不会影响系统稳定性",
                "optimize_dns_cache_flush" => "DNS 缓存刷新",
                "optimize_quicklook_rebuild" => "QuickLook 缩略图重建",
                "optimize_launchservices_rebuild" => "LaunchServices 重建",
                "optimize_saved_state_cleanup" => "Saved State 清理",
                "optimize_gatekeeper_cleanup" => "Gatekeeper 下载清理",
                "optimize_memory_pressure_release" => "内存压力释放",
                "optimize_spotlight_reindex" => "Spotlight 索引重建",
                "optimize_login_items_audit" => "登录项审计",
                // 系统维护（P1，对标 MangoDisk system_maintenance）
                "optimize_icon_cache_rebuild" => "图标缓存重建",
                "optimize_finder_service_restart" => "重启 Finder 服务",
                "optimize_audio_service_restart" => "重启音频服务",
                "optimize_legacy_overrides_clean" => "清理旧版打开方式覆盖",
                "optimize_user_permissions_repair" => "修复用户目录权限",
                "optimize_startup_disk_verify" => "校验启动盘",
                "opt_icon_cache_success" => "图标缓存已重建",
                "opt_icon_cache_partial" => "图标缓存已清除（Finder 待刷新）",
                "opt_icon_cache_fail" => "图标缓存重建失败",
                "opt_finder_restart_success" => "Finder 已重启",
                "opt_finder_restart_fail" => "Finder 重启失败",
                "opt_audio_restart_success" => "音频服务已重启",
                "opt_audio_restart_fail" => "音频服务重启需要管理员权限，授权后重试即可",
                "opt_legacy_overrides_success" => "旧版打开方式覆盖已清理（已备份）",
                "opt_legacy_overrides_no_backup" => "旧版打开方式覆盖已清理（备份失败）",
                "opt_legacy_overrides_empty" => "未发现旧版打开方式覆盖",
                "opt_legacy_overrides_fail" => "旧版打开方式覆盖清理失败",
                "opt_permissions_repair_success" => "用户目录权限已修复",
                "opt_permissions_repair_fail" => "用户目录权限修复失败",
                "opt_startup_disk_success" => "启动盘校验完成，未发现问题",
                "opt_startup_disk_fail" => "启动盘校验失败或发现问题",
                "optimize_confirm_title" => "高风险维护任务",
                "optimize_confirm_body" => "该任务会修改用户级设置，可能需要一段时间。是否继续？",
                "optimize_confirm_yes" => "继续",
                "optimize_confirm_no" => "取消",
                "optimize_running" => "执行中…",
                "optimize_busy" => "已有任务正在执行，请稍候",
                "optimize_aborted" => "⚠ 优化任务异常退出",
                // Windows 优化任务
                "optimize_win_dns_flush" => "DNS 缓存刷新",
                "optimize_win_temp_cleanup" => "临时文件清理",
                "optimize_win_disable_telemetry" => "关闭遥测",
                "optimize_win_disable_copilot" => "禁用 Copilot",
                "optimize_win_disable_suggestions" => "关闭广告与建议",
                "optimize_win_startup_audit" => "启动项审计",
                "optimize_win_disable_fast_startup" => "关闭快速启动",
                "optimize_win_restore_point" => "创建系统还原点",
                "optimize_win_restart_explorer" => "重启资源管理器",
                "optimize_win_trim_drives" => "驱动器优化 (TRIM/碎片整理)",
                "optimize_logs" => "📋 优化日志",
                // 优化任务执行结果
                "opt_dns_success" => "✅ DNS 缓存已刷新",
                "opt_dns_fail" => "⚠️ DNS 缓存刷新需要管理员权限。请在终端执行：sudo dscacheutil -flushcache && sudo killall -HUP mDNSResponder",
                "opt_quicklook_success" => "✅ QuickLook 缩略图缓存已重建",
                "opt_quicklook_fail" => "⚠️ QuickLook 缓存重建失败",
                "opt_launchservices_success" => "✅ LaunchServices 数据库已重建",
                "opt_launchservices_fail" => "⚠️ LaunchServices 重建失败",
                "opt_saved_state_success" => "✅ 已清理 {0} 个旧的应用保存状态",
                "opt_gatekeeper_success" => "✅ Gatekeeper 下载记录已清理",
                "opt_gatekeeper_fail" => "⚠️ Gatekeeper 清理失败",
                "opt_gatekeeper_empty" => "✅ Gatekeeper 下载记录已为空",
                "opt_memory_success" => "✅ 已释放非活跃内存",
                "opt_memory_fail" => "⚠️ 内存释放需要管理员权限。请在终端执行：sudo purge",
                "opt_spotlight_success" => "✅ Spotlight 索引重建已启动（可能需要几分钟）",
                "opt_spotlight_fail" => "⚠️ Spotlight 重建需要管理员权限。请在终端执行：sudo mdutil -E /",
                "opt_login_items_opened" => "✅ 已打开系统设置 > 登录项",
                "opt_login_items_fail" => "⚠️ 无法打开登录项设置",
                // Windows 优化任务结果
                "opt_win_dns_success" => "✅ DNS 缓存已刷新",
                "opt_win_dns_fail" => "⚠️ DNS 刷新失败。请在 CMD 执行：ipconfig /flushdns",
                "opt_win_temp_success" => "✅ 已清理 {0} 临时文件",
                "opt_win_temp_fail" => "⚠️ 临时文件清理失败",
                "opt_win_telemetry_success" => "✅ 遥测已关闭（注册表策略已设置）",
                "opt_win_telemetry_fail" => "⚠️ 关闭遥测失败，请以管理员身份运行",
                "opt_win_copilot_success" => "✅ Copilot 已禁用（注册表策略已设置）",
                "opt_win_copilot_fail" => "⚠️ 禁用 Copilot 失败，请以管理员身份运行",
                "opt_win_suggestions_success" => "✅ 广告与建议已关闭",
                "opt_win_suggestions_fail" => "⚠️ 关闭建议失败，请以管理员身份运行",
                "opt_win_startup_opened" => "✅ 已打开任务管理器 > 启动",
                "opt_win_startup_fail" => "⚠️ 无法打开任务管理器",
                "opt_win_faststartup_success" => "✅ 快速启动已关闭",
                "opt_win_faststartup_fail" => "⚠️ 关闭快速启动失败，请以管理员身份运行",
                "opt_win_restore_success" => "✅ 系统还原点已创建",
                "opt_win_restore_fail" => "⚠️ 创建还原点失败，请以管理员身份运行",
                "opt_win_explorer_success" => "✅ 资源管理器已重启",
                "opt_win_explorer_fail" => "⚠️ 重启资源管理器失败",
                "opt_win_trim_success" => "✅ 驱动器优化已启动",
                "opt_win_trim_fail" => "⚠️ 驱动器优化失败，请以管理员身份运行",
                "opt_unknown" => "⚠️ 未知的优化任务: {0}",
                // 权限引导
                "permission_title" => "权限设置",
                "permission_headline" => "授权完全磁盘访问",
                "permission_desc" => "Maclean 需要完全磁盘访问权限才能删除开发者缓存文件。",
                "permission_sub_desc" => "部分缓存文件由 root 创建且带有 macOS 安全属性，没有此权限将无法删除。",
                "permission_steps_title" => "请按以下步骤操作：",
                "permission_step1" => "点击下方「打开系统设置」按钮",
                "permission_step2" => "在「完全磁盘访问」列表中找到 Maclean",
                "permission_step3" => "如果没有，点击 + 号添加 Maclean.app",
                "permission_step4" => "确保 Maclean 旁边的开关已打开",
                "permission_step5" => "重启 Maclean 后即可正常删除",
                "permission_open_settings" => "打开系统设置",
                "permission_install_app" => "安装到应用程序",
                "permission_later" => "稍后再说",
                "permission_hint" => "提示: 授权后重启 Maclean 即可正常使用所有删除功能",
                // 删除确认
                "confirm_fda_warning" => "未授予完全磁盘访问权限",
                "confirm_fda_sub_warning" => "部分文件可能无法删除，建议先授权",
                "confirm_grant" => "去授权",
                "confirm_preview" => "预览删除项",
                "confirm_preview_summary" => "共 {} 项, 缓存类永久删除, 大文件移至废纸篓",
                // sudo 密码弹窗
                "sudo_title_setup" => "启用 Touch ID",
                "sudo_title_delete" => "需要管理员权限",
                "sudo_desc_setup" => "首次启用 Touch ID 需要输入一次管理员密码，以创建 /etc/pam.d/sudo_local。\n启用后，后续删除操作可使用 Touch ID 验证。",
                "sudo_desc_delete" => "{} 项文件因权限不足需要输入管理员密码继续删除。",
                "sudo_password_hint" => "请输入管理员密码...",
                "sudo_password_note" => "密码仅用于本次 sudo 授权，不会保存到钥匙串。",
                "sudo_confirm_setup" => "确认启用",
                "sudo_confirm_delete" => "确认删除",
                "sudo_cancelled_log" => "用户取消密码授权",
                // Touch ID 启用提示
                "touchid_setup_title" => "启用 Touch ID",
                "touchid_setup_headline" => "使用 Touch ID 代替密码",
                "touchid_setup_desc" => "{} 项文件需要管理员权限删除。",
                "touchid_setup_detail" => "启用后会创建 /etc/pam.d/sudo_local 配置文件（macOS 官方推荐方式），\n之后所有管理员操作都可以用 Touch ID 验证，无需输入密码。\n这是一次性操作，系统更新后依然有效。",
                "touchid_enable" => "启用 Touch ID",
                "touchid_use_password" => "用密码代替",
                // Touch ID 等待
                "touchid_wait_title" => "等待 Touch ID 启用",
                "touchid_wait_headline" => "请在系统弹窗中输入密码",
                "touchid_wait_desc" => "系统会弹出密码对话框，请输入管理员密码\n以创建 /etc/pam.d/sudo_local 配置文件。\n完成后会自动继续删除操作。",
                "touchid_wait_time" => "已等待 {} 秒（超时 120 秒）",
                "touchid_wait_cancel" => "取消，用密码代替",
                // Touch ID 删除中
                "touchid_verify_title" => "Touch ID 验证",
                "touchid_verify_headline" => "请在 Touch ID 传感器上验证指纹",
                "touchid_verify_desc" => "正在删除 {} 项需要管理员权限的文件...",
                "touchid_verify_hint" => "系统会弹出 Touch ID 对话框，请触碰指纹传感器",
                "touchid_verify_log" => "最近日志:",
                "touchid_cancelled" => "已取消 Touch ID 授权",
                "touchid_clamshell_error" => "屏幕已合上，Touch ID 不可用，请输入密码",
                "touchid_timeout_error" => "操作超时：未检测到 Touch ID 启用，请重试或使用密码",
                // 删除中
                "deleting_sudo_phase" => "正在使用管理员权限删除...",
                // 删除完成汇总
                "summary_title" => "清理结果",
                "summary_success" => "成功删除 {} 项",
                "summary_fail" => "删除失败 {} 项",
                "summary_fail_hint" => "部分文件因权限或系统保护无法删除，详见上方日志。",
                "summary_solution_title" => "提示: 失败原因及解决方案",
                "summary_sip_tip" => "SIP/系统保护: /Library/Developer/CoreSimulator 等路径受 macOS SIP 保护，任何权限都无法删除。此类路径已在扫描时标记为不可删除，无需关闭 SIP。",
                "summary_perm_tip" => "权限不足: 部分文件由 root 拥有或位于受保护目录。点击下方「重试删除（管理员授权）」，应用会自动请求授权后完成删除，无需手动执行命令。",
                "summary_solution_1" => "1. 点击下方「重试删除（管理员授权）」：应用自动授权并重试删除失败项",
                "summary_solution_2" => "2. 系统保护路径（SIP）无法删除属正常现象：应用不会移除 macOS 系统文件",
                "summary_solution_3" => "3. 重试后仍无法删除的项目：可在对应 Tab 取消勾选后继续使用，或检查文件是否被占用",
                "summary_open_settings" => "打开系统设置",
                "summary_fail_list" => "失败列表:",
                "summary_copy_paths" => "复制路径",
                "summary_copy_sudo" => "复制 sudo 命令",
                "summary_retry_delete" => "重试删除（管理员授权）",
                "log_protected_root_rejected" => "受保护的系统根目录，拒绝删除：{}",
                "summary_retry_hint" => "应用会自动请求管理员授权重试删除，无需手动执行命令。",
                "fail_reason_perm" => "权限不足",
                "fail_reason_sip" => "受 SIP/系统保护",
                "fail_reason_inuse" => "文件正被其他进程占用",
                "fail_reason_immutable" => "文件被锁定（immutable 标志）",
                "fail_reason_other" => "其他原因",
                "summary_free_space" => "当前可用空间: {}",
                "summary_ok" => "确定",
                "finish_summary" => "清理完成: 成功 {} 项, 失败 {} 项",
                // 后台日志
                "log_intercepted" => "已拦截: {} - {}",
                "log_skipped" => "已跳过: {} - {}",
                "log_deleting" => "正在删除: {}",
                "log_deleted" => "已删除 [{}] {} (成功 {} / 失败 {})",
                "log_deleted_sudo" => "已删除 [{}] {} (管理员权限)",
                "log_deleted_touchid" => "已删除 [{}] {} (Touch ID)",
                "log_delete_failed" => "删除失败: {} - {}",
                "log_snapshot_deleted" => "已删除快照: {}",
                "log_runtime_deleted" => "已删除运行时: {}",
                "log_action_trashed" => "已移至废纸篓",
                "log_action_deleted" => "已删除",
                "log_xcrun_failed" => "xcrun 删除失败，将尝试 sudo: {}",
                "log_skip_running" => "跳过 [{}] Xcode/Simulator 正在运行",
                "log_docker_failed" => "Docker 清理失败: {}",
                "log_path_not_exist" => "路径不存在: {}",
                "already_cleaned" => "已清理",
                "log_symlink_rejected" => "拒绝删除符号链接: {}",
                "log_need_sudo" => "{} 项需要管理员权限",
                "log_sudo_phase" => "正在使用管理员权限删除...",
                "log_password_wrong" => "管理员密码错误，请重新输入",
                "log_trash_failed" => "无法移入废纸篓（请在系统设置中授予「自动化」权限），已保留原文件：{}",
                // M-3 一键卸载
                "uninstall_invalid_path" => "该路径不是可卸载的应用",
                "uninstall_no_bundle" => "无法读取应用的 Bundle ID，请确认应用包完整",
                "uninstall_blocked_critical" => "系统关键应用，禁止一键卸载",
                "uninstall_blocked_official" => "受保护应用，请使用 {0} 官方卸载工具",
                "uninstall_delegated" => "已启动官方卸载器：{0}，请在卸载器中完成剩余步骤",
                "uninstall_nothing" => "该应用没有可删除的文件",
                "uninstall_done" => "已卸载 {0}：{1} 项已移入废纸篓",
                "uninstall_partial" => "已卸载 {0}：{1} 项被拦截或删除失败",
                "log_sudo_rejected" => "sudo 删除前被安全校验拦截：{} — {}",
                "log_sudo_symlink_rejected" => "sudo 删除前被拦截（路径已变为符号链接）：{}",
                "log_sudo_unsafe_path" => "sudo 删除前被拦截（路径含不安全字符）：{}",
                "log_exit_code" => "sudo 退出码 {}",
                "log_no_sim_runtimes" => "没有找到已安装的模拟器运行时",
                "log_mount_in_use" => "模拟器运行时正在被挂载使用，跳过删除",
                "log_cannot_get_mount" => "无法获取挂载点信息，为安全起见跳过删除",
                "log_sim_deleted" => "已通过 xcrun simctl 删除 {} 个模拟器运行时{}",
                "log_sim_failed_suffix" => "，{} 个失败",
                "log_docker_not_running" => "Docker daemon 未运行，请先启动 Docker Desktop",
                "log_xcrun_done" => "xcrun 执行完成: {}",
                "log_touchid_verifying" => "Touch ID 验证中，请在传感器上验证指纹...",
                "log_wait_touchid" => "等待 Touch ID 授权执行 xcrun...",
                "log_touchid_prepare_xcrun" => "准备通过 xcrun 删除: {}",
                "log_touchid_xcrun_deleted" => "已通过 xcrun simctl 删除 {} 个模拟器运行时镜像",
                "log_touchid_runtime_deleted_path" => "已通过 xcrun simctl 删除模拟器运行时镜像: {}",
                "log_sudo_execute" => "准备 sudo 删除 {} 项残留文件...",
                "log_sudo_done2" => "sudo 删除执行完成，正在解析结果...",
                "log_no_sudo_needed" => "无需 sudo 删除，全部通过 xcrun 完成",
                "log_touchid_cancel" => "Touch ID 取消",
                "log_sip_protected" => "SIP保护无法删除: {}",
                "log_still_exists" => "管理员权限删除后仍存在",
                "log_sudo_failed" => "无法启动 sudo: {} - {}",
                "log_cannot_start_sudo" => "无法启动 sudo: {} - {}",
                "log_docker_done" => "Docker 清理完成，释放空间: {}",
                "log_menu_event" => "菜单栏事件: {}",
                "log_quickclean_start" => "一键清理：自动选择 {} 个安全项，开始删除",
                "log_quickclean_none" => "一键清理：没有可删除的安全项",
                "log_keepalive_failed" => "sudo keepalive 启动失败: {}",
                "log_unknown" => "未知",
                "log_cancelled_auth" => "已取消授权: {}",
                "log_cancel_reason" => "用户取消授权",
                "log_sip_reason" => "SIP保护或系统限制",
                "log_acl_protected" => "系统保护（ACL规则禁止删除，任何权限均无法删除）: {}",
                "summary_close" => "关闭",
                // 关联文件标签
                "assoc_app" => "应用本体",
                "assoc_container" => "应用容器",
                "assoc_group" => "共享容器",
                "assoc_cookie" => "Cookie",
                "assoc_webkit" => "WebKit数据",
                "assoc_script" => "应用脚本",
                "assoc_metadata" => "元数据",
                "assoc_cache" => "缓存",
                "assoc_app_data" => "应用数据",
                "assoc_prefs" => "偏好设置",
                "assoc_logs" => "日志",
                "assoc_saved_state" => "窗口状态",
                "assoc_http_storage" => "网络存储",
                "assoc_other" => "其他",
                _ => "",
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translate_recommend() {
        assert_eq!(translate_recommend(&Recommend::Safe, true), "Safe");
        assert_eq!(
            translate_recommend(&Recommend::CacheOnly, true),
            "Cache Only"
        );
        assert_eq!(translate_recommend(&Recommend::Caution, true), "Caution");
        assert_eq!(translate_recommend(&Recommend::Advanced, true), "Advanced");
        assert_eq!(translate_recommend(&Recommend::Safe, false), "推荐");
        assert_eq!(translate_recommend(&Recommend::CacheOnly, false), "仅缓存");
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
        assert_eq!(
            translate_category("Xcode编译-MyProject", true),
            "Xcode Build-MyProject"
        );
        assert_eq!(translate_category("iOS设备-17.0", true), "iOS Device-17.0");
        assert_eq!(
            translate_category("孤儿服务-user", true),
            "Orphaned Service-user"
        );
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

    // ---------- 注册表英文文案（2026-09-19 修的漏翻 bug） ----------

    /// 英文界面下，缓存注册表里的每一条都必须翻成英文。
    ///
    /// 修之前注册表根本不参与翻译，全靠手写表里那份副本；那条路径一旦被
    /// 人改坏（或加新缓存时只写了 desc_zh 忘了同步手写表），英文界面就会
    /// 静默漏出中文。这条用例钉死"注册表是唯一来源且必须生效"。
    #[test]
    fn every_registry_description_translates_to_english() {
        for def in crate::scanner::cache_registry::CACHE_REGISTRY {
            let out = translate_description(def.desc_zh, true);
            assert_eq!(
                out, def.desc_en,
                "注册表条目 {} 的英文文案没生效（拿到的是 {:?}）",
                def.name, out
            );
            // 漏翻的典型症状就是结果里还带着中文字符
            assert!(
                !out.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "注册表条目 {} 的英文文案里混进了中文: {:?}",
                def.name,
                out
            );
        }
    }

    /// 中文模式下必须原样返回，不能被英文污染
    #[test]
    fn registry_descriptions_stay_chinese_when_lang_is_zh() {
        for def in crate::scanner::cache_registry::CACHE_REGISTRY {
            assert_eq!(translate_description(def.desc_zh, false), def.desc_zh);
        }
    }

    /// 单一真相：属于注册表的描述不许再抄一份进 i18n 手写表。
    ///
    /// 两份副本内容一致时看不出问题，但改了一处、另一处没跟着改就是乱码级
    /// 的体验问题，而且没有任何编译期保护 —— 只能靠这条用例拦。
    #[test]
    fn registry_descriptions_are_not_duplicated_here() {
        for def in crate::scanner::cache_registry::CACHE_REGISTRY {
            assert!(
                exact_description_en(def.desc_zh).is_none(),
                "{} 的英文文案在注册表和 i18n 手写表里各有一份，删掉手写表那条",
                def.name
            );
        }
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
        assert_eq!(
            translate_category("App残留缓存", true),
            "App Residual Cache"
        );
        assert_eq!(
            translate_category("App残留配置", true),
            "App Residual Prefs"
        );
    }

    #[test]
    fn test_translate_app_uninstall_descriptions() {
        assert_eq!(
            translate_description("Downloads 中的 Test.app，同名应用已安装，可安全清理", true),
            "Test.app in Downloads, same-name app installed, safe to clean"
        );
        assert_eq!(
            translate_description(
                "Downloads 中的 Test.app，未检测到同名已安装应用，请确认后再删除",
                true
            ),
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
