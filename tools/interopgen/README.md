# interopgen

Generate BepInEx interop assemblies from a clientpatch runtime dump using
Il2CppDumper and Il2CppInterop.CLI. Run once per game build.

## Generate

Use PowerShell 7 and the .NET runtimes required by the tools. Launch the game
once with `interopdump.enable = true` and `restore_handles = false`, then close it.
Supply Il2CppDumper and Il2CppInterop.CLI:

```powershell
pwsh -File tools/interopgen/interopgen.ps1 -GameDir '<game-directory>' `
  -DumperDir '<Il2CppDumper-directory>' -CliDll '<Il2CppInterop.CLI.dll>'
```

The default tool locations are `tools/interopgen/tools/Il2CppDumper/` and
`tools/interopgen/tools/Il2CppInterop.CLI/Il2CppInterop.CLI.dll`.
Output goes to `<game-directory>/clientpatch/interop/<build_id>/interop/`.
Use `-Force` to regenerate; failed generation preserves the previous assembly set.

## Deploy

With the game stopped, supply an extracted BepInEx IL2CPP Windows x64 payload:

```powershell
pwsh -File tools/interopgen/deploy_bepinex.ps1 -GameDir '<game-directory>' `
  -BepInExSource '<BepInEx-payload-directory>'
```

The default payload location is `tools/interopgen/tools/BepInEx/`.
Use `-Update` to replace an existing installation. Deployment installs Doorstop as
`doorstop.dll` for clientpatch to load and disables BepInEx's own interop generation.
