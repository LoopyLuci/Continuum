# API Stability Policy

## Semantic Versioning

This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html):
- **MAJOR**: Breaking API changes
- **MINOR**: New features, backward-compatible
- **PATCH**: Bug fixes, backward-compatible

## Stability Guarantees

- **Public APIs**: Stable across MINOR versions, may break in MAJOR
- **Internal APIs**: May change without notice
- **Configuration files**: Versioned, backward-compatible for at least 2 major releases
- **Network protocol**: Versioned, backward-compatible for at least 2 major releases

## Deprecation Policy

When deprecating APIs:
1. Mark with `#[deprecated(note = "...")]`
2. Provide migration path in documentation
3. Remove only in next MAJOR version
4. Announce in CHANGELOG.md

## Feature Flags

Optional features must:
- Have a clear purpose
- Document dependencies and trade-offs
- Maintain default-feature parity
- Not break core functionality when disabled
