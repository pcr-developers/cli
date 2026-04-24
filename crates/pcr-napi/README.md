# pcr-dev (npm)

Published to npm as [`pcr-dev`](https://www.npmjs.com/package/pcr-dev).

## How the distribution works

`pcr-dev` itself is a tiny JS package containing only:

- `bin/pcr` — 3-line shim that calls `require("./index.js").run(process.argv)`
- `index.js` — platform selector that `require()`s the right `@pcr-dev/<triple>` subpackage
- `index.d.ts` — TypeScript type for `run(argv)`

The native code lives in 7 platform-specific subpackages:

| Subpackage | Used for |
|---|---|
| `@pcr-dev/darwin-x64` | Intel Macs |
| `@pcr-dev/darwin-arm64` | Apple Silicon Macs |
| `@pcr-dev/linux-x64-gnu` | Linux x64 glibc |
| `@pcr-dev/linux-x64-musl` | Linux x64 musl (Alpine) |
| `@pcr-dev/linux-arm64-gnu` | Linux arm64 |
| `@pcr-dev/win32-x64-msvc` | Windows x64 |
| `@pcr-dev/win32-arm64-msvc` | Windows arm64 |

`pcr-dev`'s `optionalDependencies` lists all 7; npm only installs the one that matches the user's `os`/`cpu`/`libc`.

## Local development

```bash
# Build the .node addon for the current platform
cd crates/pcr-napi
npx @napi-rs/cli build --platform --release

# Run via the bin shim
./bin/pcr status --plain
```
