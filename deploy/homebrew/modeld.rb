# Homebrew formula for modeld (C3).
# SHA256 values are filled by CI from modeld-<tag>-SHA256SUMS.txt at release time.
class Modeld < Formula
  desc "Content-addressable storage and deduplication for AI model files"
  homepage "https://github.com/dongfangzhizhu/modeld"
  version "0.2.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/dongfangzhizhu/modeld/releases/download/v#{version}/modeld-v#{version}-macos-aarch64.tar.gz"
      sha256 "REPLACE_WITH_SHA256_MACOS_AARCH64"
    else
      url "https://github.com/dongfangzhizhu/modeld/releases/download/v#{version}/modeld-v#{version}-macos-x86_64.tar.gz"
      sha256 "REPLACE_WITH_SHA256_MACOS_X86_64"
    end
  end

  on_linux do
    url "https://github.com/dongfangzhizhu/modeld/releases/download/v#{version}/modeld-v#{version}-linux-x86_64.tar.gz"
    sha256 "REPLACE_WITH_SHA256_LINUX_X86_64"
  end

  def install
    bin.install "modeld"
    bin.install "modeld-webui"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/modeld --version")
  end
end
