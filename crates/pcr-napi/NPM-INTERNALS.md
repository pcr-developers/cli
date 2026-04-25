# `pcr-dev` — npm distribution internals

Published as [`pcr-dev`](https://www.npmjs.com/package/pcr-dev). This is the
package you get from `npm install -g pcr-dev`.

For everything `pcr` does as a tool, see the [main README](../../README.md).
This file documents how the npm distribution is shaped, why, and how to
rebuild it locally.

> **Note:** the user-facing README that ships on npmjs.com is the project's
> top-level [`README.md`](../../README.md). The release workflow copies it
> into this directory at publish time (see step in
> [`.github/workflows/release.yml`](../../.github/workflows/release.yml)).
> This file (`NPM-INTERNALS.md`) stays repo-only and is not published.

## Why the npm distribution exists at all

The `pcr` CLI is also available as a standalone binary (Homebrew, GitHub
Releases). The npm distribution is here for one reason: **it works on
Windows machines locked down with AppLocker / WDAC** — typical of corporate
laptops where unsigned executables in user-writable directories are blocked
by policy.

Most CLI tools ship a standalone `.exe` and ask you to drop it on `PATH`.
On a managed Windows machine, that `.exe` lives in a user-writable
directory (`%AppData%\npm\node_modules\…`), AppLocker sees a `CreateProcess`
call to an unsigned binary in an untrusted path, and refuses to launch it.
You get `spawn UNKNOWN` and there's nothing you can do without admin rights.

`pcr-dev` sidesteps this by shipping the Rust code as a Node native addon
(a `.node` file, which is just a DLL). The npm shim runs `require('./index.js')`,
which `LoadLibrary`s the `.node` into the already-running `node.exe` — and
`node.exe` is a signed, trusted binary that AppLocker has no problem with.
No second `CreateProcess`, no policy violation, no admin needed.

## Anatomy

`pcr-dev` itself is a tiny pure-JS package that contains:

```text
pcr-dev/
├── bin/pcr        # 4-line shim: require("../index.js").run(["pcr", ...argv])
├── index.js       # platform selector — picks the right native subpackage
├── index.d.ts     # TypeScript types for the run() export
└── package.json   # optionalDependencies pin one subpackage per platform
```

The actual native code is in **per-platform subpackages**, each of which
contains exactly one `.node` file built from this crate. npm only installs
the subpackage matching the user's `os` / `cpu` / `libc` triple, so the
download is small even though we publish many architectures:

| Subpackage | Used for | Triple |
|---|---|---|
| `pcr-dev-darwin-arm64` | Apple Silicon Macs | `aarch64-apple-darwin` |
| `pcr-dev-darwin-x64`   | Intel Macs | `x86_64-apple-darwin` |
| `pcr-dev-linux-x64-gnu`| Linux x64 (glibc) | `x86_64-unknown-linux-gnu` |
| `pcr-dev-windows-x64`  | Windows x64 | `x86_64-pc-windows-msvc` |

The Windows subpackage is named `windows-x64` (not the conventional
`win32-x64-msvc`) because npm's anti-typosquatting heuristic flags
`win32` in unscoped package names. Renaming once cost an evening of
`403 Forbidden`s — please don't change it back.

## Install + run flow

```text
$ npm install -g pcr-dev
   ├─ npm reads optionalDependencies
   └─ installs pcr-dev-<your-platform> as the only matching one

$ pcr start
   ├─ npm shim invokes:  node bin/pcr  start
   ├─ bin/pcr does:      require("../index.js").run(["pcr", "start"])
   ├─ index.js does:     require("pcr-dev-<triple>")     ← LoadLibrary
   └─ Rust entrypoint takes over inside node.exe         ← no new process
```

The whole thing is one process. `node.exe` is the only executable that
ever launches, and it's signed by the Node.js Foundation — exactly the
property AppLocker cares about.

## Local development

```bash
# Build the .node addon for your current platform
cd cli/crates/pcr-napi
npx @napi-rs/cli build --platform --release

# Run via the bin shim (without installing globally)
./bin/pcr help
./bin/pcr status --plain
```

`@napi-rs/cli build` produces a `pcr.<triple>.node` next to `index.js`;
`index.js` will pick it up automatically when you `require()` from
inside this directory.

## Publishing

You don't publish from your laptop — the [release workflow](../../.github/workflows/release.yml)
does it for you. See [RELEASING.md](../../RELEASING.md) for the full
procedure. The short version:

1. Tag the repo (e.g. `git tag v0.2.3 && git push --tags`).
2. CI builds one `.node` per target on the matching runner.
3. The `npm-publish` job uploads each subpackage, then publishes the
   meta `pcr-dev` package with matching `optionalDependencies`.

## Troubleshooting

- **`Unsupported platform/arch`** — your platform isn't in the table above.
  [Open an issue](https://github.com/pcr-developers/cli/issues) and we'll
  add a build target.
- **`spawn UNKNOWN` on Windows** — somehow you're getting a standalone
  binary instead of the addon path. Confirm `npm root -g` shows
  `pcr-dev/bin/pcr` (the JS shim, not a `.exe`), and that the right
  `pcr-dev-<triple>` subpackage is installed alongside it.
- **`Cannot find module 'pcr-dev-<triple>'`** — npm skipped the optional
  dependency, usually because of a stale `node_modules`. Reinstall with
  `npm install -g pcr-dev --force`.
