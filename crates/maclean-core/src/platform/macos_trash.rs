//! macOS 进程内「移入废纸篓」。
//!
//! 旧实现用 `osascript -e 'tell application "Finder" to delete ...'` 让
//! **Finder 跨进程代删**，属于 AppleEvent 自动化：受 TCC「自动化 → Finder」
//! 约束，且由后台子进程拉起的授权面板常常 Touch ID 不可用、用户输完密码后
//! 授权结果也无法正确交回（典型表现：弹窗后 `can't operate` / -1743，删除
//! 结果与返回值还会时序错位）。
//!
//! 这里改为苹果官方的**进程内**回收 API，行为等同于用户在访达里手动拖入
//! 废纸篓，可从废纸篓还原：
//!
//! 1. 先静默尝试 [`NSFileManager::trashItemAtURL`]：不弹任何窗。用户拥有的
//!    文件 / `~/Applications` 下 PWA、以及当前用户可写的 `/Applications`
//!    应用直接成功。
//! 2. 失败（典型为 root/pkg 安装、当前用户无写权限的 `/Applications/X.app`）
//!    再走 [`NSWorkspace::recycleURLs`]：由 maclean 自身弹出**正规**系统
//!    Touch ID / 密码授权并完成，不需要「自动化 Finder」权限。调用派到主线程，
//!    删除工作线程在后台等待 completion（用户输密码可能较慢，给 5 分钟预算）。
//!
//! 安全承诺不变：两条进程内路径都失败时返回 `false`、**绝不**降级为永久删除。

use std::ptr::NonNull;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::{autoreleasepool, Retained};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSArray, NSDictionary, NSError, NSFileManager, NSString, NSURL};

use crate::logger;

/// 第一档：[`NSFileManager`] 同步回收。线程安全、不弹授权窗。
fn file_manager_trash(path: &str) -> bool {
    autoreleasepool(|_pool| {
        let fm = NSFileManager::defaultManager();
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        let mut resulting: Option<Retained<NSURL>> = None;
        match fm.trashItemAtURL_resultingItemURL_error(&url, Some(&mut resulting)) {
            Ok(()) => true,
            Err(e) => {
                logger::warn(&format!(
                    "[trash] NSFileManager 静默回收失败（错误码 {}），改走系统授权回收: {}",
                    e.code(),
                    path
                ));
                false
            }
        }
    })
}

/// 第二档：[`NSWorkspace::recycleURLs`] 异步授权回收。
///
/// 必须在**后台线程**调用：本函数把发起动作派到主线程（NSWorkspace 要求），
/// 然后在当前线程阻塞等待 completion handler，绝不能在主线程等待（会死锁）。
fn workspace_recycle(path: &str) -> bool {
    if unsafe { libc::pthread_main_np() } != 0 {
        // 删除链路（start_delete）固定在工作线程；真在主线程说明调用方式有误，
        // 直接失败而不是派发到主线程后死等（主队列任务永远不会被执行）。
        logger::warn("[trash] 系统授权回收不能在主线程发起/等待，已保留文件");
        return false;
    }

    // None=尚未回调；Some(true/false)=回收成功/用户取消或拒绝。
    let signal = Arc::new((Mutex::new(None::<bool>), Condvar::new()));
    let sig2 = signal.clone();
    let owned = path.to_string();

    DispatchQueue::main().exec_async(move || {
        autoreleasepool(|_pool| {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&owned));
            let urls: Retained<NSArray<NSURL>> = NSArray::arrayWithObject(&url);
            let ws = NSWorkspace::sharedWorkspace();

            // 系统会把 handler block copy 到堆并在完成后释放，因此本 block
            // 随闭包返回而 drop 不影响回调。
            let block = RcBlock::new(
                move |_result: NonNull<NSDictionary<NSURL, NSURL>>, error: *mut NSError| {
                    let ok = error.is_null();
                    if !ok {
                        // NSError 在本回调作用域内有效，仅读取整数错误码后即用即弃。
                        let code = unsafe { error.as_ref() }.map(|e| e.code()).unwrap_or(0);
                        logger::warn(&format!(
                            "[trash] 系统授权回收未完成（错误码 {}，多为用户取消/拒绝授权）: {}",
                            code, owned
                        ));
                    }
                    let (m, c) = (&sig2.0, &sig2.1);
                    if let Ok(mut g) = m.lock() {
                        *g = Some(ok);
                    }
                    c.notify_all();
                },
            );

            ws.recycleURLs_completionHandler(&urls, Some(&block));
        });
    });

    let (lock, cvar) = &*signal;
    let mut guard = match lock.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let deadline = Instant::now() + Duration::from_secs(300); // 留足手动输密码时间
    loop {
        if let Some(ok) = *guard {
            return ok;
        }
        if Instant::now() >= deadline {
            logger::warn(&format!(
                "[trash] 等待系统授权回收超时（300s），保留文件: {}",
                path
            ));
            return false;
        }
        let (next, _timeout) = cvar
            .wait_timeout(guard, Duration::from_millis(250))
            .unwrap_or_else(|e| e.into_inner());
        guard = next;
    }
}

/// macOS 移入废纸篓入口（由 [`super::move_to_trash`] 调用）。
pub(crate) fn move_to_trash_impl(path: &str) -> bool {
    let p = std::path::Path::new(path);
    if !p.exists() && p.symlink_metadata().is_err() {
        return false;
    }

    // 只读卷（OrbStack 挂载、APFS 快照等）上系统无法回收，调用只会弹出
    // 无法静音的失败对话框；预先探测并短路。
    if crate::platform::path_on_readonly_volume(path) {
        logger::warn(&format!(
            "目标位于只读卷，跳过废纸篓（不触发系统对话框）: {}",
            path
        ));
        return false;
    }

    if file_manager_trash(path) {
        return true;
    }
    workspace_recycle(path)
}
