# Releasing `pcr-dev`

This repo now produces two artifacts from a single Rust codebase rooted at
the repo root:

1. **Standalone native binaries** for Homebrew + GitHub Release direct download
   (`pcr-macos-arm64`, `pcr-macos-x64`, `pcr-linux-x64`, `pcr-linux-arm64`,
   `pcr-linux-x64-musl`, `pcr-windows-x64.exe`, `pcr-windows-arm64.exe`).
2. **napi-rs `.node` addons** packaged as the `pcr-dev` npm meta package plus
   seven `@pcr-dev/<triple>` per-platform optional subpackages.

The Go tree under `cli/` is retained during the transition for rollback. Its
build is not used by the release workflow anymore.

## Cutting a stable release (`v0.2.0` and onward)

1. Bump the workspace version.

   ```bash
   # Cargo.toml — workspace.package.version
   # crates/pcr-napi/package.json — "version"
   # crates/pcr-napi/npm/*/package.json — "version" (all seven)
   # crates/pcr-napi/package.json — optionalDependencies entries
   ```

2. Commit, tag, push.

   ```bash
   git add -A
   git commit -m "release v0.2.0"
   git tag v0.2.0
   git push && git push --tags
   ```

   The tag push triggers `.github/workflows/release.yml`, which runs:

   - `lint` — `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`
   - `binaries` — 7-target matrix producing standalone binaries
   - `napi` — 7-target matrix producing per-triple `.node` files
   - `npm-publish` — publishes the 7 subpackages + the meta `pcr-dev`
   - `release` — creates the GitHub Release with binaries + `.sha256`s
   - `homebrew` — dispatches to the `pcr-developers/homebrew-pcr` repo so it
     re-renders `Formula/pcr.rb` from the template.

## Beta (next) channel

Publish a new Rust build as `pcr-dev@next` on npm and as a brew `--HEAD` so
interested users can opt in before the default tag flips:

```bash
cd crates/pcr-napi
npm version 0.2.0-beta.1 --no-git-tag-version
for d in npm/*/; do (cd "$d" && npm version 0.2.0-beta.1 --no-git-tag-version); done
# Publish each subpackage, then the meta:
for d in npm/*/; do (cd "$d" && npm publish --tag next --access public); done
npm publish --tag next --access public
```

For Homebrew, add a temporary `head` URL to the formula pointing at
`pcr-developers/cli` on the Rust branch:

```ruby
head "https://github.com/pcr-developers/cli.git", branch: "main"
```

Users install with `brew install --HEAD pcr-developers/pcr/pcr` for beta.

The stable tag on npm (`latest`) and the stable formula (no `head`) keep
pointing at the previous Go release until the cutover step below.

## Rollback (`@legacy`)

Before cutting the first Rust release, tag the final Go build as the
permanent rollback:

```bash
# One-time, using the last green Go build (v0.1.14 or whatever shipped):
npm dist-tag add pcr-dev@0.1.14 legacy
# Pin the homebrew tap's old formula in a `Formula/pcr@legacy.rb` alias
cp homebrew-pcr/Formula/pcr.rb homebrew-pcr/Formula/pcr@legacy.rb
# Commit to the homebrew repo.
```

After that, `npm install -g pcr-dev@legacy` and `brew install pcr@legacy`
give any user the Go build in one command if the Rust build ever breaks
for them.

## Cutover (flip `latest` to Rust)

After the beta window (usually two release cycles):

```bash
# npm — retag the tested Rust release as `latest`
npm dist-tag add pcr-dev@0.2.0 latest

# homebrew — the tap's automation PR from the release workflow already
# swapped pcr.rb to the v0.2.0 Rust URLs/sha256s, so nothing extra here.

```

The Go sources were removed as part of the rust-port merge commit. If
you ever need the old code, check out the `v0.1.17` tag (the last Go
release) or any commit before the rust-port merge.

## Required GitHub secrets

| Secret | Purpose |
|---|---|
| `NPM_TOKEN` | Publish the pcr-dev meta + 7 subpackages |
| `HOMEBREW_TAP_TOKEN` | Dispatch event to `pcr-developers/homebrew-pcr` |
| `AZURE_*`, `TRUSTED_SIGNING_*` | (Optional) Windows Authenticode signing via Azure Trusted Signing |
| `APPLE_DEVELOPER_ID`, `APPLE_TEAM_ID`, `APPLE_API_KEY` | (Optional) macOS codesign + notarization |

The signing secrets are not required for the Rust build to function on an
AppLocker-locked machine — AppLocker evaluates `CreateProcess` and not
`LoadLibrary`, and the new distribution never invokes `CreateProcess` on a
PCR-shipped binary on that machine. Signing is purely for SmartScreen
reputation (Windows) and Gatekeeper (macOS) on unmanaged home installs.
