fn main() {
    // 链接 Security.framework（提供 AuthorizationCreate / AuthorizationExecuteWithPrivileges）
    println!("cargo:rustc-link-lib=framework=Security");
}
