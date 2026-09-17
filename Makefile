.PHONY: build release package clean install test

# maclean Makefile

# 版本号单一来源：Cargo.toml（不要在 Makefile 里再写一份，否则三处不一致）
VERSION := $(shell grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

## 编译 debug 版本
build:
	cargo build

## 编译 release 版本
release:
	cargo build --release

## 运行 (debug)
run: build
	./target/debug/maclean

## 开发者模式运行（跳过 License/额度限制）
dev: build
	MACLEAN_DEV=1 ./target/debug/maclean

## 开发者模式运行（release）
dev-release: release
	MACLEAN_DEV=1 ./target/release/maclean

## 运行 (release)
run-release: release
	./target/release/maclean

## 打包 .pkg 和 .dmg
package: release
	@bash scripts/build-package.sh

## 快速打包 (跳过编译)
package-only:
	@bash scripts/build-package.sh

## 安装 GUI app 到 /Applications (需要 sudo)
install-app: package
	@echo "安装 maclean.app 到 /Applications..."
	@bash scripts/install-app.sh
	@echo "安装完成，请在 Launchpad 或 /Applications 中打开 maclean"

## 卸载 GUI app
uninstall-app:
	@echo "卸载 maclean.app..."
	@bash scripts/uninstall-app.sh
	@echo "卸载完成"

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

## 运行测试
test:
	cargo test --bin maclean

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
	@echo "  make build       - 编译 debug 版本"
	@echo "  make release     - 编译 release 版本"
	@echo "  make run         - 运行 debug 版本"
	@echo "  make dev         - 开发者模式运行（无 License 限制）"
	@echo "  make dev-release - 开发者模式运行 release 版本"
	@echo "  make run-release - 运行 release 版本"
	@echo "  make package     - 打包 .pkg 和 .dmg"
	@echo "  make install     - 安装到 /usr/local/bin"
	@echo "  make uninstall   - 卸载"
	@echo "  make clean       - 清理构建产物"
	@echo "  make check       - 检查编译"
	@echo "  make test        - 运行单元测试"
	@echo "  make fmt         - 检查代码格式"
	@echo "  make lint        - Clippy 静态检查（-D warnings）"
	@echo "  当前版本: $(VERSION)"
