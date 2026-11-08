//! 废纸篓（Trash / Recycle Bin）能力契约。
//!
//! 现有实现位于 maclean-core `platform/macos_trash.rs`（进程内
//! NSFileManager/NSWorkspace）；本模块定义统一契约，供 core / CLI / 未来
//! Pro 共用，也为 Windows Recycle Bin 预留适配位。

/// 废纸篓操作结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrashResult {
    /// 已移入废纸篓（可还原）
    Moved,
    /// 目标不存在（视为成功，幂等）
    NotFound,
    /// 失败，附原因
    Failed(String),
}

impl TrashResult {
    pub fn is_ok(&self) -> bool {
        matches!(self, TrashResult::Moved | TrashResult::NotFound)
    }
}

/// 废纸篓能力
pub trait Trash {
    /// 把一个文件/目录移入废纸篓（同名已存在时由系统改名，不覆盖）
    fn move_to_trash(&self, path: &std::path::Path) -> TrashResult;

    /// 是否支持还原（当前 Free 核心以「移入废纸篓」为可还原语义）
    fn supports_restore(&self) -> bool {
        true
    }
}

/// 无操作适配器（用于测试 / 非交互环境显式禁用废纸篓）
pub struct NoopTrash;

impl Trash for NoopTrash {
    fn move_to_trash(&self, _path: &std::path::Path) -> TrashResult {
        TrashResult::Failed("废纸篓已禁用".into())
    }

    fn supports_restore(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_result_ok_semantics() {
        assert!(TrashResult::Moved.is_ok());
        assert!(TrashResult::NotFound.is_ok());
        assert!(!TrashResult::Failed("x".into()).is_ok());
    }

    #[test]
    fn noop_trash_never_moves() {
        let t = NoopTrash;
        assert!(!t.move_to_trash(std::path::Path::new("/tmp/x")).is_ok());
        assert!(!t.supports_restore());
    }
}
