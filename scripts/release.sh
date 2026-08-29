#!/usr/bin/env bash
set -euo pipefail

# Release script for Continuum
# Usage: ./scripts/release.sh <version>
# Example: ./scripts/release.sh 0.3.0

VERSION="${1:?Usage: $0 <version>}"
echo "Preparing release v${VERSION}"

# Validate version format
if [[ ! "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Error: Version must be in format X.Y.Z"
    exit 1
fi

# Run tests
echo "Running tests..."
cargo test --workspace --quiet --tests -- --skip continuum_ai

# Run clippy
echo "Running clippy..."
cargo clippy --workspace --quiet -- -D warnings

# Run formatter check
echo "Checking formatting..."
cargo fmt -- --check

# Build release binaries
echo "Building release binaries..."
cargo build --release --workspace --exclude continuum-ai --exclude continuum-plugin-sdk

# Run deny check
echo "Running cargo deny..."
cargo deny check

# Update Cargo.toml versions
echo "Updating Cargo.toml versions..."
find src -name "Cargo.toml" -exec sed -i 's/^version = ".*"$/version = "'"${VERSION}"'"/' {} \;

# Generate changelog entry
echo "## [${VERSION}] - $(date +%Y-%m-%d)" > CHANGELOG_NEW.md
echo "" >> CHANGELOG_NEW.md
echo "### Added" >> CHANGELOG_NEW.md
echo "- See git log for changes" >> CHANGELOG_NEW.md
echo "" >> CHANGELOG_NEW.md
cat CHANGELOG.md >> CHANGELOG_NEW.md
mv CHANGELOG_NEW.md CHANGELOG.md

# Commit changes
git add -A
git commit -m "chore: release v${VERSION}"

echo "Release v${VERSION} prepared successfully!"
echo "Next steps:"
echo "  git push origin main"
echo "  git tag -a v${VERSION} -m 'Release v${VERSION}'"
echo "  git push origin v${VERSION}"
echo "  Create GitHub release with artifacts from target/release/"
