# clientpatch

A mod loader and manager for Heaven Burns Red, providing server redirection, profile isolation, BepInEx support and more.

## Usage

1. Download the ZIP from [Releases](https://github.com/SeraphArchive/clientpatch/releases)
2. Unzip to your game folder
3. Configure using manager or directly modify `clientpatch/clientpatch.toml` (rename from clientpatch.example.toml first)
4. Launch game

Note that manager requires the [.NET 10 Desktop Runtime](https://dotnet.microsoft.com/download/dotnet/10.0).

## Building

On Windows x64 with PowerShell 7, Rust MSVC, LLVM (`lld-link` on PATH), and the .NET 10 SDK:

```powershell
./build.ps1
```

Packages and SHA-256 checksums appear under `dist/releases/`.

## License

[MIT](./LICENSE).
