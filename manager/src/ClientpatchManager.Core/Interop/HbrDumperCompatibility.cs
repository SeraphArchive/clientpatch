namespace ClientpatchManager.Core.Interop;

public interface IToolPreparer {
    string PrepareDumper(string cachedDirectory, string scratch);
}

/// <summary>Copies the supplied Il2CppDumper into a private working directory.
/// Tool binaries, shared cache files, and game inputs remain unchanged.</summary>
public sealed class HbrDumperCompatibility : IToolPreparer {
    public string PrepareDumper(string cachedDirectory, string scratch) {
        var copy = Path.Combine(scratch, "dumper-tool");
        Directory.CreateDirectory(copy);
        foreach (var source in Directory.GetFiles(cachedDirectory, "*", SearchOption.AllDirectories)) {
            var target = Path.Combine(copy, Path.GetRelativePath(cachedDirectory, source));
            Directory.CreateDirectory(Path.GetDirectoryName(target)!); File.Copy(source, target);
        }
        _ = Directory.GetFiles(copy, "Il2CppDumper.dll", SearchOption.AllDirectories).SingleOrDefault()
            ?? throw new InvalidDataException("Compatible dumper bundle must contain Il2CppDumper.dll.");
        return copy;
    }
}
