# maclean 项目长期约定

## 门禁与工具链

- 提交前必跑：`cargo test` + `cargo clippy --all-targets -- -D warnings`
  + `cargo clippy --target x86_64-pc-windows-msvc --all-targets` + `cargo fmt`。
- **CI 用 `dtolnay/rust-toolchain@stable`，版本会漂移**。本机 rustc 可能落后
  CI 好几个版本，本机 clippy 干净 ≠ CI 干净。
  遇到"CI 红但本机绿"，先 `rustup toolchain install <CI版本> --profile minimal
  --component clippy` 再 `cargo +<版本> clippy` 复现。（2026-09-20：本机 1.90 /
  CI 1.98，差 8 个版本的 lint。）

## 测试写法约定

- **禁止在测试里重写被测逻辑**。发现过 `backup::tests::listing_sorts_newest_first`
  自己写一遍降序再断言 —— 只验证了标准库，生产代码改坏也通过。
  解法：把被测逻辑抽成纯函数，测试调用它，并反向验证（改坏 → 测试必须变红）。
- **`include_str!` 源码级断言会自命中**：测试里写的匹配字面量会被自己数进去。
  按测试模块切分源码再断言：`src.split("\nmod tests").next().unwrap_or(src)`。
- 跨平台纯逻辑**不加 cfg**，平台分支用入参传入，这样两个分支在开发机都能真跑。
- clippy 的 dead_code 是**接线检查器**：写完模块报 dead_code = 写了没接线，
  修法是接上去，不是加 `#[allow]`。

## 本项目的两个易错点

- Edit 工具曾多次"报成功但文件未改"。改完必须立刻 grep/sed 校验。
- Windows 侧改动必须过 `--target x86_64-pc-windows-msvc` 验证，
  `#[cfg(windows)]` 里的代码在本机 0 编译 0 测试。
