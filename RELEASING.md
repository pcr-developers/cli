# Releasing

## How to release a new version

1. Bump the version in `npm/package.json`
2. Commit, tag, and push:

```bash
git add npm/package.json
git commit -m "release vX.Y.Z"
git tag vX.Y.Z
git push && git push --tags
```

The tag push triggers CI, which does everything else automatically:

- Builds Go binaries for macOS (arm64, x64) and Linux (x64)
- Creates a GitHub Release with the binaries and their SHA256 checksums
- Publishes the npm package (`pcr-dev`) with the version set from the tag
- Updates the Homebrew formula in `pcr-developers/homebrew-pcr`

## Required secrets (in the `cli` repo)

| Secret | Purpose |
|---|---|
| `NPM_TOKEN` | Publish to npm |
| `HOMEBREW_TAP_TOKEN` | Dispatch event to the homebrew-pcr repo |
