namespace ClientpatchManager.Core.Config;

/// <summary>
/// Loads and edits clientpatch.toml while preserving the original document's
/// comments (full-line and inline) and section order.
/// </summary>
public interface IConfigService {
    /// <summary>Parse TOML text into the typed <see cref="ClientpatchConfig"/> model.</summary>
    ClientpatchConfig Load(string tomlText);

    /// <summary>
    /// Apply <paramref name="edited"/> back onto the ORIGINAL <paramref name="tomlText"/>,
    /// mutating only the changed value nodes in place so comments and section order survive.
    /// </summary>
    string ApplyToText(string tomlText, ClientpatchConfig edited);

    /// <summary>Validate raw TOML. Returns "" when valid, else the parse error message.</summary>
    string RawValidate(string tomlText);
}
