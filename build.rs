fn main() {
    // 仅 macOS 链接 Security.framework（提供 AuthorizationCreate / AuthorizationExecuteWithPrivileges）
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-lib=framework=Security");
    }
}
