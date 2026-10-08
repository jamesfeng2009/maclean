# Third-Party Notices

maclean 依赖以下第三方组件。许可信息基于各组件发布元数据（crates.io / npm）；
完整许可文本见各组件仓库或本地 Cargo registry 缓存。

## Rust（workspace 直接依赖）

| 组件 | 版本 | License |
|---|---|---|
| serde / serde_derive | 1 | Apache-2.0 OR MIT |
| serde_json | 1 | Apache-2.0 OR MIT |
| clap / clap_builder / clap_derive | 4 | Apache-2.0 OR MIT |
| ctrlc | 3 | MIT OR Apache-2.0 |
| walkdir | 2 | Unlicense OR MIT |
| rayon | 1 | Apache-2.0 OR MIT |
| dirs | 5 | MIT OR Apache-2.0 |
| libc | 0.2 | MIT OR Apache-2.0 |
| eframe / egui / epaint / egui-winit | 0.29 | MIT OR Apache-2.0 |
| tray-icon | 0.14 | Apache-2.0 OR MIT |
| image | 0.25 | MIT OR Apache-2.0 |
| ed25519-dalek | 2 | BSD-3-Clause |
| base64 | 0.22 | MIT OR Apache-2.0 |
| sha2 | 0.10 | Apache-2.0 OR MIT |
| uuid | 1 | Apache-2.0 OR MIT |
| embed-resource | 2 | MIT |
| tauri / tauri-build / tauri-plugin-* | 2 | Apache-2.0 OR MIT |
| anyhow | 1 | MIT OR Apache-2.0 |
| log | 0.4 | MIT OR Apache-2.0 |

## JavaScript / TypeScript（tauri/ 前端）

| 组件 | 版本 | License |
|---|---|---|
| @tauri-apps/api | 2.x | Apache-2.0 OR MIT |
| react / react-dom | 18.x | MIT |
| vite | 5.x | MIT |
| typescript | 5.x | Apache-2.0 |
| @vitejs/plugin-react | 4.x | MIT |

## 生成工具链（开发期）

- `cargo` / `rustc`（Rust 工具链）：MIT OR Apache-2.0
- Homebrew（打包脚本调用）：BSD-2-Clause
- create-dmg（tauri bundler 内置）：MIT

## 备注

- 未发现 copyleft（GPL/AGPL/LGPL）依赖进入生产链路（cargo-deny 门禁见
  `deny.toml`，Phase 9 P9-10）。
- 依赖清单随 `cargo tree` 生成；新增直接依赖时请同步更新本文件并核对 license。
- 若某组件版权声明缺失或有疑问，请按 `SECURITY.md` 渠道联系维护者。
