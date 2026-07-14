fn main() {
    // 仅 macOS 目标链接 Security.framework（提供 AuthorizationCreate / AuthorizationExecuteWithPrivileges）
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "macos" {
        println!("cargo:rustc-link-lib=framework=Security");
    }
}
