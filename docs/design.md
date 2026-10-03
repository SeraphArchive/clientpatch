# Design

clientpatch has two cooperating components: a Rust loader running inside the
Windows x64 game process, and a .NET WPF manager for configuration, updates,
plugins, and interop generation. They share one version and one release bundle.
This document describes implementation boundaries; it is not a user manual.

## Loader lifecycle

`src/lib.rs` gates initialization to the game process and excludes its CEF helpers.
`DllMain` disables thread notifications and starts a worker; configuration parsing,
runtime discovery, and hook installation run outside the loader lock.
`src/version_proxy.rs` exports the Windows version APIs and resolves the real DLL
through the system directory obtained from Windows, avoiding a fixed installation path.

`src/bootstrap.rs` loads configuration and runs the module lifecycle defined in
`src/module.rs`:

1. Native early hooks run before IL2CPP discovery. BepInEx runs first in this pass
   because Doorstop must intercept runtime initialization; registry and platform
   hooks also need to precede the game's first use of those APIs.
2. Runtime readiness checks precede assembly enumeration. Corlib availability
   alone is insufficient; bootstrap also waits for the assembly count to stabilize.
   The IL2CPP early pass handles hooks that need to precede game initialization.
3. The main pass installs runtime-dependent modules after the readiness gates.
   BepInEx runs last here because setup can generate interop and restart the game.

The IL2CPP binding and detour state live for the process lifetime. Hook installation
stores the trampoline before enabling the detour, and hook bodies fence Rust panics
so they cannot unwind through game or Win32 frames. Missing or invalid configuration
loads no modules; diagnostics remain available.

## Module boundaries

| Module | Responsibility | Source |
| --- | --- | --- |
| `steam` | Steam restart handling and optional native API stubs | `src/modules/steam/` |
| `lilypad` | API/platform origin redirection and response-signature handling | `src/modules/lilypad/` |
| `regredirect` | Redirect the game's registry subtree to a separate sibling key | `src/modules/regredirect.rs` |
| `titlebar` | Reflect observed redirect state in the game window title | `src/modules/titlebar.rs` |
| `interopdump` | Capture runtime IL2CPP inputs and manage the per-build dump cache | `src/modules/interopdump/` |
| `bepinex` | Coordinate Doorstop loading, payload setup, and interop currency | `src/modules/bepinex/` |

Modules are registered at compile time and selected by `enable = true/false` in
their own configuration sections. Missing switches default off; the shipped template
explicitly enables Steam, Lilypad, registry isolation, and titlebar, and disables
interopdump and BepInEx. `[lilypad.report]` controls Lilypad's version-reporting
subcomponent with its own `enable`; reporting requires both parent and child switches.
Legacy top-level `[report]` is migrated on read, and an explicit nested report takes
priority. Legacy `loader.modules` and `enabled`
keys are accepted only for read compatibility; explicit `enable` values take priority.
Manager saves convert the switches to the canonical per-section form.
Parsing, defaults, and validation belong in `src/config.rs`; shared native hook
installation belongs in `src/hook.rs`. Localization is supplied by the separately
distributed HbrLocalize BepInEx plugin, not a second built-in localization hook layer.

### Server redirection

The IL2CPP layer resolves fields and methods by name. It sets the API base URI,
neutralizes route overrides, and reasserts the destination at the non-generic
`PrepareHeaders` funnel. Already constructed request URIs are rewritten directly;
a UnityWebRequest send hook covers requests that bypass the first funnel.
Generic method stubs are not treated as safe native detour targets.
Requests receive the per-launch `x-lilypad-session` header used by the server's
session protocol.

Native GameLib hooks rewrite the official gl-payment origin in platform configuration
strings. They do not redirect unrelated payment origins. Signing hooks support the
configured server mode; RSA public keys are converted to the client's expected
PKCS#1 representation by `keyconv.rs`.

`src/status.rs` distinguishes Pending, Live, and Failed. Installing a hook does not
prove a redirect occurred: Live requires an observed rewrite, and a platform failure
remains visible even if a later API rewrite succeeds. The titlebar consumes this state.

### Registry isolation

Registry hooks rewrite open/create/delete-key paths rather than emulating registry
values. Matching is case-insensitive and segment-aware so similarly named product
keys are unaffected. Windows retains responsibility for value types, enumeration,
and persistence inside the redirected subtree.

## Manager boundaries

`ClientpatchManager.Core` contains filesystem, configuration, feed, launch, backup,
update, plugin, and interop services. `ClientpatchManager.App` supplies WPF pages,
view models, localization, theme handling, and dependency injection. Configuration
form and raw TOML editing share one document through the comment-preserving config
service. Settings changes notify cached pages through the shared settings state.

The manager creates a missing `clientpatch.toml` from `clientpatch.example.toml`.
An existing configuration is preserved. Template initialization uses a temporary
copy and a move without overwrite; bundle installation includes configuration
creation in its recoverable file transaction.

Update selection requires the version-matched Windows ZIP. Extraction and installation
both restrict it to `version.dll`, `manager.exe`, and `clientpatch/`. The installer
validates the binaries and shared version, verifies a supplied checksum, checks that
the game is stopped, and takes the installation lock. Backups and a recovery journal
protect file replacement; the version marker is committed last.

When replacing the running manager, the new bundled executable starts from staging
and waits for the old process to exit before installation. It then restarts the
installed manager. Runtime setup handoffs use the same Core services for payload
installation, interop generation, and relaunch.

## Interop generation

The runtime path captures the decrypted GameAssembly image and metadata from the
game process. The offline path runs Senbei, the compatible Il2CppDumper, and
Il2CppInterop.CLI against staged inputs. Tool releases are resolved from their own
upstream repositories, independently of the clientpatch release feed.

Currency checks compare the installed GameAssembly fingerprint, the latest dump's
build identity, and the generated interop marker. A metadata build ID derives from
decrypted metadata, not its protected on-disk representation. Generation stages a
complete assembly set and promotes it with the completion marker last; failure must
not publish a partially generated set as current. Cache cleanup retains current or
pending inputs and avoids following directory links outside managed storage.

For runtime dump generation and deployment commands, see
[interopgen](../tools/interopgen/README.md).

## Storage and distribution

`version.dll` and `manager.exe` sit beside the game executable. clientpatch's config,
logs, dumps, and version markers belong under `clientpatch/`; BepInEx keeps its own
payload tree. Path resolution must not introduce workstation or sibling-repository
dependencies. Game binaries, keys, generated assemblies, and captures are local
inputs or outputs and are not distributed as project source.

For contributor workflows, see [development](development.md). For asset layout,
release triggers, licensing, and publication checks, see [releasing](releasing.md).
