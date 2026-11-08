//! 磁盘信息契约。
//!
//! 现有实现位于 maclean-core `platform/mod.rs`（`disk_info`：df /
//! PowerShell）。本模块定义统一契约与解析纯逻辑。

use serde::{Deserialize, Serialize};

/// 磁盘空间信息
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskInfo {
    /// 总字节
    pub total_bytes: u64,
    /// 可用字节
    pub available_bytes: u64,
}

impl DiskInfo {
    /// 已用字节
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.available_bytes)
    }

    /// 使用率（0.0 - 1.0；total 为 0 时返回 0）
    pub fn usage_ratio(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            self.used_bytes() as f64 / self.total_bytes as f64
        }
    }
}

/// 磁盘信息提供者契约
pub trait DiskInfoProvider {
    fn disk_info(&self) -> Result<DiskInfo, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_info_arithmetic() {
        let d = DiskInfo {
            total_bytes: 100,
            available_bytes: 25,
        };
        assert_eq!(d.used_bytes(), 75);
        assert!((d.usage_ratio() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn zero_total_avoids_div_zero() {
        let d = DiskInfo {
            total_bytes: 0,
            available_bytes: 0,
        };
        assert_eq!(d.usage_ratio(), 0.0);
    }
}
