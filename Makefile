.PHONY: build release package clean install test

# maclean Makefile（2026-10-13 商业化边界调整：公开仓库 = 纯 CLI）

# 版本号单一来源：Cargo.toml（不要在 Makefile 里再写一份，否则三处不一致）
VERSION := $(shell grep -m1 '^version' apps/maclean-cli/Cargo.toml | cut -d'"' -f2)

## 编译 debug 版本（纯 CLI）
build:
	cargo build

## 编译 release 版本（纯 CLI）
release:
	cargo build --release

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
	cargo clean
	rm -rf target/package

## 检查
check:
	cargo check

## 运行测试（workspace = 5 公开 crate + CLI）
test:
	cargo test --workspace

## 代码格式检查
fmt:
	cargo fmt --all -- --check

## Clippy 静态检查
lint:
	cargo clippy --all-targets -- -D warnings

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
	@echo "  make lint        - Clippy 静态检查（-D warnings）"
	@echo "  当前版本: $(VERSION)"
	@echo ""
	@echo "GUI App（14 天试用 / 付费）在私有 maclean-pro 仓库构建，不在本仓库。"
