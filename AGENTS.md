# Working on clientpatch

This is a standalone Windows x64 repository: Rust builds the loader, and .NET 10
builds the WPF manager. Preserve existing local changes and keep all required build
inputs within this repository. For architecture or module changes, read
[docs/design.md](docs/design.md); for implementation and validation workflows, read
[docs/development.md](docs/development.md). Verify behavior against current source.

## Build and verify

Run commands from the repository root with PowerShell 7. Building requires Rust
MSVC, LLVM (`lld-link` on PATH), and the .NET 10 SDK.

- Run `./check-source.ps1` after changing source, project references, or documentation.
- Run `./build.ps1` for changes to the loader, manager, build scripts, or packaging.
  It restores dependencies, runs Rust and manager tests, builds the release bundle,
  checks proxy exports/API calls, and scans packaged files for build-machine paths.
- For a release version override, use `./build.ps1 -Version X.Y.Z`.
  `-SkipTests` is for local iteration; release validation must include tests.
- For documentation-only changes, source hygiene and `git diff --check` suffice.
- Report skipped checks and distinguish automated verification from real game
  testing, GUI self-update, and a successful GitHub Actions run.

## Release and update constraints

Before changing release workflows, assets, versioning, or update installation,
read [docs/releasing.md](docs/releasing.md).

- The loader and manager share one version and one release ZIP. There is no
  independent manager release or version. `Cargo.toml` supplies the default version;
  `Directory.Build.props` and `build.ps1` propagate it to managed builds.
- Package `clientpatch.dll` as `version.dll` alongside `manager.exe`. Preserve the
  configuration template, shared version marker, and third-party notices.
- Ship the template as `clientpatch/clientpatch.example.toml`. The manager copies it
  to `clientpatch.toml` only when that file is missing; never replace user configuration
  with an example during initialization or updates.
- Keep the default release source `SeraphArchive/clientpatch`. Migrate previous
  default settings without overwriting a user's custom feed.
- Publish assets only from the independent `release.yml` workflow triggered by
  `release: published`. Build the release tag and attach files to the existing Release;
  ordinary CI and tag pushes must not create Releases or upload Release assets.
- Updates must extract and install only `version.dll`, `manager.exe`, and
  `clientpatch/`, regardless of other ZIP entries. Preserve existing user configuration,
  validate the payload and shared version, and retain backup/recovery behavior.
  Commit the version marker last; do not mark a partially installed bundle current.
- A running manager must hand installation to the staged manager and exit before
  its executable is replaced. Keep game-stopped checks and installation locking.
- Test failure cases when changing updates: invalid payload/checksum, mismatched
  version, locked destinations, and restoration of the loader and manager together.

## Source hygiene

- Derive paths from the repository, supplied game directory, or OS APIs. Do not
  commit workstation paths or require sibling checkouts. Keep project references
  and documentation file links inside this repository.
- Synthetic absolute paths and traversal strings in security/path tests are
  intentional fixtures; they must not become production filesystem dependencies.
- Keep game binaries, keys, captures, generated interop assemblies, downloaded
  tools, runtime state, and build outputs untracked. Update `.gitignore` when adding
  generated output locations. Keep `Cargo.lock` tracked and use locked Cargo builds.
- Preserve source-path remapping, published-symbol exclusion, and package path
  scanning. Review generated licenses/notices when dependencies change.
- Keep README minimal. Put maintainer release details in `docs/releasing.md`.
  Module activation uses per-section `enable`, never a newly written loader array.
  Version reporting belongs under `lilypad.report` and requires Lilypad to be enabled.
  Keep config comments short; user-facing explanations belong in a future manual.
  Do not rewrite Git history to remove paths; clean current files instead.
