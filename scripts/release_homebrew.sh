#!/bin/bash
set -euo pipefail

# -------------------------------------------------------------------
# release_homebrew.sh
#
# 1. Bump patch version in Cargo.toml
# 2. Build web/dist
# 3. cargo build --release
# 4. Create a tar.gz of the binary
# 5. Create a GitHub release and upload the tarball
# 6. Update the homebrew-tap Formula with new version + sha256
# 7. Push tap changes
# -------------------------------------------------------------------

ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
REPO_DIR="$ROOT_DIR"
TAP_DIR_DEFAULT="$ROOT_DIR/../../github/homebrew-tap"
TAP_DIR="${TAP_DIR:-$TAP_DIR_DEFAULT}"
TAP_REPO="everettjf/homebrew-tap"
FORMULA_PATH="Formula/finally-a-value-bot.rb"
GITHUB_REPO="finally-a-value-bot/finally-a-value-bot"

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64) echo "x86_64" ;;
    arm64|aarch64) echo "aarch64" ;;
    *)
      echo "Unsupported architecture: $(uname -m)" >&2
      exit 1
      ;;
  esac
}

latest_release_tag() {
  git tag --list 'v*' --sort=-version:refname | head -n1
}

build_release_notes() {
  local prev_tag="$1"
  local new_tag="$2"
  local out_file="$3"
  local compare_url="https://github.com/$GITHUB_REPO/compare"
  local changes

  if [ -n "$prev_tag" ]; then
    changes="$(git log --no-merges --pretty=format:'%s' "$prev_tag..HEAD" \
      | grep -vE '^bump version to ' \
      | head -n 30 || true)"
  else
    changes="$(git log --no-merges --pretty=format:'%s' \
      | grep -vE '^bump version to ' \
      | head -n 30 || true)"
  fi

  {
    echo "FinallyAValueBot $new_tag"
    echo
    echo "## Change log"
    if [ -n "$changes" ]; then
      while IFS= read -r line; do
        [ -n "$line" ] && echo "- $line"
      done <<< "$changes"
    else
      echo "- Internal maintenance and release packaging updates"
    fi
    echo
    echo "## Compare"
    if [ -n "$prev_tag" ]; then
      echo "$compare_url/$prev_tag...$new_tag"
    else
      echo "N/A (first tagged release)"
    fi
  } > "$out_file"
}

require_cmd cargo
require_cmd git
require_cmd gh
require_cmd shasum
require_cmd tar
require_cmd npm

if ! gh auth status >/dev/null 2>&1; then
  echo "GitHub CLI not authenticated. Run: gh auth login" >&2
  exit 1
fi

cd "$REPO_DIR"

# --- Build web assets (embedded via include_dir! in src/web.rs) ---
if [ -f "web/package.json" ]; then
  echo "Building web assets..."
  if [ -f "web/package-lock.json" ]; then
    npm --prefix web ci
  else
    npm --prefix web install
  fi
  npm --prefix web run build
  test -f "web/dist/index.html" || {
    echo "web/dist/index.html is missing after web build" >&2
    exit 1
  }
  test -f "web/dist/icon.png" || {
    echo "web/dist/icon.png is missing after web build" >&2
    exit 1
  }
  if ! ls web/dist/assets/*.js >/dev/null 2>&1; then
    echo "web/dist/assets/*.js is missing after web build" >&2
    exit 1
  fi
fi

# --- Bump patch version in Cargo.toml ---
PREV_TAG="$(latest_release_tag)"
CURRENT_VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"
NEW_PATCH=$((PATCH + 1))
NEW_VERSION="$MAJOR.$MINOR.$NEW_PATCH"
TAG="v$NEW_VERSION"

if [ "$PREV_TAG" = "$TAG" ]; then
  PREV_TAG="$(git tag --list 'v*' --sort=-version:refname | sed -n '2p')"
fi

sed -i '' "s/^version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" Cargo.toml
echo "Version bumped: $CURRENT_VERSION -> $NEW_VERSION"
if [ -n "$PREV_TAG" ]; then
  echo "Previous tag: $PREV_TAG"
else
  echo "Previous tag: (none)"
fi

# --- Build release binary ---
echo "Building release binary..."
cargo build --release

BINARY="target/release/finally-a-value-bot"
if [ ! -f "$BINARY" ]; then
  echo "Binary not found: $BINARY" >&2
  exit 1
fi

# --- Create tarball ---
ARCH="$(detect_arch)"
TARBALL_NAME="finally-a-value-bot-$NEW_VERSION-${ARCH}-apple-darwin.tar.gz"
TARBALL_PATH="target/release/$TARBALL_NAME"

tar -czf "$TARBALL_PATH" -C target/release finally-a-value-bot
echo "Created tarball: $TARBALL_PATH"

SHA256=$(shasum -a 256 "$TARBALL_PATH" | awk '{print $1}')
echo "SHA256: $SHA256"

# --- Git commit + tag ---
git add .
git commit -m "bump version to $NEW_VERSION"
git tag "$TAG"
git push
git push --tags

# --- GitHub release ---
RELEASE_NOTES_FILE="target/release/release-notes-$TAG.md"
build_release_notes "$PREV_TAG" "$TAG" "$RELEASE_NOTES_FILE"

if gh release view "$TAG" --repo "$GITHUB_REPO" >/dev/null 2>&1; then
  echo "Release $TAG already exists. Skipping create."
else
  gh release create "$TAG" "$TARBALL_PATH" \
    --repo "$GITHUB_REPO" \
    -t "$TAG" \
    -F "$RELEASE_NOTES_FILE"
  echo "Created GitHub release: $TAG"
fi

# --- Update homebrew-tap ---
if [ ! -d "$TAP_DIR/.git" ]; then
  echo "Cloning tap repo..."
  git clone "https://github.com/$TAP_REPO.git" "$TAP_DIR"
fi

cd "$TAP_DIR"

# Create Formula dir if it doesn't exist
mkdir -p Formula

# Write the formula
cat > "$FORMULA_PATH" << RUBY
class Microclaw < Formula
  desc "Agentic AI assistant for Telegram — web search, scheduling, memory, tool execution"
  homepage "https://github.com/$GITHUB_REPO"
  url "https://github.com/$GITHUB_REPO/releases/download/$TAG/$TARBALL_NAME"
  sha256 "$SHA256"
  license "MIT"

  def install
    bin.install "finally-a-value-bot"
  end

  test do
    assert_match "FinallyAValueBot", shell_output("#{bin}/finally-a-value-bot help")
  end
end
RUBY

git add .
git commit -m "finally-a-value-bot homebrew release $NEW_VERSION"
git push

echo ""
echo "Done! Released $TAG and updated Homebrew tap."
echo ""
echo "Users can install with:"
echo "  brew tap everettjf/tap"
echo "  brew install finally-a-value-bot"
