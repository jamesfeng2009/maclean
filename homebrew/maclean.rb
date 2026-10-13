class Maclean < Formula
  # maclean CLI — 永久免费开源的 macOS 磁盘清理工具（2026-10-13 商业化边界调整）
  #
  # 公开仓库 = 纯 CLI（Apache-2.0），GUI App（14 天试用 / 付费）在私有
  # maclean-pro 仓库分发，不在本 formula 内。
  #
  # version / sha256（源码 tarball）由 scripts/update-homebrew-formula.sh
  # 在发布 GitHub Release 后自动回填。手动修改容易漏，别手写。

  desc "Free & open-source macOS disk cleaner CLI for developers (Rust)"
  homepage "https://github.com/jamesfeng2009/maclean"
  url "https://github.com/jamesfeng2009/maclean/archive/refs/tags/v0.2.0.tar.gz"
  sha256 "286a9a419a9d92c0cf5a7b2b64db6392b275a04857848c38dc5e5813fde7b7d1"
  license "Apache-2.0"

  depends_on "rust" => :build

  def install
    # 只安装 CLI 二进制（apps/maclean-cli），不构建/安装 GUI。
    system "cargo", "install", "--locked", "--root", prefix, "--path", "apps/maclean-cli"
  end

  test do
    assert_match "maclean", shell_output("#{bin}/maclean --version")
    # 无子命令应提示用法并返回非零（纯 CLI 行为）
    refute_predicate shell_output("#{bin}/maclean 2>&1 || true", 1), :empty?
  end
end
