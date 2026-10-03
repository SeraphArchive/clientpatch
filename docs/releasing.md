# Releasing

Run `./build.ps1 -Version X.Y.Z` from a Windows x64 checkout. Tests are required
for release; `-SkipTests` is only for local iteration. The script uses repository-local
build outputs and never deploys to a game installation.

The release ZIP is the only update payload. It includes the loader renamed to
`version.dll`, `manager.exe`, and `clientpatch/` as its only root entries.
The configuration example, README.md, LICENSE, and dependency notices live under `clientpatch/`.
The manager creates `clientpatch/clientpatch.toml` from the example only when missing,
at startup, when selecting an installation, on Home refresh, or during bundle installation.
Updates replace the example but preserve an existing user configuration.
The shared version is recorded in `clientpatch/clientpatch.version` and stamped
into the manager by the build script. No separate manager version or download is published.
The manager selects the ZIP matching `clientpatch-vX.Y.Z-win-x64.zip`, verifies its
checksum when present, preserves existing configuration, backs up replaced files,
and commits the shared version last. When updating itself, it starts the new bundled
manager from a temporary directory, waits for the old process to exit, applies the
bundle, and restarts the installed manager. Failed commits use the recovery journal.
Restoring a backup that replaces the running manager uses the same staged handoff:
the temporary manager waits for the old process to exit, restores the selected backup,
and restarts the restored manager when present. Restores commit completion markers last.
Only `version.dll`, `manager.exe`, and files under `clientpatch/` are extracted and
installed during updates. Other ZIP entries, including root documentation and license
directories, are ignored; unrelated existing game files are left alone.
A handoff leaves its temporary bundle available for diagnosis/recovery.

Before the first public release:

- Review generated dependency notices when changing dependencies.
  MIT covers this project's code; Rust and NuGet dependencies retain their own licenses.
  The package includes a dependency inventory and upstream license/notice files under
  `clientpatch/licenses/`, including the vendored disassembler notices. The inventory
  and project license also live under `clientpatch/` so updates include them.
  Missing non-MIT license files
  stop packaging until a reviewed license copy is provided.
- Smoke-test the ZIP on a clean Windows installation with the .NET 10 Desktop Runtime,
  including first installation, preserved configuration, self-update, and rollback.
- Check upstream interop tool availability and the pinned dumper compatibility with
  the intended game build. Interop tools and game-derived data are fetched/generated
  separately and are not bundled.
- Create and publish a GitHub Release using a `clientpatch-vX.Y.Z` tag. Choose the
  prerelease flag in GitHub when appropriate (for example `clientpatch-vX.Y.Z-beta.1`).
  The independent `release.yml` workflow checks out that tag, builds and tests it,
  then uploads the ZIP and checksum to the existing release. It does not create or
  edit release notes or change the prerelease flag. Re-running it replaces matching
  asset names. A failed build/test leaves the published release without new assets.
  Actions must be allowed, and the release job needs contents-write permission.
  Branch pushes, pull requests, and manual CI runs build only; pushing a tag alone
  does not attach release assets.
- Consider Authenticode signing for public binaries and pinning Actions to reviewed
  commit hashes before enabling releases.

## Source and packaging hygiene

Build and publish require no sibling checkout or game files. Optional smoke-plugin
references and interop tools are placed in ignored directories inside `tools/` or supplied
explicitly. All project references resolve within this repository.

Workstation paths and sibling-project links were removed from tracked documentation
and configuration. The version proxy resolves the system directory at runtime rather
than assuming a Windows installation drive. Absolute paths in unit tests and historical
test examples are synthetic
inputs for Windows path parsing, command-line quoting, and rejection of unsafe paths;
they are not filesystem dependencies. Relative traversal strings in security tests are
intentional. Architecture and contributor workflows are described in
[design](design.md) and [development](development.md).

`dist/`, Rust/.NET outputs, tool downloads, reference DLLs, secrets, logs, crash dumps,
local settings, IDE caches, and agent-session files are ignored. `Cargo.lock` stays tracked.
Only explicitly staged bundle files are packaged. Rust source paths are remapped and .NET
symbols are disabled in published binaries to avoid embedding build-machine paths.
Packaging rejects manager publish sidecars because the update contract installs only
the single `manager.exe` at the game root.
Git history is untouched; initialize the public repository from the reviewed source tree.
