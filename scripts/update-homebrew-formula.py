"""Render the cargo-cleaner formula from GitHub's release API response."""

import json
import re
import sys

release = json.load(sys.stdin)
tag = release["tag_name"]
if not re.fullmatch(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", tag):
    raise SystemExit(f"Unexpected release tag {tag!r}")

assets = {asset["name"]: asset.get("digest") for asset in release["assets"]}
platforms = {
    "macos": {
        "arm": "aarch64-apple-darwin",
        "intel": "x86_64-apple-darwin",
    },
    "linux": {
        "arm": "aarch64-unknown-linux-gnu",
        "intel": "x86_64-unknown-linux-gnu",
    },
}

lines = [
    "class CargoCleaner < Formula",
    '  desc "Find and clean Cargo build caches"',
    '  homepage "https://github.com/gridness/cargo-cleaner"',
    f'  version "{tag[1:]}"',
    '  license "MIT"',
]
for platform, architectures in platforms.items():
    lines.extend(["", f"  on_{platform} do"])
    if platform == "macos":
        lines.extend(["    depends_on macos: :ventura", ""])
    for architecture, target in architectures.items():
        filename = f"cargo-cleaner-{tag}-{target}.tar.gz"
        digest = assets.get(filename)
        if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest or ""):
            raise SystemExit(f"Missing SHA-256 for {filename}")
        url = f"https://github.com/gridness/cargo-cleaner/releases/download/{tag}/{filename}"
        lines.extend([
            f"    on_{architecture} do",
            f'      url "{url}"',
            f'      sha256 "{digest.removeprefix("sha256:")}"',
            "    end",
        ])
    lines.append("  end")

print("\n".join(lines))
print('''
  def install
    bin.install "cargo-cleaner"
    bash_completion.install "completions/cargo-cleaner.bash" => "cargo-cleaner"
    zsh_completion.install "completions/_cargo-cleaner"
    fish_completion.install "completions/cargo-cleaner.fish"
  end

  test do
    assert_equal "cargo-cleaner #{version}", shell_output("#{bin}/cargo-cleaner --version").strip
    assert_match "Find Cargo projects", shell_output("#{bin}/cargo-cleaner scan --help")
  end
end''')
