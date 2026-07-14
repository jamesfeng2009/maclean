//! macOS AuthorizationExecuteWithPrivileges (AEWP) FFI 封装
//!
//! 通过 dlsym 运行时查找 AEWP 符号，即使未来 macOS 移除该函数也能优雅失败。
//! 实现思路复刻 STPrivilegedTask (https://github.com/sveinbjornt/STPrivilegedTask)。
//!
//! AEWP 会弹出系统原生密码对话框，以 root 权限执行指定工具。
//! 不需要 Apple Developer ID 证书，不需要打开终端。
//! 兼容 macOS 12+ (Monterey 到 Tahoe)，Intel 和 Apple Silicon 均可。

#![cfg(target_os = "macos")]

use std::ffi::{CStr, CString};
use std::io::Read;
use std::os::raw::{c_char, c_int, c_void};
use std::os::unix::io::FromRawFd;

// ============ FFI 类型声明 ============

pub type OSStatus = i32;
pub type AuthorizationRef = *mut c_void;
pub type AuthorizationFlags = u32;

#[repr(C)]
pub struct AuthorizationItem {
    pub name: *const c_char,
    pub value_length: usize,
    pub value: *const c_void,
    pub flags: u32,
}

#[repr(C)]
pub struct AuthorizationItemSet {
    pub count: usize,
    pub items: *mut AuthorizationItem,
}

// kAuthorizationFlag* 常量
const K_AUTH_FLAG_DEFAULTS: AuthorizationFlags = 0;
const K_AUTH_FLAG_INTERACTION_ALLOWED: AuthorizationFlags = 1 << 0;
const K_AUTH_FLAG_EXTEND_RIGHTS: AuthorizationFlags = 1 << 1;
const K_AUTH_FLAG_PRE_AUTHORIZE: AuthorizationFlags = 1 << 4;

// 错误码
pub const ERR_AUTH_CANCELED: OSStatus = -128;

// libc FFI
extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn fileno(stream: *mut c_void) -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    fn fclose(stream: *mut c_void) -> c_int;
    fn waitpid(pid: i32, status: *mut c_int, options: c_int) -> i32;
    fn dup(fd: c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
}

// Security.framework FFI
extern "C" {
    fn AuthorizationCreate(
        rights: *const AuthorizationItemSet,
        environment: *const AuthorizationItemSet,
        flags: AuthorizationFlags,
        authorization: *mut AuthorizationRef,
    ) -> OSStatus;

    fn AuthorizationFree(authorization: AuthorizationRef, flags: AuthorizationFlags) -> OSStatus;

    fn AuthorizationCopyRights(
        authorization: AuthorizationRef,
        rights: *const AuthorizationItemSet,
        environment: *const AuthorizationItemSet,
        flags: AuthorizationFlags,
        authorized_rights: *mut *mut AuthorizationItemSet,
    ) -> OSStatus;
}

// RTLD_DEFAULT: 在所有已加载镜像中查找符号
const RTLD_DEFAULT: *mut c_void = -1isize as *mut c_void;

// fcntl 命令
const F_GETOWN: c_int = 5;

// waitpid 选项
const WNOHANG: c_int = 1;

/// AEWP 函数签名
type AuthorizationExecuteWithPrivilegesFn = unsafe extern "C" fn(
    authorization: AuthorizationRef,
    path_to_tool: *const c_char,
    options: AuthorizationFlags,
    arguments: *mut *mut c_char,
    communications_pipe: *mut *mut c_void,
) -> OSStatus;

/// 查找 AEWP 函数指针（运行时 dlsym）
fn lookup_aewp_fn() -> Option<AuthorizationExecuteWithPrivilegesFn> {
    unsafe {
        let name = CStr::from_bytes_with_nul(b"AuthorizationExecuteWithPrivileges\0").ok()?;
        let ptr = dlsym(RTLD_DEFAULT, name.as_ptr());
        if ptr.is_null() {
            return None;
        }
        Some(std::mem::transmute::<
            *mut c_void,
            AuthorizationExecuteWithPrivilegesFn,
        >(ptr))
    }
}

/// AEWP 错误类型
#[derive(Debug)]
pub enum AewpError {
    /// AEWP 函数在当前 macOS 中不存在（被 Apple 移除）
    FunctionUnavailable,
    /// 用户在密码对话框点击取消
    UserCanceled,
    /// 授权失败（密码错误等）
    AuthorizationFailed(OSStatus),
    /// 执行失败
    ExecutionFailed(OSStatus),
    /// 路径包含 NUL
    InvalidPath,
    /// IO 错误
    Io(std::io::Error),
}

impl std::fmt::Display for AewpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AewpError::FunctionUnavailable => write!(f, "系统不支持此授权方式"),
            AewpError::UserCanceled => write!(f, "用户取消了授权"),
            AewpError::AuthorizationFailed(s) => write!(f, "授权失败，错误码: {}", s),
            AewpError::ExecutionFailed(s) => write!(f, "执行失败，错误码: {}", s),
            AewpError::InvalidPath => write!(f, "路径包含 NUL 字节"),
            AewpError::Io(e) => write!(f, "IO 错误: {}", e),
        }
    }
}

impl std::error::Error for AewpError {}

impl From<std::io::Error> for AewpError {
    fn from(e: std::io::Error) -> Self {
        AewpError::Io(e)
    }
}

/// 执行结果
#[allow(dead_code)]
pub struct PrivilegedOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
}

/// 检测 AEWP 在当前系统是否可用（不弹窗）
pub fn aewp_available() -> bool {
    lookup_aewp_fn().is_some()
}

/// 以 root 权限执行外部工具并捕获 stdout
///
/// `tool_path` 必须是绝对路径，指向可执行文件（推荐 /bin/sh）
/// `arguments` 是参数列表（不含 argv[0]）
///
/// 会弹出系统密码对话框（阻塞当前线程），建议在后台线程调用。
pub fn execute_with_privileges(
    tool_path: &str,
    arguments: &[&str],
) -> Result<PrivilegedOutput, AewpError> {
    // 1. 查找 AEWP 函数
    let aewp_fn = lookup_aewp_fn().ok_or(AewpError::FunctionUnavailable)?;

    // 2. 构造 C 字符串
    let c_path = CString::new(tool_path).map_err(|_| AewpError::InvalidPath)?;
    let c_args: Vec<CString> = arguments
        .iter()
        .map(|s| CString::new(*s).unwrap())
        .collect();
    // AEWP 要求 argv 风格：以 NULL 结尾的指针数组
    let mut argv: Vec<*mut c_char> = c_args.iter().map(|s| s.as_ptr() as *mut c_char).collect();
    argv.push(std::ptr::null_mut());

    // 3. 创建 AuthorizationRef
    let mut auth_ref: AuthorizationRef = std::ptr::null_mut();
    let status = unsafe {
        AuthorizationCreate(
            std::ptr::null(),
            std::ptr::null(),
            K_AUTH_FLAG_DEFAULTS,
            &mut auth_ref,
        )
    };
    if status != 0 {
        return Err(AewpError::AuthorizationFailed(status));
    }

    // 4. 预授权 kAuthorizationRightExecute（此处弹出系统密码对话框）
    let right_name = b"system.privilege.admin\0";
    let right_name_ptr: *const c_char = right_name.as_ptr() as *const c_char;
    let mut item = AuthorizationItem {
        name: right_name_ptr,
        value_length: 0,
        value: std::ptr::null(),
        flags: 0,
    };
    let rights = AuthorizationItemSet {
        count: 1,
        items: &mut item,
    };

    let flags = K_AUTH_FLAG_DEFAULTS
        | K_AUTH_FLAG_INTERACTION_ALLOWED
        | K_AUTH_FLAG_EXTEND_RIGHTS
        | K_AUTH_FLAG_PRE_AUTHORIZE;

    let status = unsafe {
        AuthorizationCopyRights(
            auth_ref,
            &rights,
            std::ptr::null(),
            flags,
            std::ptr::null_mut(),
        )
    };

    if status == ERR_AUTH_CANCELED {
        unsafe { AuthorizationFree(auth_ref, K_AUTH_FLAG_DEFAULTS) };
        return Err(AewpError::UserCanceled);
    }
    if status != 0 {
        unsafe { AuthorizationFree(auth_ref, K_AUTH_FLAG_DEFAULTS) };
        return Err(AewpError::AuthorizationFailed(status));
    }

    // 5. 执行特权工具
    let mut comm_pipe: *mut c_void = std::ptr::null_mut();
    let status = unsafe {
        aewp_fn(
            auth_ref,
            c_path.as_ptr(),
            K_AUTH_FLAG_DEFAULTS,
            argv.as_mut_ptr(),
            &mut comm_pipe,
        )
    };

    unsafe { AuthorizationFree(auth_ref, K_AUTH_FLAG_DEFAULTS) };

    if status != 0 {
        return Err(AewpError::ExecutionFailed(status));
    }

    // 6. 读取 stdout（comm_pipe 是 FILE*）
    let pid = if !comm_pipe.is_null() {
        unsafe { fcntl(fileno(comm_pipe), F_GETOWN, 0) }
    } else {
        -1
    };

    // 读管道直到 EOF
    let mut stdout = Vec::new();
    if !comm_pipe.is_null() {
        let fd = unsafe { fileno(comm_pipe) };
        let dup_fd = unsafe { dup(fd) };
        if dup_fd >= 0 {
            let mut file = unsafe { std::fs::File::from_raw_fd(dup_fd) };
            let _ = file.read_to_end(&mut stdout);
            unsafe { close(dup_fd) };
        }
        unsafe { fclose(comm_pipe) };
    }

    // 7. 等待子进程退出
    let mut wait_status: c_int = 0;
    if pid > 0 {
        loop {
            let waited = unsafe { waitpid(pid, &mut wait_status, WNOHANG) };
            if waited == pid {
                break;
            }
            if waited == -1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    let exit_code = if wait_status == 0 {
        0
    } else {
        // 提取退出码：WEXITSTATUS
        (wait_status >> 8) & 0xff
    };

    Ok(PrivilegedOutput { exit_code, stdout })
}
