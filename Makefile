.PHONY: build release package clean install test check-windows check-windows-full print-version fmt lint run run-release package-only uninstall help

# maclean Makefile（2026-10-13 商业化边界调整：公开仓库 = 纯 CLI）

# ---------------------------------------------------------------------------
# CORE-U06 · 统一工具链（所有 cargo target 共用，别只改 check-windows）
#
# 本机陷阱：/opt/homebrew/bin 若排在 ~/.cargo/bin 之前，`cargo` 会是 Homebrew
# 的 1.99.1，rust-toolchain.toml 的 1.90.0 pin 被架空（E0463 假象）。
# 这里一次性抽出 rustup 1.90.0 的 bin（TCDIR），前置到**所有** target 的 PATH；
# 与 CI（dtolnay/rust-toolchain@1.90.0）保持同一版本。CI 工具链版本不改。
# ---------------------------------------------------------------------------
TOOLCHAIN ?= 1.90.0
# TCDIR 一次性算出、所有 cargo target 复用（别只在 check-windows 里算）。
TCDIR := $(shell rustup which rustc --toolchain $(TOOLCHAIN) 2>/dev/null | sed 's|/bin/rustc$$|/bin|')
ifeq ($(strip $(TCDIR)),)
$(warning 未找到 rustup $(TOOLCHAIN) 的 rustc，前缀留空将回退到系统 PATH；请 `rustup toolchain install $(TOOLCHAIN)`)
endif
# 所有直接 cargo 调用统一走这个前缀：把 rustup $(TOOLCHAIN) 的 bin 前置到
# 本次命令的 PATH。`$$PATH` 由 recipe 的 shell 展开为原 PATH（不是 Make 变量）。
CARGO := PATH="$(TCDIR):$$PATH" cargo

# H-07 / I-01：版本号唯一来源是根 Cargo.toml 的 [workspace.package] version。
# 旧写法 `grep '^version' apps/maclean-cli/Cargo.toml | cut -d'"' -f2` 在成员改为
# `version.workspace = true` 后取到的是该字面量（无引号可切），并非版本号。
# 解析口径与 ci.yml / build-cli-package.sh / release-macos.yml 保持一致。
VERSION := $(shell sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml | grep -m1 '^version = "' | cut -d'"' -f2)

## 编译 debug 版本（纯 CLI）
build:
	$(CARGO) build

## 编译 release 版本（纯 CLI）
release:
	$(CARGO) build --release

## 运行 (debug)
run: build
	./target/debug/maclean

## 运行 (release)
run-release: release
	./target/release/maclean

## 打包 CLI 发行产物（tar.gz + SHA256，供 GitHub Release 使用）
package: release
	@bash scripts/build-cli-package.sh

## 快速打包 (跳过编译)
package-only:
	@bash scripts/build-cli-package.sh

## 安装到系统 (需要 sudo)
install: release
	@echo "安装 maclean 到 /usr/local/bin..."
	sudo cp target/release/maclean /usr/local/bin/maclean
	sudo chmod +x /usr/local/bin/maclean
	@echo "安装完成，终端运行: maclean"

## 卸载
uninstall:
	@echo "卸载 maclean..."
	sudo rm -f /usr/local/bin/maclean
	@echo "卸载完成"

## 清理
clean:
	$(CARGO) clean
	rm -rf target/package

## 检查
check:
	$(CARGO) check

## 运行测试（workspace = 3 个库 crate + CLI）
test:
	$(CARGO) test --workspace

## 代码格式检查
fmt:
	$(CARGO) fmt --all -- --check

## Clippy 静态检查（H-04：--workspace，否则 3 个库 crate 不 lint）
lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

## H-07：打印从根 Cargo.toml 解析出的版本（CI version-consistency 用它对齐）
print-version:
	@echo $(VERSION)

## H-05：本地 Windows 交叉编译门禁（纯 Rust crate 的生产代码，本机 macOS 可通过）。
## CORE-U06：不再各 target 自己前置 —— 顶部统一定义 `$(CARGO)`（把 rustup
## $(TOOLCHAIN) 的 bin 经 TCDIR 前置到本次命令 PATH），build/release/test/fmt/lint/check
## 全部生效；CI 工具链版本不改。
## 只跑 --lib（生产 #[cfg(windows)] 分支）：含测试的 --all-targets / cargo test 由
## windows-ci.yml 的 Windows runner 承担（H-03），本机无 MSVC linker 也链接不了测试。
WINDOWS_TARGET ?= x86_64-pc-windows-msvc
check-windows:
	@echo "→ rustup target add $(WINDOWS_TARGET) --toolchain $(TOOLCHAIN)"
	rustup target add $(WINDOWS_TARGET) --toolchain $(TOOLCHAIN)
	@echo "→ 用 $(TOOLCHAIN) 校验 Windows cfg 分支（TCDIR=$(TCDIR) 已前置到 PATH）"
	$(CARGO) check --target $(WINDOWS_TARGET) \
	  -p maclean-types -p maclean-core -p maclean-cli-contract --lib

## H-05：完整 --workspace --all-targets（验收原命令）。
## macOS 上会因 apps/maclean-cli 的 ureq→rustls→ring C 依赖缺 MSVC SDK 失败；
## 需在带 MSVC 的环境或 windows-ci.yml 的 Windows runner 执行，结论见 CONTRIBUTING.md。
check-windows-full:
	@echo "→ rustup target add $(WINDOWS_TARGET) --toolchain $(TOOLCHAIN)"
	rustup target add $(WINDOWS_TARGET) --toolchain $(TOOLCHAIN)
	@echo "→ 用 $(TOOLCHAIN) 校验（完整 workspace，需 MSVC）"
	$(CARGO) check --target $(WINDOWS_TARGET) --workspace --all-targets

## 帮助
help:
	@echo "maclean Makefile"
	@echo ""
	@echo "可用命令:"
	@echo "  make build       - 编译 debug 版本（纯 CLI）"
	@echo "  make release     - 编译 release 版本（纯 CLI）"
	@echo "  make run         - 运行 debug 版本"
	@echo "  make run-release - 运行 release 版本"
	@echo "  make package     - 打包 CLI 发行产物（tar.gz + SHA256）"
	@echo "  make install     - 安装到 /usr/local/bin"
	@echo "  make uninstall   - 卸载"
	@echo "  make clean       - 清理构建产物"
	@echo "  make check       - 检查编译"
	@echo "  make test        - 运行全部测试（workspace）"
	@echo "  make fmt         - 检查代码格式"
	@echo "  make lint        - Clippy 静态检查（--workspace -D warnings）"
	@echo "  make print-version - 打印根 Cargo.toml 版本号"
	@echo "  make check-windows - Windows 交叉编译门禁（纯 Rust crate，H-05）"
	@echo "  make check-windows-full - 完整 workspace 交叉编译（需 MSVC）"
	@echo "  工具链: rustup $(TOOLCHAIN)（所有 cargo target 已前置 $(TCDIR)）"
	@echo "  当前版本: $(VERSION)"
	@echo ""
	@echo "GUI App（14 天试用 / 付费）在私有 maclean-pro 仓库构建，不在本仓库。"
