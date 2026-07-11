.PHONY: build release package clean install test

# maclean Makefile

VERSION := 0.1.0

## 编译 debug 版本
build:
	cargo build

## 编译 release 版本
release:
	cargo build --release

## 运行 (debug)
run: build
	./target/debug/maclean

## 运行 (release)
run-release: release
	./target/release/maclean

## 打包 .pkg 和 .dmg
package: release
	@bash scripts/build-package.sh

## 快速打包 (跳过编译)
package-only:
	@bash scripts/build-package.sh

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

## 帮助
help:
	@echo "maclean Makefile"
	@echo ""
	@echo "可用命令:"
	@echo "  make build       - 编译 debug 版本"
	@echo "  make release     - 编译 release 版本"
	@echo "  make run         - 运行 debug 版本"
	@echo "  make run-release - 运行 release 版本"
	@echo "  make package     - 打包 .pkg 和 .dmg"
	@echo "  make install     - 安装到 /usr/local/bin"
	@echo "  make uninstall   - 卸载"
	@echo "  make clean       - 清理构建产物"
	@echo "  make check       - 检查编译"
