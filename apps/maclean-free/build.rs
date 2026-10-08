//! 构建脚本
//!
//! 两件事：
//! 1. macOS 目标链接 Security.framework（AuthorizationExecuteWithPrivileges / Touch ID 提权）
//! 2. Windows 目标嵌入资源：应用图标 + manifest（DPI 感知 / 长路径 / UAC 级别 / 版本信息）
//!
//! 关于第 2 点的**验证边界**：
//! - 本机（macOS）交叉编译时若找不到资源编译器（windres / rc.exe），只打印
//!   `cargo:warning=` 并跳过 —— 不能因为本机缺工具链就把整个交叉编译打断。
//! - 在 Windows 原生构建（host == windows）时找不到/编译失败则**直接 panic**。
//!   这是刻意的：CI 的 windows-check 任务就在原生 Windows 上跑，
//!   资源嵌入如果悄悄退化，CI 不该假装通过。

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // 仅 macOS 目标链接 Security.framework（提供 AuthorizationCreate /
    // AuthorizationExecuteWithPrivileges / LAContext 等）
    if target_os == "macos" {
        println!("cargo:rustc-link-lib=framework=Security");
    }

    if target_os == "windows" {
        embed_windows_resources();
    }
}

fn embed_windows_resources() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let icon = manifest_dir.join("assets/icon/maclean.ico");
    let app_manifest = manifest_dir.join("assets/windows.manifest");

    if !icon.exists() {
        panic!("缺少图标资源: {}（应先从 PNG 生成 .ico）", icon.display());
    }
    if !app_manifest.exists() {
        panic!("缺少 Windows manifest: {}", app_manifest.display());
    }

    let (maj, min, pat) = version_triple();
    let file_version = format!("{},{},{},0", maj, min, pat);
    let str_version = format!("{}.{}.{}.0", maj, min, pat);

    // .rc 源码在 OUT_DIR 里生成，而不是仓库里手写死一份 ——
    // 版本号只有一个来源：Cargo.toml。手写第二份必然漂移
    // （ci.yml 里那个 version-consistency 任务防的就是这种漂移）。
    //
    // 路径一律用正斜杠：rc.exe 与 windres 都接受，且不需要转义反斜杠。
    // 刻意不 #include <winver.h>：VERSIONINFO 里全用数值字面量，不需要头文件。
    // 少了这个 include，从 macOS 交叉编译时就不必依赖 Windows SDK 头文件 ——
    // 那是 llvm-rc 预处理阶段最容易炸的一环。
    let rc_src = format!(
        r#"IDI_ICON1 ICON "{icon}"

/* RT_MANIFEST = 24；exe 的清单资源 ID 必须是 1（CREATEPROCESS_MANIFEST_RESOURCE_ID） */
1 24 "{manifest}"

VS_VERSION_INFO VERSIONINFO
FILEVERSION {fv}
PRODUCTVERSION {fv}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName",      "sunge.men"
            VALUE "FileDescription",  "maclean - disk cleaner"
            VALUE "FileVersion",      "{sv}"
            VALUE "InternalName",     "maclean"
            VALUE "OriginalFilename", "maclean.exe"
            VALUE "ProductName",      "maclean"
            VALUE "ProductVersion",   "{sv}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#,
        icon = slash(&icon),
        manifest = slash(&app_manifest),
        fv = file_version,
        sv = str_version,
    );

    let rc_path = out_dir.join("maclean.rc");
    fs::write(&rc_path, rc_src).expect("写入 maclean.rc 失败");

    // 资源文件变了要重新跑构建脚本
    println!("cargo:rerun-if-changed=assets/icon/maclean.ico");
    println!("cargo:rerun-if-changed=assets/windows.manifest");

    // embed_resource::compile 的契约（2.5.2）：
    // - Windows 宿主：找不到 rc.exe 或编译失败会直接 panic —— 这正是 CI 需要的硬门禁，
    //   不需要我们再包一层。
    // - 非 Windows 宿主：只认 llvm-rc（或 RC_<target> / RC 环境变量）。
    //   找不到时**静默返回，什么都不做**，既不报错也不 panic。
    //
    // 第二条是本机的实际处境（macOS 上没有 llvm-rc），也是最危险的一种：
    // 构建"成功"但资源根本没进去，且没有任何痕迹。
    // 所以编译后自己验一次产物是否存在 —— 用 crate 自己的命名契约
    // （产物固定是 {OUT_DIR}/{file_stem}.lib），不去重复实现它的探测逻辑。
    let expected = out_dir.join("maclean.lib");
    embed_resource::compile(&rc_path, embed_resource::NONE);

    if !expected.exists() {
        println!(
            "cargo:warning=未嵌入 Windows 资源（图标 / manifest / 版本信息）：本机缺少资源编译器。\
             交叉编译产物可用于验证编译通过，但**不可用于发布**。\
             Windows 原生构建会在这一步 panic，CI 的 windows-check 会拦住。"
        );
    }
}

/// 把路径里的 `\` 换成 `/`，供 .rc 字符串字面量使用
fn slash(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// 从 CARGO_PKG_VERSION 解析出 (major, minor, patch)
///
/// 版本号可能带预发布后缀（0.2.0-beta.1），这里只取开头的数字段。
/// 解析不出来就退回 0 —— VERSIONINFO 只是个展示字段，
/// 绝不能因为版本号格式怪异就让构建失败。
fn version_triple() -> (u64, u64, u64) {
    let v = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let mut parts = v
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}
