# Development

Use [the design](design.md) to locate component boundaries and
[the release guide](releasing.md) when changing versions, packaging, or publication.
Current source is authoritative for signatures, configuration defaults, and runtime
behavior. These documents replace dated implementation task lists and duplicated
source snippets.

## Repository map

| Location | Purpose |
| --- | --- |
| `src/lib.rs`, `src/bootstrap.rs` | Process gating and loader initialization |
| `src/version_proxy.rs` | System version API forwarding |
| `src/il2cpp.rs`, `src/ffi.rs`, `src/hook.rs` | Runtime binding and native detour support |
| `src/config.rs`, `clientpatch.toml` | Loader configuration schema and release template |
| `src/module.rs`, `src/modules/` | Module lifecycle, registry, and implementations |
| `src/logging.rs`, `src/crashguard.rs`, `src/status.rs` | Diagnostics and observed runtime state |
| `manager/src/ClientpatchManager.Core/` | Manager services and persistence |
| `manager/src/ClientpatchManager.App/` | WPF UI, view models, and resources |
| `manager/tests/` | Core and application regression tests |
| `tools/` | Optional interop utilities, proxy verification, and license packaging |

## Build and focused checks

Build on Windows x64 with PowerShell 7, Rust MSVC, LLVM's `lld-link` on PATH,
and the .NET 10 SDK. From the repository root:

```powershell
./build.ps1
```

This runs source hygiene, dependency restore, Rust and manager tests, release builds,
proxy export/API verification, package license generation, and build-path scanning.
It also runs isolated PowerShell regressions for interop generation and deployment,
including failed promotion and preservation of previously installed files.
Outputs remain under ignored `target/`, manager `bin/` and `obj/`, and `dist/`.
`-SkipTests` is for local iteration and does not establish release readiness.

For focused Rust work, run
`cargo test --locked --target x86_64-pc-windows-msvc`, optionally with a test-name filter.
For manager work, restore the solution with the repository NuGet configuration,
then run the affected test project or solution:

```powershell
dotnet restore manager/ClientpatchManager.sln -r win-x64 --configfile NuGet.Config
dotnet test manager/ClientpatchManager.sln -c Release -r win-x64 --no-restore
```

For documentation-only edits, run `./check-source.ps1` and `git diff --check`.
Keep config comments brief; detailed end-user explanations belong in a future user
manual. Preserve example values and enabled modules unless a behavior change is intended.

## Changing loader modules

Choose the lifecycle pass according to the first native or managed call that the
hook must precede. Register new modules in `src/module.rs`, keep their configuration
schema and validation in `src/config.rs`, and update the example only as needed.
Every module uses a boolean `enable` in its own table. Keep loader and manager
defaults, legacy read normalization, validation, and form/raw editing consistent.
Put pure parsing, matching, and state decisions outside Windows hook bodies so
they can be exercised independently.

Resolve managed symbols by name when possible. For native addresses or structures
that depend on the game build, retain the source/capture/native evidence that supports
them and make unsupported builds explicit. Do not replace a failed resolution with
an invented behavior or a success claim.

Preserve trampoline-before-enable ordering, process-lifetime hook state, panic
fences, initialization readiness gates, and safe handling of null/unavailable APIs.
Module activation, hook installation, and observed runtime success are distinct states.

## Changing manager services

Keep service logic in Core and UI behavior in App. Changes to forms and raw TOML
editing must preserve one shared document and validation before saving. Keep all
localized resource key sets synchronized.

Filesystem mutations must preserve game-stopped checks, installation locking,
path containment, backup/recovery behavior, and completion-marker ordering. For updates,
cover invalid payloads/checksums, version mismatches, extra archive entries, locked
destinations, existing/missing configuration, and rollback of the shared bundle.
For interop changes, cover stale fingerprints, incomplete generation, interrupted
promotion, and cleanup while a build is pending.

## Runtime verification

Enable `[diagnostics]` with `enable = true` in the configuration file for native
startup investigation. The manager has no diagnostics controls and preserves this
section when saving other settings. Both `native_init` and `crash_context` default
to true within the enabled section; the entire module defaults off.

`native_init` records GameLib/Steam module bases through the shared loader watcher.
In real Steam modes it observes `SteamAPI_Init` results/durations and
`SteamAPI_Shutdown` calls through forwarding detours. It does not install these
observers in stub mode or change their return values.
With Lilypad's payment redirect hooks active, it also records payment initialization
entry, return value, initialized state and elapsed time. Known error markers from
the last 100 console rows are reported after initialization; raw console text is
never logged. These markers may include earlier errors from the same launch.
`crash_context` adds module-relative offsets and RIP/RSP/RAX/RCX to the existing
unhandled-crash report. It does not attach a debugger, intercept recoverable
exceptions, or collect a memory dump, credentials or request bodies.

Module observation is delivered after DLL loading returns; a missing observation
does not prove that loading failed. Initialization and crash logs supplement real
game reproduction and do not replace a stack trace for deadlock attribution.

Unit tests and successful compilation do not prove game compatibility or the GUI
self-update handoff. For hook, bootstrap, Steam, or interop changes, validate against
the intended game build and record which paths were exercised. Check manager setup,
generation, deployment, and relaunch against current source.

Report the build/test result, ignored or skipped checks, and any unverified game,
GUI, or hosted-CI behavior. Keep keys, captures, dumps, and game-derived outputs out
of tracked content; keep `Cargo.lock` and required third-party license notices current.
